// SPDX-License-Identifier: GPL-3.0-or-later

//! The single place every external command is described and run. Every
//! pipeline step builds [`Invocation`]s here instead of formatting a shell
//! string or shelling out for a file edit, so dry-run, real execution,
//! logging and password redaction all happen in one spot.
//!
//! Two [`Runner`] implementations: [`DryRunRunner`] only reports what it
//! was asked to do, and [`RealRunner`] does it. Both report through a
//! [`Sink`] rather than printing, since in `--serve` mode stdout is the
//! socket to the GUI. [`Log`] is the sink every install writes through:
//! it keeps `/var/log/dawn.log` and the tail an error event carries.
//! Nothing else in the backend spawns a process or writes a file for the
//! install.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::{BufRead as _, BufReader, Read, Write as _};
use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::repos;

/// What, if anything, is written to an invocation's stdin. `chpasswd` and
/// similar tools take secrets this way instead of as an argv entry, which
/// would otherwise be visible to anyone who can run `ps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stdin {
    Plain(String),
    /// The real content is carried so `RealRunner` can still use it; only
    /// display (dry-run output, logging) redacts it.
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

/// How a command reports its own progress on stdout, if it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressFormat {
    /// pacman, through pacstrap, with its progress bars off because
    /// stdout isn't a terminal: a `Packages (N)` line, then one
    /// `installing <name>...` line per package.
    Pacman,
    /// One percentage per line, as `unsquashfs -percentage` prints.
    Percentage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<Stdin>,
    pub capture: Option<Capture>,
    pub progress: Option<ProgressFormat>,
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
            progress: None,
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

    pub fn with_progress(mut self, progress: ProgressFormat) -> Self {
        self.progress = Some(progress);
        self
    }
}

