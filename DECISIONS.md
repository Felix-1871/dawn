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
  or `sed`.** Steps that just write a whole config file (`vconsole.conf`,
  `/etc/hostname`, the kernel cmdline, ...) are recorded as
  `Action::WriteFile { path, content }` rather than a fake shell command.
  This fits CLAUDE.md's "no shell strings" rule better than inventing a
  shell invocation for something Rust can just write directly, and is
  what SPEC.md's arch adapter table already says for locale/keymap:
  "Files are written directly, since `localectl` needs a running
  systemd." (M0 originally carried a human-readable `description` here
  instead of real `content`, since only dry-run existed yet; M1 needed
  the real bytes to actually write, and split `locale.gen`'s case out
  into its own `UncommentLine` action — see M1 below.)

- **The user's password reaches `chpasswd` via stdin, not argv**, tagged
  `Stdin::Redacted` so dry-run output (and any future logging) can never
  print it. This isn't spelled out in SPEC.md, but it follows directly
  from CLAUDE.md's "the user password never appears in logs, errors,
  snapshots or test fixtures" — an argv value shows up in `ps` and process
  logs in a way stdin doesn't.

- **The live ISO's user account is `luminos`.** Confirmed by the user;
  used in `offline_cleanup`'s `userdel -r` (step 6, offline installs
  only).

- **Login manager: `greetd`, with `regreet` (hosted under Hyprland) as
  the default greeter.** SPEC.md only says "the login manager
  `luminos-desktop` depends on" without naming it; asked the user, who
  wanted a recommendation on low weight and customizability. First pass
  was `greetd` + `tuigreet`, since `regreet`'s usual host compositor
  (`cage`) would have been an extra dependency. The user pointed out
  LuminOS already ships Hyprland as `luminos-desktop`'s compositor, so
  hosting `regreet` under a minimal Hyprland session instead of `cage`
  costs nothing extra — no new compositor package needed. That gets the
  CSS-themeable, brand-carrying graphical login without the weight
  tradeoff, so it's the v1 default rather than a later upgrade. `greetd`
  itself is still what's protocol-based and swappable; the greeter's own
  config ships as part of `luminos-desktop`, not Dawn. Dawn's adapter
  only enables the `greetd` service (step 12) and writes an override to
  `/etc/greetd/config.toml` when the plan asks for autologin — that part
  is unaffected by which greeter is configured underneath.

- **VM tests run in CI by default, with local QEMU as an accepted
  fallback** when something isn't possible in CI. This applies to M1's
  Done-when check (a plan installs into a QEMU VM disk and boots), not
  just M6's nightly end-to-end run. Confirmed by the user; M1 should
  still check for `/dev/kvm`, `qemu-system-x86_64` and OVMF per
  CLAUDE.md's "VM and test environment" section before assuming either
  path works.

- **`cargo-deny`'s dependency license set for v1**: `MIT`, `Apache-2.0`,
  `Unicode-3.0`, `Unlicense`, `GPL-3.0-or-later` — everything M0's
  dependency tree (serde, schemars, clap, thiserror, and their transitive
  deps) actually uses. `allow-wildcard-paths` doesn't cover the
  `plan`/`backend`/`frontend` path dependencies between each other because
  cargo-deny treats a crate without `publish = false` as publishable, so
  all three crates set `publish = false` (none of them are meant to reach
  crates.io).

## M1

- **`Action` grew three variants beyond `WriteFile` once real content
  (not just a dry-run description) had to exist somewhere:**
  `UncommentLine` for `/etc/locale.gen` (the base install ships every
  locale commented out; this is a real edit of existing content, not a
  fresh write), `RemoveFile` for offline cleanup's drop-ins (missing is
  success — they're not guaranteed to exist on every image), and
  `CopyIfMissing` for the offline kernel copy (SPEC.md: "copy
  `vmlinuz-linux` from the medium if the image has no kernel in `/boot`"
  — a real runtime condition, not something decidable while just
  building the command list). `Invocation`'s free-form `note` field
  (M0, dry-run-only decoration) became a structured `Capture` enum
  instead, since genfstab's stdout actually has to land in
  `/etc/fstab` for a real run, not just get mentioned in passing.

- **The keyboard layout is written to its own `keyboard.conf`, not
  `hyprland.conf` directly.** SPEC.md says to "set `kb_layout` and
  `kb_variant` in the default Hyprland config in `/etc/skel`", but
  `Action::WriteFile` fully overwrites a file — doing that to the shared
  `hyprland.conf` would destroy every other default (keybinds,
  autostart, window rules) that `luminos-desktop` or the user puts
  there. Instead Dawn owns a dedicated `~/.config/hypr/keyboard.conf`
  that `luminos-desktop`'s default `hyprland.conf` is expected to
  `source`, matching how Hyprland configs are conventionally split up
  for exactly this kind of override. `luminos-desktop` needs that
  `source` line — flagging as an open item below.

- **greetd's autologin override replaces the whole `config.toml`, not a
  drop-in.** greetd has no drop-in directory (unlike systemd units), so
  when the plan asks for autologin, Dawn writes a complete
  `config.toml` with both `default_session` (regreet, for later logins)
  and `initial_session` (the plan's user, autologin). Non-autologin
  installs don't touch the file at all — `luminos-desktop`'s own default
  stands.

- **`mkinitcpio.conf`'s `HOOKS` array is `luminos-base`'s job, not
  Dawn's.** SPEC.md's adapter table describes UKI presets with
  "systemd-based hooks plus microcode", but that's `/etc/mkinitcpio.conf`
  content the base package should ship correctly out of the box —
  there's no per-plan reason to vary it, unlike the preset file (which
  names the actual UKI outputs) or the cmdline (which needs the real
  root partition). Dawn only writes the two files that genuinely depend
  on the plan.

