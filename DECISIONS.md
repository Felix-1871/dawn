# Decisions

Log of choices made during implementation where SPEC.md didn't spell out the
answer, or where a concrete detail had to be picked to make code compile.
Newest first.

## M0

- **Repo layout is the repo root, not a nested `dawn/` folder.** SPEC.md's
  "Repo layout" tree is headed `dawn/`, but CLAUDE.md's Commands section
  gives paths like `tests/plans/erase-online.json` and
  `cargo run -p backend -- --dry-run ...`, which only work if the workspace
  `Cargo.toml` is at the repo root. Read `dawn/` as naming the project, not
  an extra directory level.

- **M0's `--dry-run` builds the real command list, not a stub.** The
  milestone table only lists "backend CLI with `--dry-run`" under M0's
  scope, and puts "pacstrap and squashfs paths, locale, user, UKIs,
  systemd-boot" under M1. But M0's Done-when check is
  "`dawn-backend --dry-run plan.json` prints the full command list", and
  the Testing table's dry-run snapshot row runs "every commit", not just
  from M1. Read together: M0 delivers the pipeline's command-list
  construction (the `arch` adapter included) entirely in argument-list
  form; M1 adds a runner that actually spawns those commands and verifies
  the result boots in a VM. No real execution path exists yet — the CLI
  refuses to run without `--dry-run`.

- **File edits go through a `WriteFile` action, not a shelled-out `echo`
  or `sed`.** Steps that just edit a config file (`locale.gen`,
  `vconsole.conf`, `/etc/hostname`, the kernel cmdline, ...) are recorded
  as `Action::WriteFile { path, description }` rather than a fake shell
  command. This fits CLAUDE.md's "no shell strings" rule better than
  inventing a shell invocation for something Rust can just write directly,
  and is what SPEC.md's arch adapter table already says for locale/keymap:
  "Files are written directly, since `localectl` needs a running systemd."

- **The user's password reaches `chpasswd` via stdin, not argv**, tagged
  `Stdin::Redacted` so dry-run output (and any future logging) can never
  print it. This isn't spelled out in SPEC.md, but it follows directly
  from CLAUDE.md's "the user password never appears in logs, errors,
  snapshots or test fixtures" — an argv value shows up in `ps` and process
  logs in a way stdin doesn't.

- **Two placeholder values in the `arch` adapter aren't verified against a
  real LuminOS ISO yet**, since M0 has no VM to check them against:
  - the live user's name (`liveuser`) removed during offline cleanup
    (step 6);
  - the login manager service enabled in step 12 (`greetd`) — SPEC.md only
    says "the login manager luminos-desktop depends on" without naming it.

  Both are marked with a comment at their definition
  ([`backend/src/adapters/arch.rs`](backend/src/adapters/arch.rs)) and
  should be confirmed (and the golden files regenerated if they change)
  once M1's VM pipeline exists to test against.

- **`cargo-deny`'s dependency license set for v1**: `MIT`, `Apache-2.0`,
  `Unicode-3.0`, `Unlicense`, `GPL-3.0-or-later` — everything M0's
  dependency tree (serde, schemars, clap, thiserror, and their transitive
  deps) actually uses. `allow-wildcard-paths` doesn't cover the
  `plan`/`backend`/`frontend` path dependencies between each other because
  cargo-deny treats a crate without `publish = false` as publishable, so
  all three crates set `publish = false` (none of them are meant to reach
  crates.io).

## Open questions for the user

- Per CLAUDE.md: before M1 starts, we need to know whether VM tests run
  locally (requires `/dev/kvm`, `qemu-system-x86_64`, OVMF on this
  machine) or in CI.
