// SPDX-License-Identifier: GPL-3.0-or-later

//! The single place every external command is described. Every pipeline
//! step builds [`Invocation`]s here instead of formatting a shell string,
//! so dry-run, logging and password redaction all happen in one spot.
//!
//! M0 only implements the dry-run path (see [`DryRunRunner`]); a runner
//! that actually spawns processes arrives in M1, gated by the safety rules
//! in CLAUDE.md (never outside `--dry-run` except inside a disposable VM).

use std::fmt::Write as _;

/// What, if anything, is written to an invocation's stdin. `chpasswd` and
/// similar tools take secrets this way instead of as an argv entry, which
/// would otherwise be visible to anyone who can run `ps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stdin {
    Plain(String),
    /// Real content is carried for a future real runner, but dry-run
    /// output (and `Debug`) never shows it.
    Redacted(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<Stdin>,
    /// Extra context shown next to the command in dry-run output, e.g.
    /// "output appended to /mnt/target/etc/fstab".
    pub note: Option<String>,
}

impl Invocation {
    pub fn new<I, S>(program: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            stdin: None,
            note: None,
        }
    }

    pub fn with_stdin(mut self, stdin: Stdin) -> Self {
        self.stdin = Some(stdin);
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// One thing a pipeline step does: run an external command, or write a
/// file directly. File edits (locale.gen, hostname, kernel cmdline, ...)
/// go through `WriteFile` rather than a shelled-out `echo` or `sed`, since
/// the real runner can just do the write itself in Rust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Run(Invocation),
    WriteFile { path: String, description: String },
}

/// Shell-style quoting for display only — never used to build a shell
/// string that's actually executed. Arguments always reach `Invocation` as
/// a `Vec<String>`, never joined into one.
fn quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=,@".contains(c))
    {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

fn format_invocation(inv: &Invocation) -> String {
    let mut line = quote(&inv.program);
    for arg in &inv.args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    if let Some(stdin) = &inv.stdin {
        match stdin {
            Stdin::Plain(content) => {
                let _ = write!(line, "\n    <<< {content}");
            }
            Stdin::Redacted(_) => {
                line.push_str("\n    <<< [REDACTED]");
            }
        }
    }
    if let Some(note) = &inv.note {
        let _ = write!(line, "  # {note}");
    }
    line
}

pub fn format_action(action: &Action) -> String {
    match action {
        Action::Run(inv) => format_invocation(inv),
        Action::WriteFile { path, description } => format!("write {path}  # {description}"),
    }
}

/// Records the formatted command list instead of running anything.
#[derive(Debug, Default)]
pub struct DryRunRunner {
    pub lines: Vec<String>,
}

impl DryRunRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, action: &Action) {
        self.lines.push(format_action(action));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_arguments_with_spaces() {
        let inv = Invocation::new("useradd", ["-c", "Ada Lovelace", "ada"]);
        assert_eq!(format_invocation(&inv), "useradd -c 'Ada Lovelace' ada");
    }

    #[test]
    fn plain_arguments_are_not_quoted() {
        let inv = Invocation::new("bootctl", ["install"]);
        assert_eq!(format_invocation(&inv), "bootctl install");
    }

    #[test]
    fn redacted_stdin_never_prints_the_real_value() {
        let inv = Invocation::new("arch-chroot", ["/mnt/target", "chpasswd"])
            .with_stdin(Stdin::Redacted("ada:hunter2".to_string()));
        let formatted = format_invocation(&inv);
        assert!(!formatted.contains("hunter2"));
        assert!(formatted.contains("[REDACTED]"));
    }

    #[test]
    fn notes_are_appended_as_a_comment() {
        let inv = Invocation::new("genfstab", ["-U", "/mnt/target"])
            .with_note("appended to /mnt/target/etc/fstab");
        assert_eq!(
            format_invocation(&inv),
            "genfstab -U /mnt/target  # appended to /mnt/target/etc/fstab"
        );
    }
}
