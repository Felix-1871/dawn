// SPDX-License-Identifier: GPL-3.0-or-later

//! The live session's keyboard layout. The Keyboard screen switches the
//! session Dawn runs in to the layout being installed, so its preview
//! field, and the password typed on the Account screen, use the layout
//! the new system will have (DECISIONS.md, M4 follow-ups). LuminOS's live
//! session is Hyprland, which Dawn tells through its control socket, the
//! way `hyprctl` does: `keyword` for a `hyprland.conf` configuration,
//! Lua through `eval` for a `hyprland.lua` one, which refuses `keyword`.

use std::io::{Read as _, Write as _};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

pub trait LiveKeyboard: Send + Sync {
    /// Switches the live session to `layout` and `variant`, empty for the
    /// layout's own default.
    fn switch(&self, layout: &str, variant: &str) -> Result<(), String>;
}

/// For a session Dawn can't switch, and for tests: does nothing.
pub struct NoLiveKeyboard;

impl LiveKeyboard for NoLiveKeyboard {
    fn switch(&self, _layout: &str, _variant: &str) -> Result<(), String> {
        Ok(())
    }
}

/// How long Hyprland gets to answer a request.
const TIMEOUT: Duration = Duration::from_secs(2);

/// A running Hyprland, through its control socket.
pub struct Hyprland {
    socket: PathBuf,
}

impl Hyprland {
    /// The Hyprland instance Dawn runs in, if it runs in one.
    pub fn from_env() -> Option<Self> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
        let instance = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
        Some(Self::at(
            PathBuf::from(runtime)
                .join("hypr")
                .join(instance)
                .join(".socket.sock"),
        ))
    }

    pub fn at(socket: PathBuf) -> Self {
        Self { socket }
    }

    /// Sends one request, as `hyprctl` does (flags, `/`, the command),
    /// and returns the reply.
    fn request(&self, request: &str) -> Result<String, String> {
        let io =
            |err: std::io::Error| format!("Hyprland's socket {}: {err}", self.socket.display());
        let mut stream = UnixStream::connect(&self.socket).map_err(io)?;
        stream.set_read_timeout(Some(TIMEOUT)).map_err(io)?;
        stream.set_write_timeout(Some(TIMEOUT)).map_err(io)?;
        stream.write_all(request.as_bytes()).map_err(io)?;
        stream.shutdown(Shutdown::Write).map_err(io)?;
        let mut reply = String::new();
        stream.read_to_string(&mut reply).map_err(io)?;
        Ok(reply.trim().to_string())
    }

    fn expect_ok(&self, request: &str) -> Result<(), String> {
        let reply = self.request(request)?;
        if reply == "ok" {
            Ok(())
        } else {
            Err(format!("Hyprland answered {reply:?}"))
        }
    }

    /// `hyprland.conf`: one option at a time, each applied at once. The
    /// variant is cleared first, since the old one may not exist for the
    /// new layout.
    fn keyword(&self, layout: &str, variant: &str) -> Result<(), String> {
        self.expect_ok("/keyword input:kb_variant ")?;
        self.expect_ok(&format!("/keyword input:kb_layout {layout}"))?;
        if !variant.is_empty() {
            self.expect_ok(&format!("/keyword input:kb_variant {variant}"))?;
        }
        Ok(())
    }

    /// `hyprland.lua`: both options at once; the `r` flag has Hyprland
    /// apply them.
    fn lua(&self, layout: &str, variant: &str) -> Result<(), String> {
        self.expect_ok(&format!(
            "r/eval hl.config({{ input = {{ kb_layout = \"{layout}\", kb_variant = \"{variant}\" }} }})"
        ))
    }
}

/// XKB names are letters, digits, `_` and `-`; anything else never goes
/// into a request.
fn is_xkb_name(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl LiveKeyboard for Hyprland {
    fn switch(&self, layout: &str, variant: &str) -> Result<(), String> {
        if layout.is_empty() || !is_xkb_name(layout) || !is_xkb_name(variant) {
            return Err(format!("{layout:?} ({variant:?}) isn't a keyboard layout"));
        }
        self.keyword(layout, variant)
            .or_else(|keyword| self.lua(layout, variant).map_err(|_| keyword))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread::JoinHandle;

    use super::*;

    /// A stand-in for Hyprland's socket: answers each request with the
    /// next of `replies`, and returns the requests it got.
    fn fake_hyprland(replies: &[&str]) -> (Hyprland, JoinHandle<Vec<String>>) {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let socket =
            std::env::temp_dir().join(format!("dawn-hypr-{}-{n}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let replies: Vec<String> = replies.iter().map(|reply| reply.to_string()).collect();
        let path = socket.clone();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for reply in replies {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = String::new();
                stream.read_to_string(&mut request).unwrap();
                requests.push(request);
                stream.write_all(reply.as_bytes()).unwrap();
            }
            let _ = std::fs::remove_file(path);
            requests
        });
        (Hyprland::at(socket), server)
    }

    #[test]
    fn switches_a_hyprland_conf_session_one_option_at_a_time() {
        let (hyprland, server) = fake_hyprland(&["ok", "ok", "ok"]);
        hyprland.switch("de", "nodeadkeys").unwrap();
        assert_eq!(
            server.join().unwrap(),
            [
                "/keyword input:kb_variant ",
                "/keyword input:kb_layout de",
                "/keyword input:kb_variant nodeadkeys"
            ]
        );
    }

    #[test]
    fn a_layouts_default_variant_needs_no_third_request() {
        let (hyprland, server) = fake_hyprland(&["ok", "ok"]);
        hyprland.switch("us", "").unwrap();
        assert_eq!(server.join().unwrap().len(), 2);
    }

    #[test]
    fn a_hyprland_lua_session_gets_lua() {
        let (hyprland, server) = fake_hyprland(&[
            "keyword can't work with non-legacy parsers. Use eval.",
            "ok",
        ]);
        hyprland.switch("de", "").unwrap();
        assert_eq!(
            server.join().unwrap()[1],
            r#"r/eval hl.config({ input = { kb_layout = "de", kb_variant = "" } })"#
        );
    }

    #[test]
    fn when_both_ways_fail_the_first_error_is_the_one_reported() {
        let (hyprland, server) = fake_hyprland(&[
            "no such option",
            "eval is only supported with the lua config manager",
        ]);
        let err = hyprland.switch("de", "").unwrap_err();
        assert!(err.contains("no such option"), "{err}");
        server.join().unwrap();
    }

    #[test]
    fn refuses_anything_but_an_xkb_name() {
        let hyprland = Hyprland::at(PathBuf::from("/nonexistent/socket"));
        assert!(hyprland.switch("de; exec rm", "").is_err());
        assert!(hyprland.switch("de", "x\"y").is_err());
        assert!(hyprland.switch("", "").is_err());
    }
}
