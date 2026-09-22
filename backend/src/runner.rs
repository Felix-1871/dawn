// SPDX-License-Identifier: GPL-3.0-or-later

//! The single place every external command is described and run. Every
//! pipeline step builds [`Invocation`]s here instead of formatting a shell
//! string or shelling out for a file edit, so dry-run, real execution,
//! logging and password redaction all happen in one spot.
//!
//! Two [`Runner`] implementations: [`DryRunRunner`] just records what it
//! was asked to do (M0), and [`RealRunner`] actually does it (M1). Nothing
//! else in the backend calls `std::process::Command` or `std::fs::write`
//! directly.

use std::fmt::Write as _;
use std::io::Write as _;
use std::process::Stdio;

use thiserror::Error;

/// What, if anything, is written to an invocation's stdin. `chpasswd` and
/// similar tools take secrets this way instead of as an argv entry, which
/// would otherwise be visible to anyone who can run `ps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stdin {
    Plain(String),
    /// The real content is carried so `RealRunner` can still use it; only
    /// display (dry-run output, `Debug`-style logging) redacts it.
    Redacted(String),
}

impl Stdin {
    fn content(&self) -> &str {
        match self {
            Stdin::Plain(s) | Stdin::Redacted(s) => s,
        }
    }
}

/// What to do with a command's stdout after it exits successfully.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capture {
    /// Append stdout to this file — genfstab's output into
    /// `/etc/fstab`, for instance.
    AppendStdoutTo(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<Stdin>,
    pub capture: Option<Capture>,
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
            capture: None,
        }
    }

    pub fn with_stdin(mut self, stdin: Stdin) -> Self {
        self.stdin = Some(stdin);
        self
    }

    pub fn with_capture(mut self, capture: Capture) -> Self {
        self.capture = Some(capture);
        self
    }
}

/// One thing a pipeline step does. File edits go through `WriteFile` or
/// `UncommentLine` rather than a shelled-out `echo`/`sed`, since the real
/// runner can just do them itself in Rust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Run(Invocation),
    WriteFile {
        path: String,
        content: String,
    },
    /// Uncomments the first line under `path` that, once stripped of `#`
    /// and leading whitespace, equals `pattern` exactly. Used for
    /// `/etc/locale.gen`, which the base install already ships commented
    /// out in full.
    UncommentLine {
        path: String,
        pattern: String,
    },
    /// Removing a file is not the same as writing an empty one: missing
    /// already is success, since offline cleanup's targets (the live
    /// session's drop-ins) aren't guaranteed to exist on every image.
    RemoveFile {
        path: String,
    },
    /// Copies `source` to `destination` only if `destination` doesn't
    /// already exist. Used for the offline kernel copy: some archiso
    /// images keep the kernel out of the squashfs entirely, some don't.
    CopyIfMissing {
        source: String,
        destination: String,
    },
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

fn indented_block(content: &str) -> String {
    let mut out = String::new();
    for line in content.lines() {
        let _ = write!(out, "\n    <<< {line}");
    }
    out
}

fn format_invocation(inv: &Invocation) -> String {
    let mut line = quote(&inv.program);
    for arg in &inv.args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    match &inv.stdin {
        Some(Stdin::Plain(content)) => line.push_str(&indented_block(content)),
        Some(Stdin::Redacted(_)) => line.push_str("\n    <<< [REDACTED]"),
        None => {}
    }
    if let Some(Capture::AppendStdoutTo(path)) = &inv.capture {
        let _ = write!(line, "  # stdout appended to {path}");
    }
    line
}

pub fn format_action(action: &Action) -> String {
    match action {
        Action::Run(inv) => format_invocation(inv),
        Action::WriteFile { path, content } => {
            format!("write {path}{}", indented_block(content))
        }
        Action::UncommentLine { path, pattern } => {
            format!("uncomment {pattern:?} in {path}")
        }
        Action::RemoveFile { path } => format!("remove {path}"),
        Action::CopyIfMissing {
            source,
            destination,
        } => {
            format!("copy {source} to {destination} (if missing)")
        }
    }
}

#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("failed to run {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{program} exited with status {status}: {stderr}")]
    NonZeroExit {
        program: String,
        status: i32,
        stderr: String,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("no commented-out {pattern:?} line found in {path}")]
    PatternNotFound { path: String, pattern: String },
}

/// Runs (or records) [`Action`]s. Implemented by [`DryRunRunner`], which
/// only records what it was asked to do, and [`RealRunner`], which does it.
pub trait Runner {
    fn run(&mut self, action: &Action) -> Result<(), RunnerError>;
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
}

impl Runner for DryRunRunner {
    fn run(&mut self, action: &Action) -> Result<(), RunnerError> {
        self.lines.push(format_action(action));
        Ok(())
    }
}