/// One thing a pipeline step does. File edits go through `WriteFile` and
/// friends rather than a shelled-out `echo`/`sed`, since the real runner
/// can just do them itself in Rust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Run(Invocation),
    /// Replaces `path` with `content`, creating any missing parent
    /// directories first: config drop-ins such as
    /// `/etc/skel/.config/hypr/keyboard.conf` go into directories that
    /// only exist if some package happened to create them.
    WriteFile {
        path: String,
        content: String,
    },
    /// Uncomments the first line under `path` that, once stripped of `#`
    /// and leading whitespace, equals `pattern` exactly. Used for
    /// `/etc/locale.gen`, which the base install already ships commented
    /// out in full. If the line is already there uncommented (a live
    /// image an offline install copies may have its locale enabled), the
    /// file is left as it is.
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
    /// Copies the kernel a package installed as
    /// `<root>/usr/lib/modules/<version>/vmlinuz` to `destination`,
    /// unless `destination` already exists. The version isn't known until
    /// an offline install has unpacked the image, so the runner looks for
    /// the one modules directory whose `pkgbase` file names `pkgbase`.
    InstallKernelFromModules {
        root: String,
        pkgbase: String,
        destination: String,
    },
    /// Makes `<root><link>` a symlink to `points_to`, a path as the
    /// system under `root` sees it, replacing whatever was there. Refuses
    /// to leave a dangling link: `<root><points_to>` must exist.
    Symlink {
        root: String,
        link: String,
        points_to: String,
    },
    /// Sets `[table]` in the TOML file at `path` to exactly these string
    /// entries, replacing any table of that name and leaving the rest of
    /// the file alone. Used for greetd's `config.toml`, which has no
    /// drop-in directory.
    SetTomlTable {
        path: String,
        table: String,
        entries: Vec<(String, String)>,
    },
    /// Checks every repository in `pacman_conf` has a mirror serving its
    /// database, so an online install fails in step 1 instead of halfway
    /// through pacstrap.
    CheckReposReachable {
        pacman_conf: String,
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
        Action::InstallKernelFromModules {
            root,
            pkgbase,
            destination,
        } => format!(
            "copy the {pkgbase} kernel from {root}/usr/lib/modules/*/vmlinuz to {destination} (if missing)"
        ),
        Action::Symlink {
            root,
            link,
            points_to,
        } => format!("symlink {root}{link} -> {points_to}"),
        Action::SetTomlTable {
            path,
            table,
            entries,
        } => {
            let body: String = entries
                .iter()
                .map(|(key, value)| format!("{key} = {value:?}\n"))
                .collect();
            format!("set [{table}] in {path}{}", indented_block(&body))
        }
        Action::CheckReposReachable { pacman_conf } => {
            format!("check the repositories in {pacman_conf} are reachable")
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
    #[error("no {pattern:?} line, commented out or not, found in {path}")]
    PatternNotFound { path: String, pattern: String },
    #[error("found no {pkgbase} kernel under {modules_dir}")]
    KernelNotFound {
        modules_dir: String,
        pkgbase: String,
    },
    #[error("found more than one {pkgbase} kernel under {modules_dir}")]
    AmbiguousKernel {
        modules_dir: String,
        pkgbase: String,
    },
    #[error("{0} doesn't exist")]
    MissingLinkTarget(String),
    #[error("could not edit {path}: {message}")]
    Toml { path: String, message: String },
    #[error("no mirror answered for: {}", .0.join(", "))]
    ReposUnreachable(Vec<String>),
    #[error("cancelled")]
    Cancelled,
}

/// Where a runner reports what it's doing. The install's [`Log`] is one;
/// tests use their own.
pub trait Sink {
    /// One line: an action about to run, or a line a command printed.
    fn line(&mut self, line: &str);
    /// How far along the current action is, from 0.0 to 1.0, for the
    /// commands that report it (see [`ProgressFormat`]).
    fn progress(&mut self, fraction: f32);
}

/// Runs (or only reports) [`Action`]s.
pub trait Runner {
    fn run(&mut self, action: &Action, sink: &mut dyn Sink) -> Result<(), RunnerError>;
}

/// Reports every action instead of running it.
#[derive(Debug, Default)]
pub struct DryRunRunner;

impl Runner for DryRunRunner {
    fn run(&mut self, action: &Action, sink: &mut dyn Sink) -> Result<(), RunnerError> {
        sink.line(&format_action(action));
        Ok(())
    }
}

/// Set from another thread to stop the install: the running command's
/// whole process group is terminated and the runner returns
/// [`RunnerError::Cancelled`].
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_requested(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Actually spawns processes and writes files. Only ever constructed after
/// the safety checks (`--target` matches the plan, the disk isn't mounted,
/// the running system or the boot medium) have passed — see `main.rs`.
#[derive(Debug, Default)]
pub struct RealRunner {
    cancel: Cancel,
}

impl RealRunner {
    pub fn new(cancel: Cancel) -> Self {
        Self { cancel }
    }
}

impl Runner for RealRunner {
    fn run(&mut self, action: &Action, sink: &mut dyn Sink) -> Result<(), RunnerError> {
        if self.cancel.is_requested() {
            return Err(RunnerError::Cancelled);
        }
        sink.line(&format_action(action));
        match action {
            Action::Run(inv) => run_invocation(inv, sink, &self.cancel),
            Action::WriteFile { path, content } => write_file(path, content),
            Action::UncommentLine { path, pattern } => uncomment_line(path, pattern),
            Action::RemoveFile { path } => remove_file(path),
            Action::InstallKernelFromModules {
                root,
                pkgbase,
                destination,
            } => install_kernel_from_modules(root, pkgbase, destination),
            Action::Symlink {
                root,
                link,
                points_to,
            } => symlink(root, link, points_to),
            Action::SetTomlTable {
                path,
                table,
                entries,
            } => set_toml_table(path, table, entries),
            Action::CheckReposReachable { pacman_conf } => {
                check_repos_reachable(pacman_conf, sink, &self.cancel)
            }
        }
    }
}

/// Runs `inv` to completion and returns its stdout, for the backend's own
/// probes (`lsblk`, `curl`) rather than install steps: nothing is logged,
/// and a non-zero exit is an error.
pub fn capture_stdout(inv: &Invocation) -> Result<String, RunnerError> {
    let output = Command::new(&inv.program)
        .args(&inv.args)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| RunnerError::Spawn {
            program: inv.program.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(RunnerError::NonZeroExit {
            program: inv.program.clone(),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

enum Output {
    Stdout(String),
    Stderr(String),
    Closed,
}

fn spawn_reader(stream: impl Read + Send + 'static, tx: mpsc::Sender<Output>, stdout: bool) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf)
                        .trim_end_matches(['\n', '\r'])
                        .to_string();
                    let message = if stdout {
                        Output::Stdout(line)
                    } else {
                        Output::Stderr(line)
                    };
                    if tx.send(message).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = tx.send(Output::Closed);
    });
}

/// How long to keep reading output after the command itself has exited.
/// A daemon it started (pacstrap's gpg-agent, say) could otherwise hold
/// the pipes open indefinitely.
const OUTPUT_GRACE: Duration = Duration::from_secs(2);

/// How long a cancelled command gets to exit after SIGTERM before SIGKILL.
const TERM_GRACE: Duration = Duration::from_secs(5);

fn run_invocation(
    inv: &Invocation,
    sink: &mut dyn Sink,
    cancel: &Cancel,
) -> Result<(), RunnerError> {
    let spawn_err = |source| RunnerError::Spawn {
        program: inv.program.clone(),
        source,
    };

    let mut command = Command::new(&inv.program);
    command
        .args(&inv.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if inv.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        // Its own process group, so cancelling stops everything it
        // started, not just the command itself.
        .process_group(0);
    let mut child = command.spawn().map_err(spawn_err)?;

    if let (Some(stdin), Some(mut pipe)) = (&inv.stdin, child.stdin.take()) {
        pipe.write_all(stdin.content().as_bytes())
            .map_err(spawn_err)?;
        // Dropping the pipe closes it, so the command sees end of input.
    }

    let (tx, rx) = mpsc::channel();
    let mut open_streams = 0;
    if let Some(stdout) = child.stdout.take() {
        spawn_reader(stdout, tx.clone(), true);
        open_streams += 1;
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(stderr, tx, false);
        open_streams += 1;
    }

    let mut progress = inv.progress.map(ProgressParser::new);
    let mut stderr_tail: VecDeque<String> = VecDeque::new();
    let mut captured = String::new();
    let mut exited: Option<(ExitStatus, Instant)> = None;

    while open_streams > 0 {
        if cancel.is_requested() {
            stop_process_group(&mut child);
            return Err(RunnerError::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Output::Stdout(line)) => {
                if let Some(fraction) = progress.as_mut().and_then(|p| p.feed(&line)) {
                    sink.progress(fraction);
                }
                if inv.capture.is_some() {
                    captured.push_str(&line);
                    captured.push('\n');
                } else {
                    sink.line(&line);
                }
            }
            Ok(Output::Stderr(line)) => {
                sink.line(&line);
                if stderr_tail.len() == 20 {
                    stderr_tail.pop_front();
                }
                stderr_tail.push_back(line);
            }
            Ok(Output::Closed) => open_streams -= 1,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if exited.is_none()
                    && let Ok(Some(status)) = child.try_wait()
                {
                    exited = Some((status, Instant::now()));
                }
                if exited.is_some_and(|(_, at)| at.elapsed() > OUTPUT_GRACE) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let status = match exited {
        Some((status, _)) => status,
        None => child.wait().map_err(spawn_err)?,
    };
    if !status.success() {
        return Err(RunnerError::NonZeroExit {
            program: inv.program.clone(),
            status: status.code().unwrap_or(-1),
            stderr: Vec::from(stderr_tail).join("\n"),
        });
    }

    if let Some(Capture::AppendStdoutTo(path)) = &inv.capture {
        let write_err = |source| RunnerError::Write {
            path: path.clone(),
            source,
        };
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(write_err)?;
        file.write_all(captured.as_bytes()).map_err(write_err)?;
    }

    Ok(())
}

/// SIGTERM to the command's process group, then SIGKILL if it hasn't
/// exited after [`TERM_GRACE`].
fn stop_process_group(child: &mut Child) {
    let group = rustix::process::Pid::from_child(child);
    let _ = rustix::process::kill_process_group(group, rustix::process::Signal::TERM);
    let deadline = Instant::now() + TERM_GRACE;
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    let _ = child.wait();
}

/// Turns a command's stdout lines into a 0.0–1.0 fraction.
struct ProgressParser {
    format: ProgressFormat,
    total: Option<u32>,
    done: u32,
}

impl ProgressParser {
    fn new(format: ProgressFormat) -> Self {
        Self {
            format,
            total: None,
            done: 0,
        }
    }

    fn feed(&mut self, line: &str) -> Option<f32> {
        match self.format {
            ProgressFormat::Pacman => {
                if let Some(rest) = line.strip_prefix("Packages (") {
                    self.total = rest.split(')').next()?.trim().parse().ok();
                    return None;
                }
                let is_package_line = ["installing ", "upgrading ", "reinstalling "]
                    .iter()
                    .any(|prefix| line.starts_with(prefix))
                    && line.ends_with("...");
                if !is_package_line {
                    return None;
                }
                self.done += 1;
                let total = self.total.filter(|&t| t > 0)?;
                Some((self.done as f32 / total as f32).min(1.0))
            }
            ProgressFormat::Percentage => {
                let percent: f32 = line.trim().parse().ok()?;
                (0.0..=100.0).contains(&percent).then_some(percent / 100.0)
            }
        }
    }
}

fn write_file(path: &str, content: &str) -> Result<(), RunnerError> {
    let write_err = |source| RunnerError::Write {
        path: path.to_string(),
        source,
    };

    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent).map_err(write_err)?;
    }
    std::fs::write(path, content).map_err(write_err)
}

fn uncomment_line(path: &str, pattern: &str) -> Result<(), RunnerError> {
    let write_err = |source| RunnerError::Write {
        path: path.to_string(),
        source,
    };

    let original = std::fs::read_to_string(path).map_err(write_err)?;
    if original.lines().any(|line| line.trim() == pattern) {
        return Ok(());
    }

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

fn install_kernel_from_modules(
    root: &str,
    pkgbase: &str,
    destination: &str,
) -> Result<(), RunnerError> {
    if std::path::Path::new(destination).exists() {
        return Ok(());
    }
    let modules_dir = format!("{root}/usr/lib/modules");
    let not_found = || RunnerError::KernelNotFound {
        modules_dir: modules_dir.clone(),
        pkgbase: pkgbase.to_string(),
    };
    let entries = std::fs::read_dir(&modules_dir).map_err(|_| not_found())?;
    let mut kernels = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let names_pkgbase = std::fs::read_to_string(dir.join("pkgbase"))
            .is_ok_and(|content| content.trim() == pkgbase);
        let vmlinuz = dir.join("vmlinuz");
        if names_pkgbase && vmlinuz.is_file() {
            kernels.push(vmlinuz);
        }
    }
    let source = match kernels.as_slice() {
        [] => return Err(not_found()),
        [only] => only,
        _ => {
            return Err(RunnerError::AmbiguousKernel {
                modules_dir,
                pkgbase: pkgbase.to_string(),
            });
        }
    };
    std::fs::copy(source, destination)
        .map(|_| ())
        .map_err(|source| RunnerError::Write {
            path: destination.to_string(),
            source,
        })
}

fn symlink(root: &str, link: &str, points_to: &str) -> Result<(), RunnerError> {
    let target_in_root = format!("{root}{points_to}");
    if !std::path::Path::new(&target_in_root).exists() {
        return Err(RunnerError::MissingLinkTarget(target_in_root));
    }
    let link_path = format!("{root}{link}");
    let write_err = |source| RunnerError::Write {
        path: link_path.clone(),
        source,
    };
    match std::fs::remove_file(&link_path) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(write_err(source)),
    }
    std::os::unix::fs::symlink(points_to, &link_path).map_err(write_err)
}

fn set_toml_table(
    path: &str,
    table: &str,
    entries: &[(String, String)],
) -> Result<(), RunnerError> {
    let original = std::fs::read_to_string(path).map_err(|source| RunnerError::Write {
        path: path.to_string(),
        source,
    })?;
    let mut document: toml_edit::DocumentMut =
        original
            .parse()
            .map_err(|err: toml_edit::TomlError| RunnerError::Toml {
                path: path.to_string(),
                message: err.to_string(),
            })?;
    let mut new_table = toml_edit::Table::new();
    for (key, value) in entries {
        new_table.insert(key, toml_edit::value(value.as_str()));
    }
    document.insert(table, toml_edit::Item::Table(new_table));
    std::fs::write(path, document.to_string()).map_err(|source| RunnerError::Write {
        path: path.to_string(),
        source,
    })
}

fn check_repos_reachable(
    pacman_conf: &str,
    sink: &mut dyn Sink,
    cancel: &Cancel,
) -> Result<(), RunnerError> {
    let conf = std::fs::read_to_string(pacman_conf).map_err(|source| RunnerError::Write {
        path: pacman_conf.to_string(),
        source,
    })?;
    let repos = repos::parse_pacman_conf(&conf, &|path| std::fs::read_to_string(path).ok());
    let mut unreachable = Vec::new();
    for repo in &repos {
        let mut reachable = false;
        for url in repo.database_urls() {
            let curl = head_request(&url);
            sink.line(&format_invocation(&curl));
            match run_invocation(&curl, sink, cancel) {
                Ok(()) => {
                    reachable = true;
                    break;
                }
                Err(RunnerError::Cancelled) => return Err(RunnerError::Cancelled),
                Err(_) => {}
            }
        }
        if !reachable {
            unreachable.push(repo.name.clone());
        }
    }
    if unreachable.is_empty() {
        Ok(())
    } else {
        Err(RunnerError::ReposUnreachable(unreachable))
    }
}

/// A quick `HEAD` request that fails on any HTTP error.
pub fn head_request(url: &str) -> Invocation {
    Invocation::new(
        "curl",
        [
            "--silent",
            "--show-error",
            "--fail",
            "--head",
            "--max-time",
            "10",
            "--output",
            "/dev/null",
            url,
        ],
    )
}

/// How many of the last log lines an error event carries (SPEC.md "On
/// failure").
pub const LOG_TAIL_LINES: usize = 50;

/// The sink an install writes through: every line goes to the install
/// log file (if any), is remembered for the error event's tail, and is
/// passed on to whoever's watching (stdout for the CLI, the socket for
/// the GUI). Any secret registered with [`Log::redact`] is replaced in
/// every line, as a last line of defence behind [`Stdin::Redacted`].
pub struct Log {
    file: Option<std::fs::File>,
    tail: VecDeque<String>,
    secrets: Vec<String>,
    on_line: Box<dyn FnMut(&str) + Send>,
}

impl Log {
    pub fn new(on_line: impl FnMut(&str) + Send + 'static) -> Self {
        Self {
            file: None,
            tail: VecDeque::new(),
            secrets: Vec::new(),
            on_line: Box::new(on_line),
        }
    }

    /// Like [`Log::new`], but every line also goes to `path`, which
    /// starts out empty. A real install logs to `/var/log/dawn.log`
    /// (SPEC.md "On failure"), and step 12 copies that file into the
    /// target.
    pub fn with_file(
        path: &str,
        on_line: impl FnMut(&str) + Send + 'static,
    ) -> Result<Self, RunnerError> {
        let file = std::fs::File::create(path).map_err(|source| RunnerError::Write {
            path: path.to_string(),
            source,
        })?;
        let mut log = Self::new(on_line);
        log.file = Some(file);
        Ok(log)
    }

    pub fn redact(&mut self, secret: &str) {
        if !secret.is_empty() {
            self.secrets.push(secret.to_string());
        }
    }

    /// The last [`LOG_TAIL_LINES`] lines, oldest first.
    pub fn tail(&self) -> Vec<String> {
        self.tail.iter().cloned().collect()
    }
}

impl Sink for Log {
    fn line(&mut self, line: &str) {
        let mut line = line.to_string();
        for secret in &self.secrets {
            line = line.replace(secret.as_str(), "[REDACTED]");
        }
        // A lost log line isn't worth failing an install over.
        if let Some(file) = &mut self.file {
            let _ = writeln!(file, "{line}");
        }
        if self.tail.len() == LOG_TAIL_LINES {
            self.tail.pop_front();
        }
        self.tail.push_back(line.clone());
        (self.on_line)(&line);
    }

    /// The log records lines only; the install turns progress into its
    /// own events (see `install.rs`).
    fn progress(&mut self, _fraction: f32) {}
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

    /// Records what a runner reported.
    #[derive(Default)]
    struct Recorder {
        lines: Vec<String>,
        progress: Vec<f32>,
    }

    impl Sink for Recorder {
        fn line(&mut self, line: &str) {
            self.lines.push(line.to_string());
        }
        fn progress(&mut self, fraction: f32) {
            self.progress.push(fraction);
        }
    }

    fn run(action: &Action) -> (Result<(), RunnerError>, Recorder) {
        let mut recorder = Recorder::default();
        let result = RealRunner::new(Cancel::new()).run(action, &mut recorder);
        (result, recorder)
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
    fn dry_run_reports_the_action_and_runs_nothing() {
        let mut recorder = Recorder::default();
        DryRunRunner
            .run(
                &Action::Run(Invocation::new("false", Vec::<String>::new())),
                &mut recorder,
            )
            .unwrap();
        assert_eq!(recorder.lines, ["false"]);
    }

    #[test]
    fn real_runner_runs_a_command_and_streams_its_output() {
        let (result, recorder) = run(&Action::Run(Invocation::new(
            "sh",
            ["-c", "echo out; echo err >&2"],
        )));
        result.unwrap();
        assert_eq!(recorder.lines[0], "sh -c 'echo out; echo err >&2'");
        assert!(recorder.lines.contains(&"out".to_string()));
        assert!(recorder.lines.contains(&"err".to_string()));
    }

    #[test]
    fn a_non_zero_exit_carries_the_stderr_tail() {
        let (result, _) = run(&Action::Run(Invocation::new(
            "sh",
            ["-c", "echo first >&2; echo boom >&2; exit 3"],
        )));
        match result.unwrap_err() {
            RunnerError::NonZeroExit { status, stderr, .. } => {
                assert_eq!(status, 3);
                assert_eq!(stderr, "first\nboom");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn real_runner_surfaces_a_missing_program() {
        let (result, _) = run(&Action::Run(Invocation::new(
            "dawn-backend-test-nonexistent-program",
            Vec::<String>::new(),
        )));
        assert!(matches!(result.unwrap_err(), RunnerError::Spawn { .. }));
    }

    #[test]
    fn real_runner_pipes_stdin_to_the_child() {
        let dir = unique_temp_dir();
        let out = dir.join("stdin-echo.txt");
        let (result, _) = run(&Action::Run(
            Invocation::new("tee", [out.to_str().unwrap()])
                .with_stdin(Stdin::Plain("hello from stdin".to_string())),
        ));
        result.unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "hello from stdin");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_command_that_leaves_a_daemon_holding_its_output_still_returns() {
        // The background sleep inherits stdout and keeps it open well past
        // the shell's own exit, the way pacstrap's gpg-agent can.
        let started = Instant::now();
        let (result, _) = run(&Action::Run(Invocation::new(
            "sh",
            ["-c", "sleep 30 & echo started"],
        )));
        result.unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn cancelling_stops_the_whole_process_group() {
        let dir = unique_temp_dir();
        let marker = dir.join("still-running");
        let cancel = Cancel::new();
        let canceller = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            canceller.request();
        });

        let started = Instant::now();
        let script = format!("sh -c 'sleep 1; touch {}' & wait", marker.display());
        let mut recorder = Recorder::default();
        let result = RealRunner::new(cancel).run(
            &Action::Run(Invocation::new("sh", ["-c", script.as_str()])),
            &mut recorder,
        );

        assert!(matches!(result.unwrap_err(), RunnerError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(1));
        // The grandchild was in the same group, so it never got to run.
        std::thread::sleep(Duration::from_millis(1500));
        assert!(!marker.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pacman_output_becomes_progress() {
        let (result, recorder) = run(&Action::Run(
            Invocation::new(
                "printf",
                ["Packages (4) a b c d\ninstalling a...\ninstalling b...\nupgrading c...\n"],
            )
            .with_progress(ProgressFormat::Pacman),
        ));
        result.unwrap();
        assert_eq!(recorder.progress, [0.25, 0.5, 0.75]);
    }

    #[test]
    fn percentage_output_becomes_progress() {
        let (result, recorder) = run(&Action::Run(
            Invocation::new("printf", ["0\n50\nnot a number\n100\n"])
                .with_progress(ProgressFormat::Percentage),
        ));
        result.unwrap();
        assert_eq!(recorder.progress, [0.0, 0.5, 1.0]);
    }

    #[test]
    fn real_runner_writes_a_file() {
        let dir = unique_temp_dir();
        let path = dir.join("hostname").to_str().unwrap().to_string();
        let (result, _) = run(&Action::WriteFile {
            path: path.clone(),
            content: "ada-laptop\n".to_string(),
        });
        result.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ada-laptop\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn real_runner_creates_missing_parent_directories() {
        let dir = unique_temp_dir();
        let path = dir
            .join("etc/skel/.config/hypr/keyboard.conf")
            .to_str()
            .unwrap()
            .to_string();
        let (result, _) = run(&Action::WriteFile {
            path: path.clone(),
            content: "input {\n}\n".to_string(),
        });
        result.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "input {\n}\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_log_writes_its_file_keeps_a_tail_and_redacts_secrets() {
        let dir = unique_temp_dir();
        let log_path = dir.join("dawn.log").to_str().unwrap().to_string();
        std::fs::write(&log_path, "a previous install's log\n").unwrap();

        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_by_log = seen.clone();
        let mut log = Log::with_file(&log_path, move |line| {
            seen_by_log.lock().unwrap().push(line.to_string());
        })
        .unwrap();
        log.redact("hunter2");
        for n in 0..60 {
            log.line(&format!("line {n}"));
        }
        log.line("chpasswd said hunter2");

        let file = std::fs::read_to_string(&log_path).unwrap();
        assert!(file.starts_with("line 0\n"));
        assert!(file.ends_with("chpasswd said [REDACTED]\n"));
        assert!(!file.contains("hunter2"));
        let tail = log.tail();
        assert_eq!(tail.len(), LOG_TAIL_LINES);
        assert_eq!(tail.last().unwrap(), "chpasswd said [REDACTED]");
        assert_eq!(seen.lock().unwrap().len(), 61);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_log_file_never_sees_redacted_stdin() {
        let dir = unique_temp_dir();
        let log_path = dir.join("dawn.log").to_str().unwrap().to_string();
        let mut log = Log::with_file(&log_path, |_| {}).unwrap();
        RealRunner::new(Cancel::new())
            .run(
                &Action::Run(
                    Invocation::new("true", Vec::<String>::new())
                        .with_stdin(Stdin::Redacted("ada:hunter2".to_string())),
                ),
                &mut log,
            )
            .unwrap();
        let written = std::fs::read_to_string(&log_path).unwrap();
        assert!(written.contains("[REDACTED]"));
        assert!(!written.contains("hunter2"));
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
        let (result, _) = run(&Action::UncommentLine {
            path: path.clone(),
            pattern: "en_US.UTF-8 UTF-8".to_string(),
        });
        result.unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "#de_DE.UTF-8 UTF-8\nen_US.UTF-8 UTF-8\n#fr_FR.UTF-8 UTF-8\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn real_runner_leaves_an_already_uncommented_line_alone() {
        let dir = unique_temp_dir();
        let path = dir.join("locale.gen").to_str().unwrap().to_string();
        let original = "#de_DE.UTF-8 UTF-8\nen_US.UTF-8 UTF-8\n#fr_FR.UTF-8 UTF-8\n";
        std::fs::write(&path, original).unwrap();
        let (result, _) = run(&Action::UncommentLine {
            path: path.clone(),
            pattern: "en_US.UTF-8 UTF-8".to_string(),
        });
        result.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn real_runner_reports_a_missing_pattern() {
        let dir = unique_temp_dir();
        let path = dir.join("locale.gen").to_str().unwrap().to_string();
        std::fs::write(&path, "#de_DE.UTF-8 UTF-8\n").unwrap();
        let (result, _) = run(&Action::UncommentLine {
            path: path.clone(),
            pattern: "en_US.UTF-8 UTF-8".to_string(),
        });
        assert!(matches!(
            result.unwrap_err(),
            RunnerError::PatternNotFound { .. }
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn real_runner_removes_an_existing_file() {
        let dir = unique_temp_dir();
        let path = dir.join("autologin.conf").to_str().unwrap().to_string();
        std::fs::write(&path, "leftover").unwrap();
        let (result, _) = run(&Action::RemoveFile { path: path.clone() });
        result.unwrap();
        assert!(!std::path::Path::new(&path).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn real_runner_removing_a_missing_file_is_not_an_error() {
        let (result, _) = run(&Action::RemoveFile {
            path: "/nonexistent/dawn-runner-test/file".to_string(),
        });
        result.unwrap();
    }

    fn fake_kernel(root: &std::path::Path, version: &str, pkgbase: &str) {
        let dir = root.join("usr/lib/modules").join(version);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pkgbase"), format!("{pkgbase}\n")).unwrap();
        std::fs::write(dir.join("vmlinuz"), format!("{pkgbase} {version}")).unwrap();
    }

    #[test]
    fn copies_the_kernel_the_image_carries() {
        let root = unique_temp_dir();
        fake_kernel(&root, "6.99.1-arch1-1", "linux");
        fake_kernel(&root, "6.98.0-lts", "linux-lts");
        std::fs::create_dir_all(root.join("boot")).unwrap();
        let destination = root.join("boot/vmlinuz-linux");

        let (result, _) = run(&Action::InstallKernelFromModules {
            root: root.to_str().unwrap().to_string(),
            pkgbase: "linux".to_string(),
            destination: destination.to_str().unwrap().to_string(),
        });
        result.unwrap();
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "linux 6.99.1-arch1-1"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn keeps_a_kernel_already_in_place() {
        let root = unique_temp_dir();
        fake_kernel(&root, "6.99.1-arch1-1", "linux");
        std::fs::create_dir_all(root.join("boot")).unwrap();
        let destination = root.join("boot/vmlinuz-linux");
        std::fs::write(&destination, "already there").unwrap();

        let (result, _) = run(&Action::InstallKernelFromModules {
            root: root.to_str().unwrap().to_string(),
            pkgbase: "linux".to_string(),
            destination: destination.to_str().unwrap().to_string(),
        });
        result.unwrap();
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "already there"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reports_an_image_without_the_kernel() {
        let root = unique_temp_dir();
        fake_kernel(&root, "6.98.0-lts", "linux-lts");
        let (result, _) = run(&Action::InstallKernelFromModules {
            root: root.to_str().unwrap().to_string(),
            pkgbase: "linux".to_string(),
            destination: root.join("vmlinuz-linux").to_str().unwrap().to_string(),
        });
        assert!(matches!(
            result.unwrap_err(),
            RunnerError::KernelNotFound { .. }
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn symlinks_the_timezone_replacing_an_old_link() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("usr/share/zoneinfo/Europe")).unwrap();
        std::fs::write(root.join("usr/share/zoneinfo/Europe/Berlin"), "TZif").unwrap();
        std::fs::create_dir_all(root.join("etc")).unwrap();
        std::os::unix::fs::symlink("/usr/share/zoneinfo/UTC", root.join("etc/localtime")).unwrap();

        let (result, _) = run(&Action::Symlink {
            root: root.to_str().unwrap().to_string(),
            link: "/etc/localtime".to_string(),
            points_to: "/usr/share/zoneinfo/Europe/Berlin".to_string(),
        });
        result.unwrap();
        assert_eq!(
            std::fs::read_link(root.join("etc/localtime")).unwrap(),
            std::path::Path::new("/usr/share/zoneinfo/Europe/Berlin")
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn refuses_a_dangling_symlink() {
        let root = unique_temp_dir();
        let (result, _) = run(&Action::Symlink {
            root: root.to_str().unwrap().to_string(),
            link: "/etc/localtime".to_string(),
            points_to: "/usr/share/zoneinfo/Mars/Olympus_Mons".to_string(),
        });
        assert!(matches!(
            result.unwrap_err(),
            RunnerError::MissingLinkTarget(_)
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn sets_a_toml_table_and_keeps_the_rest() {
        let dir = unique_temp_dir();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[terminal]\nvt = 1\n\n# The greeter.\n[default_session]\ncommand = \"luminos-greeter\"\nuser = \"greeter\"\n\n[initial_session]\ncommand = \"old\"\nuser = \"old\"\n",
        )
        .unwrap();

        let (result, _) = run(&Action::SetTomlTable {
            path: path.to_str().unwrap().to_string(),
            table: "initial_session".to_string(),
            entries: vec![
                ("command".to_string(), "Hyprland".to_string()),
                ("user".to_string(), "ada".to_string()),
            ],
        });
        result.unwrap();

        let updated = std::fs::read_to_string(&path).unwrap();
        let parsed: toml_edit::DocumentMut = updated.parse().unwrap();
        assert_eq!(
            parsed["default_session"]["command"].as_str(),
            Some("luminos-greeter")
        );
        assert_eq!(
            parsed["initial_session"]["command"].as_str(),
            Some("Hyprland")
        );
        assert_eq!(parsed["initial_session"]["user"].as_str(), Some("ada"));
        assert!(updated.contains("# The greeter."));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn real_runner_captures_stdout_to_a_file() {
        let dir = unique_temp_dir();
        let out = dir.join("fstab").to_str().unwrap().to_string();
        std::fs::write(&out, "# existing line\n").unwrap();
        let (result, recorder) = run(&Action::Run(
            Invocation::new("echo", ["new line"])
                .with_capture(Capture::AppendStdoutTo(out.clone())),
        ));
        result.unwrap();
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "# existing line\nnew line\n"
        );
        // Captured output goes to its file, not the log.
        assert!(!recorder.lines.contains(&"new line".to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unreachable_repositories_are_named() {
        let dir = unique_temp_dir();
        let conf = dir.join("pacman.conf");
        let db_dir = dir.join("mirror");
        std::fs::create_dir_all(&db_dir).unwrap();
        std::fs::write(db_dir.join("served.db"), "db").unwrap();
        std::fs::write(
            &conf,
            format!(
                "[options]\n[served]\nServer = file://{0}\n[missing]\nServer = file://{0}/nowhere\n",
                db_dir.display()
            ),
        )
        .unwrap();

        let (result, _) = run(&Action::CheckReposReachable {
            pacman_conf: conf.to_str().unwrap().to_string(),
        });
        match result.unwrap_err() {
            RunnerError::ReposUnreachable(repos) => assert_eq!(repos, ["missing"]),
            other => panic!("unexpected error: {other}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