- **The kernel cmdline always includes a serial console
  (`console=tty0 console=ttyS0,115200n8`), not just `root=... rw`.**
  This is a real, permanent default, not a test-only hack: plenty of
  distros ship a serial console alongside the primary one for debugging,
  and with no serial port present the extra getty unit simply never
  starts — no real cost on actual hardware. It's also what lets
  `tests/e2e/qemu-run.sh` watch for a login prompt over the serial
  console instead of needing a framebuffer.

- **M1's own Done-when needs a live environment to run `dawn-backend`
  from, which doesn't exist yet** (SPEC.md's "Changes outside Dawn"
  section lists `luminos-repository` and the ISO profile as separate,
  not-yet-built work — `luminos-base`, `luminos-desktop` and the
  LuminOS-branded ISO aren't real yet). `tests/e2e/build-test-iso.sh`
  builds a throwaway archiso profile instead: plain Arch's `base` group
  in place of `luminos-base`/`luminos-desktop`, with `dawn-backend`
  copied in and a oneshot systemd service that installs the baked-in
  plan onto a second (virtio, `serial=dawn-target`) disk and powers off.
  This is enough to prove M1's actual Done-when — a plan installs and
  the result boots to a login prompt — without waiting on that other
  repo. It should be swapped for the real LuminOS ISO once M5's ISO
  integration exists; the plain-Arch substitution is scoped to
  `tests/e2e/` only and never touches the pipeline itself.

- **The test image gets a real `luminos` user added to its
  `passwd`/`shadow` overlay**, since modern releng's live session is
  just root (auto-logged in, no separate account at all) — without
  this, `offline_cleanup`'s `userdel -r luminos` has nothing to delete
  and fails outright.

- **Known, accepted gap: `offline_cleanup`'s
  `pacman -Rns luminos-dawn mkinitcpio-archiso` will still fail in the
  e2e test**, because `luminos-dawn` isn't a real installed package on
  the plain-Arch stand-in image (`pacman -R` aborts entirely if any
  named target isn't found — it doesn't partially succeed). Building a
  throwaway package just to satisfy this is possible (`makepkg` plus a
  local `file://` repo) but adds meaningful complexity for a test-only
  fixture problem, not a Dawn pipeline bug — the online path never
  exercises `offline_cleanup` at all. Deferred rather than guessed at
  further without seeing whether anything else needs fixing first; the
  online e2e path is unaffected and is where this PR's first real CI
  attention goes.

- **The loop-device integration test only covers the online
  (pacstrap) path.** The offline path unsquashes a path
  (`SQUASHFS_IMAGE` in `backend/src/adapters/arch.rs`) that only exists
  when actually booted from an archiso medium — a plain container has no
  such file. Offline install is exercised for real by
  `tests/e2e/run-e2e.sh offline` instead, which boots an actual
  archiso-based ISO. The loop-device test also never enrolls Secure
  Boot: `sbctl enroll-keys` needs real (or OVMF) UEFI variables a plain
  container doesn't have, and Secure Boot itself is M4 scope, not M1.

- **The local pinned mirror (`tests/e2e/setup-local-mirror.sh`) mirrors
  plain-Arch stand-ins for now**, not `luminos-base`/`luminos-desktop`
  (same reason as above — they don't exist yet): `base`, `linux` and
  `mkinitcpio`, plus the parts of those meta-packages the pipeline
  itself relies on — `networkmanager` and `greetd` (step 12 enables
  their services), `sudo` (step 8's drop-in), `zram-generator`
  (step 7's config) and `btrfs-progs` (mkinitcpio's `fsck` hook needs
  `fsck.btrfs` for a btrfs root, and without it `mkinitcpio -P` exits
  non-zero, failing step 9). The e2e scripts swap this list into each online
  plan's `packages`, since the plans themselves name the real
  meta-packages. It downloads the full dependency closure once from the
  real Arch mirror, resolved against an empty package database so
  nothing is skipped for already being installed on the CI host. Every
  install in the tests then gets a complete `pacman.conf` listing only
  that local mirror (not a repo appended to the host's own config,
  which would leave `[core]` and `[extra]` ahead of it), per CLAUDE.md's
  "never point automated tests at public Arch or LuminOS mirrors" rule
  — the one-time download is how a pinned local mirror gets built in
  the first place, not a test running against a public mirror.

- **CI (`.github/workflows/ci.yml`) runs the loop-device and QEMU
  end-to-end jobs inside an `archlinux:base-devel` container**, not
  directly on the `ubuntu-latest` host, even for the QEMU job. `mkarchiso`
  (needed to build the throwaway test ISO) has no Ubuntu package at all —
  it's Arch-only tooling. The container runs `--privileged`, which is
  also what gives it access to the runner's `/dev/kvm`. This hasn't had
  its first real CI run yet as of this PR (see "What couldn't be
  verified locally" in the PR description) — per the user's decision,
  VM tests run in CI, so its first real signal comes from there, not
  from guessing further locally.

## Open items for the user

- `luminos-desktop`'s default `hyprland.conf` needs to
  `source = ~/.config/hypr/keyboard.conf` for Dawn's keyboard-layout
  step to actually take effect. Worth confirming once that package
  exists.

- `luminos-base` must depend on `btrfs-progs`. SPEC.md's list of what
  the meta-packages cover doesn't name it, but with a btrfs root,
  mkinitcpio's default `fsck` hook fails the build without
  `fsck.btrfs`, so step 9 can't produce the UKIs. The e2e tests'
  stand-in package set includes it for the same reason.