/// Actually spawns processes and writes files. Only ever constructed after
/// the CLI's safety checks (`--target` matches the plan, disk isn't
/// mounted or the running system) have passed — see `main.rs`.
#[derive(Debug, Default)]
pub struct RealRunner;

impl RealRunner {
    pub fn new() -> Self {
        Self
    }
}

impl Runner for RealRunner {
    fn run(&mut self, action: &Action) -> Result<(), RunnerError> {
        match action {
            Action::Run(inv) => run_invocation(inv),
            Action::WriteFile { path, content } => write_file(path, content),
            Action::UncommentLine { path, pattern } => uncomment_line(path, pattern),
            Action::RemoveFile { path } => remove_file(path),
            Action::CopyIfMissing {
                source,
                destination,
            } => copy_if_missing(source, destination),
        }
    }
}

fn run_invocation(inv: &Invocation) -> Result<(), RunnerError> {
    let spawn_err = |source| RunnerError::Spawn {
        program: inv.program.clone(),
        source,
    };

    let mut command = std::process::Command::new(&inv.program);
    command.args(&inv.args);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    if inv.stdin.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }

    let mut child = command.spawn().map_err(spawn_err)?;

    if let Some(stdin) = &inv.stdin {
        child
            .stdin
            .take()
            .expect("stdin was requested as piped")
            .write_all(stdin.content().as_bytes())
            .map_err(spawn_err)?;
    }

    let output = child.wait_with_output().map_err(spawn_err)?;

    if !output.status.success() {
        return Err(RunnerError::NonZeroExit {
            program: inv.program.clone(),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    if let Some(Capture::AppendStdoutTo(path)) = &inv.capture {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|source| RunnerError::Write {
                path: path.clone(),
                source,
            })?;
        file.write_all(&output.stdout)
            .map_err(|source| RunnerError::Write {
                path: path.clone(),
                source,
            })?;
    }

    Ok(())
}

fn write_file(path: &str, content: &str) -> Result<(), RunnerError> {
    std::fs::write(path, content).map_err(|source| RunnerError::Write {
        path: path.to_string(),
        source,
    })
}

fn uncomment_line(path: &str, pattern: &str) -> Result<(), RunnerError> {
    let write_err = |source| RunnerError::Write {
        path: path.to_string(),
        source,
    };

    let original = std::fs::read_to_string(path).map_err(write_err)?;
    let mut found = false;
    let mut updated_lines = Vec::new();
    for line in original.lines() {
        let trimmed = line.trim_start();
        if !found && trimmed.starts_with('#') && trimmed.trim_start_matches('#').trim() == pattern {
            found = true;
            updated_lines.push(trimmed.trim_start_matches('#').trim().to_string());
        } else {
            updated_lines.push(line.to_string());
        }
    }

    if !found {
        return Err(RunnerError::PatternNotFound {
            path: path.to_string(),
            pattern: pattern.to_string(),
        });
    }

    let mut updated = updated_lines.join("\n");
    if original.ends_with('\n') {
        updated.push('\n');
    }
    std::fs::write(path, updated).map_err(write_err)
}

fn remove_file(path: &str) -> Result<(), RunnerError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(RunnerError::Write {
            path: path.to_string(),
            source,
        }),
    }
}

fn copy_if_missing(source: &str, destination: &str) -> Result<(), RunnerError> {
    if std::path::Path::new(destination).exists() {
        return Ok(());
    }
    std::fs::copy(source, destination)
        .map(|_| ())
        .map_err(|source_err| RunnerError::Write {
            path: destination.to_string(),
            source: source_err,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, uniquely-named temp directory. `cargo test` runs test
    /// functions as threads within a single process, so `process::id()`
    /// alone collides between tests; an atomic counter makes every call
    /// unique regardless of thread or process.
    fn unique_temp_dir() -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("dawn-runner-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

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
    fn capture_is_shown_as_a_comment() {
        let inv = Invocation::new("genfstab", ["-U", "/mnt/target"])
            .with_capture(Capture::AppendStdoutTo("/mnt/target/etc/fstab".to_string()));
        assert_eq!(
            format_invocation(&inv),
            "genfstab -U /mnt/target  # stdout appended to /mnt/target/etc/fstab"
        );
    }

    #[test]
    fn real_runner_runs_a_command() {
        let mut runner = RealRunner::new();
        runner
            .run(&Action::Run(Invocation::new("true", Vec::<String>::new())))
            .unwrap();
    }

    #[test]
    fn real_runner_surfaces_a_non_zero_exit() {
        let mut runner = RealRunner::new();
        let err = runner
            .run(&Action::Run(Invocation::new("false", Vec::<String>::new())))
            .unwrap_err();
        assert!(matches!(err, RunnerError::NonZeroExit { .. }));
    }

    #[test]
    fn real_runner_surfaces_a_missing_program() {
        let mut runner = RealRunner::new();
        let err = runner
            .run(&Action::Run(Invocation::new(
                "dawn-backend-test-nonexistent-program",
                Vec::<String>::new(),
            )))
            .unwrap_err();
        assert!(matches!(err, RunnerError::Spawn { .. }));
    }

    #[test]
    fn real_runner_pipes_stdin_to_the_child() {
        let mut runner = RealRunner::new();
        let dir = unique_temp_dir();
        let out = dir.join("stdin-echo.txt");

        runner
            .run(&Action::Run(
                Invocation::new("tee", [out.to_str().unwrap()])
                    .with_stdin(Stdin::Plain("hello from stdin".to_string())),
            ))
            .unwrap();

        assert_eq!(std::fs::read_to_string(&out).unwrap(), "hello from stdin");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn real_runner_writes_a_file() {
        let dir = unique_temp_dir();
        let path = dir.join("hostname").to_str().unwrap().to_string();

        let mut runner = RealRunner::new();
        runner
            .run(&Action::WriteFile {
                path: path.clone(),
                content: "ada-laptop\n".to_string(),
            })
            .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ada-laptop\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn real_runner_uncomments_the_matching_line_only() {
        let dir = unique_temp_dir();
        let path = dir.join("locale.gen").to_str().unwrap().to_string();
        std::fs::write(
            &path,
            "#de_DE.UTF-8 UTF-8\n#en_US.UTF-8 UTF-8\n#fr_FR.UTF-8 UTF-8\n",
        )
        .unwrap();

        let mut runner = RealRunner::new();
        runner
            .run(&Action::UncommentLine {
                path: path.clone(),
                pattern: "en_US.UTF-8 UTF-8".to_string(),
            })
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "#de_DE.UTF-8 UTF-8\nen_US.UTF-8 UTF-8\n#fr_FR.UTF-8 UTF-8\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn real_runner_reports_a_missing_pattern() {
        let dir = unique_temp_dir();
        let path = dir.join("locale.gen").to_str().unwrap().to_string();
        std::fs::write(&path, "#de_DE.UTF-8 UTF-8\n").unwrap();

        let mut runner = RealRunner::new();
        let err = runner
            .run(&Action::UncommentLine {
                path: path.clone(),
                pattern: "en_US.UTF-8 UTF-8".to_string(),
            })
            .unwrap_err();

        assert!(matches!(err, RunnerError::PatternNotFound { .. }));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn real_runner_removes_an_existing_file() {
        let dir = unique_temp_dir();
        let path = dir.join("autologin.conf").to_str().unwrap().to_string();
        std::fs::write(&path, "leftover").unwrap();

        let mut runner = RealRunner::new();
        runner
            .run(&Action::RemoveFile { path: path.clone() })
            .unwrap();

        assert!(!std::path::Path::new(&path).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn real_runner_removing_a_missing_file_is_not_an_error() {
        let mut runner = RealRunner::new();
        runner
            .run(&Action::RemoveFile {
                path: "/nonexistent/dawn-runner-test/file".to_string(),
            })
            .unwrap();
    }

    #[test]
    fn real_runner_copies_when_the_destination_is_missing() {
        let dir = unique_temp_dir();
        let source = dir.join("vmlinuz-linux");
        let destination = dir.join("boot-vmlinuz-linux");
        std::fs::write(&source, "kernel bytes").unwrap();

        let mut runner = RealRunner::new();
        runner
            .run(&Action::CopyIfMissing {
                source: source.to_str().unwrap().to_string(),
                destination: destination.to_str().unwrap().to_string(),
            })
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "kernel bytes"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn real_runner_skips_the_copy_when_the_destination_exists() {
        let dir = unique_temp_dir();
        let source = dir.join("vmlinuz-linux");
        let destination = dir.join("boot-vmlinuz-linux");
        std::fs::write(&source, "new kernel bytes").unwrap();
        std::fs::write(&destination, "already installed").unwrap();

        let mut runner = RealRunner::new();
        runner
            .run(&Action::CopyIfMissing {
                source: source.to_str().unwrap().to_string(),
                destination: destination.to_str().unwrap().to_string(),
            })
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "already installed"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn real_runner_captures_stdout_to_a_file() {
        let dir = unique_temp_dir();
        let out = dir.join("fstab").to_str().unwrap().to_string();
        std::fs::write(&out, "# existing line\n").unwrap();

        let mut runner = RealRunner::new();
        runner
            .run(&Action::Run(
                Invocation::new("echo", ["new line"])
                    .with_capture(Capture::AppendStdoutTo(out.clone())),
            ))
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "# existing line\nnew line\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
