# Decisions

Log of choices made during implementation where SPEC.md didn't spell out the
answer, or where a concrete detail had to be picked to make code compile.
Newest first.

## M3

- **Two backend processes: an unprivileged probe backend, and a root
  install backend started through pkexec with `--target`.** Asked the
  user. The GUI needs the backend before a disk is chosen (to list
  disks), but SPEC.md and CLAUDE.md say the backend only installs when
  `--target` on its command line matches the plan's disk. At startup
  the GUI runs `dawn-backend --serve` as the live user for the
  read-only requests (`list_disks`, `probe_firmware`, `check_online`,
  `validate`); without `--target` it refuses `install`. After the
  Summary screen's confirm, the GUI starts
  `pkexec dawn-backend --serve --target <disk>`, which does the
  install. The `--target` rule stays exactly as written, and root is
  only used for the install. This departs from SPEC.md's architecture
  diagram, which shows one backend running as root via pkexec.

- **The in-VM end-to-end test drives the real GUI through Slint's
  testing backend.** Asked the user. A driver in the test ISO builds the
  same `AppWindow` the `dawn` binary runs, with the real socket client,
  pkexec and backend, and clicks through the screens by accessible
  label, as M2's test does. Only pixel rendering is simulated, so the VM
  needs no compositor.

- **If the GUI goes away mid-install, the root backend stops.** Asked
  the user. A closed socket (crash, closed window, lost connection) is
  treated like `cancel`: the running step's processes are killed, the
  target is unmounted, and the backend exits. The disk is left partly
  installed, as with any failure, but root work never continues
  unsupervised.

- **"Retry from start" re-runs the install from step 1 with the same
  plan.** Asked the user. There's no resume, so "from start" means from
  partitioning, not back to the Welcome screen.

- **The polkit rule matches the live user alone, not their session.**
  `config/polkit/50-luminos-dawn.rules` returns YES for
  `org.luminos.dawn.backend` when `subject.user == "luminos"`, as
  SPEC.md words it; everyone else gets the action's defaults
  (`auth_admin_keep` for an active session). A first draft also required
  an active local session, but a GUI started outside a logind session,
  as the e2e driver's service is, would then be refused, and live-ISO
  installer rules conventionally match on the live user alone. The
  action's `exec.path` annotation is `/usr/bin/dawn-backend`, so pkexec
  picks it for that path only.

- **`plan` holds the socket protocol and `installer.toml`'s types**
  (`plan::protocol`, `plan::config`), still with no I/O, since both
  binaries speak and read them. When `/etc/dawn/installer.toml` is
  missing, both fall back to a compiled-in copy of
  `config/installer.toml`, which keeps the dry-run command working on a
  development machine. The GUI also takes `DAWN_CONFIG`, and passes that
  file on to backends that run as the user; the root backend always
  reads the system's own.

- **The runner streams.** Each command runs in its own process group,
  its output goes line by line to the log and the GUI, and cancelling
  sends the group SIGTERM, then SIGKILL after 5 seconds. Progress is
  parsed from pacman's `Packages (N)` and `installing` lines and from
  `unsquashfs -percentage`, weighted per step (step 5, the base system,
  is about two thirds of the bar). The log, with redaction, stays in the
  runner module, so
  CLAUDE.md's one place for dry-run, logging and redaction still holds.

- **Online means the repositories answer, checked the same way
  everywhere.** Step 1 of an online install, the GUI's startup probe and
  the re-check after joining Wi-Fi all send HEAD requests (curl,
  10-second timeout) for the repository databases in the live system's
  `pacman.conf`: every repository has to answer from at least one of its
  servers. After joining Wi-Fi the install goes online only if they
  answer; the network is set up on the installed system either
  way. A failed pacstrap whose output shows network trouble ("failed
  retrieving file", "could not resolve host", and so on) sets
  `offline_fallback`, and the error screen then offers "Install offline
  instead": the same plan again from step 1, from the live image.

- **greetd's autologin now edits only `[initial_session]`.** It sets
  `command = "Hyprland"` and the new user in `/etc/greetd/config.toml`
  through `toml_edit`, keeping `luminos-desktop`'s `[default_session]`
  and everything else. This replaces M1's rewrite of the whole file,
  which wrote `command = "regreet"`. The file has to exist; greetd's
  package ships one (see `LUMINOS-CHANGES.md`).

- **Step 7 writes `/etc/locale.conf` (`LANG`) and links
  `/etc/localtime`** to `/usr/share/zoneinfo/<timezone>`. The link is
  refused if its target doesn't exist inside the new system, so a bad
  timezone fails step 7 instead of leaving a dangling link.

- **Copy-to-RAM, as planned in M1.** The offline image is
  `/run/archiso/copytoram/<name>` when that exists, else the configured
  path. The kernel is copied from the image's
  `/usr/lib/modules/<version>/vmlinuz`, picking the directory whose
  `pkgbase` file says `linux`. The boot medium comes from archiso's
  `archisosearchuuid=`, `archisodevice=` or `archisolabel=`, resolved
  through sysfs to its whole disk. The backend refuses it as a target
  and never offers it. It also refuses a partition as `--target`: the
  plan always names a whole disk.

- **The probe backend's disk list.** `lsblk --list` with `PKNAME`, so a
  disk with any mounted partition counts as mounted. Hidden: mounted
  disks, the running system's disk, the boot medium, zram and anything
  that isn't a disk. Shown greyed out, with the reason: disks without a
  `/dev/disk/by-id/` name, read-only disks and disks smaller than
  `min_disk_gib`. Of several by-id names, a recognisable one wins over
  `wwn-` and `nvme-eui.`, then the shortest.

- **The Summary screen validates before installing.** Install asks the
  probe backend to `validate` the plan: its rules, plus whether the disk
  is still one it offers. Problems show on the Summary screen and the
  install doesn't start.

- **Messages that come from the backend stay English.** Step names,
  error messages, validation errors and a disk's unavailable reason
  arrive as text over the socket; every string the UI itself shows goes
  through `@tr`. v1 is English only (SPEC.md's non-goals); translating
  the backend's messages would need codes in the protocol, a v2 change.
  The Timezone screen's "UTC" became "Etc/UTC", which the plan's
  Region/City rule accepts, and every M2 string that missed `@tr` got
  it.

- **The error screen.** "The install couldn't start" when there's no
  step (pkexec refused, or the backend went away), otherwise the step
  and the backend's message, a note from step 2 on that the disk was
  already changed, and the backend's last 50 log lines, redacted. Save
  log writes the whole session's log to `~/dawn-install.log`, in the
  live user's home; the backend's own full log is `/var/log/dawn.log`,
  which step 12 copies into the installed system. A backend that went
  away is started afresh for the next install, so Retry works after
  pkexec was refused.

- **The Installing screen's log is a plain `ScrollView` holding the last
  500 lines.** It follows new lines until scrolled up; a `ListView`'s
  row estimates kept its scroll position a few pixels short of the last
  line. The error screen's log opens at its end, where the failure is.

- **Restart goes through logind's `Reboot`** over D-Bus, which an
  active session's user may call without a password; an error shows on
  the Done screen.

- **Wi-Fi through NetworkManager, on the GUI's side.** Scan is
  `RequestScan`, 3 seconds, then the access points, one entry per name,
  strongest first. Join is `AddAndActivateConnection` with the access
  point, so NetworkManager picks the key management (WPA2 or WPA3), as
  `nmcli device wifi connect` does. The password goes into the
  system-wide profile (`psk-flags` 0), so step 7's copy of the profile
  works on the installed system, and the plan's `network_profile` is
  that profile's keyfile name, which can differ from the network's.
  A join that fails deletes its profile. Scanning was tried against this
  machine's NetworkManager; joining wasn't, since it would have saved a
  real connection.

- **`dawn --dry-run` runs the real backend without pkexec**, with
  installs as `dawn-backend --dry-run`: the whole GUI over the real
  socket, touching nothing. `dawn --mock-backend` stays M2's canned
  backend.

- **The e2e test drives Dawn's GUI in one ISO, three scenarios.**
  Supersedes M1's plan-file runs, whose CLI path the dry-run snapshots
  and the loop-device job still cover. The stand-in `luminos-dawn` now
  carries `dawn-backend`, `gui_driver`, the polkit files and an
  `installer.toml` naming the stand-in packages, and a service that runs
  the driver as `luminos` for the scenario `qemu-run.sh` passes as an
  SMBIOS credential:
  - online: the mirror serves everything; the Network screen mustn't
    show.
  - offline: nothing serves the mirror, so the repositories are
    unreachable and the Network screen shows (there's no Wi-Fi adapter);
    the driver skips it. The VM keeps its network card: without one,
    systemd-networkd-wait-online would hold boot for 2 minutes.
  - fail-then-offline: the mirror serves only its databases, so step 1
    passes and pacstrap fails in step 5; the driver checks the error
    screen (step 5, the offline option, pacstrap's error in the log),
    then installs offline. The ISO is a USB stick with 4.5 GiB of RAM,
    so archiso's default `copytoram=auto` copies the image and unmounts
    the stick, and the driver checks it did. The plan said to boot with
    `copytoram=y`; this gets the same copy through the default the real
    ISO ships, without editing the ISO's boot entries.

  In every scenario the Disk screen must offer the target and nothing
  else, the install boots to a login prompt, and the password must
  never reach the serial console. The driver unticks Secure Boot, which
  the GUI offers checked because fresh OVMF variables are in Setup
  Mode; Secure Boot is M4's. It echoes the install log to the console,
  and CI uploads the serial logs.

- **A stand-in `luminos-live` proves the offline cleanup.** The test
  ISO's build moves releng's `pacman-init.service`,
  `etc-pacman.d-gnupg.mount`, volatile journal and do-not-suspend
  config into it, next to a unit that prints a marker on every boot. The
  marker must show on the live system and must not show when the
  installed system boots.

## M2

- **The app_id Slint's winit backend sets is empty, so `build_ui()`
  sets one explicitly: `luminos-dawn`.** SPEC.md asks M2 to confirm
  this ("The Hyprland window rule matches a stable window title and
  app_id; M2 confirms which app_id Slint's winit backend sets") —
  running the built binary under this machine's actual Hyprland session
  and checking `hyprctl clients` showed `class` came back empty. Fixed
  with `slint::set_xdg_app_id("luminos-dawn")` right after
  `AppWindow::new()`; confirmed by re-running and checking
  `hyprctl clients` again, which now reports `class: luminos-dawn`.
  M5's Hyprland window rule should match on this app_id, not the title
  (which is branding-derived and not stable across branding folders).

- **`frontend` is now a library plus a thin binary**, not just a binary.
  `slint::include_modules!()` (the generated `AppWindow`, `DiskInfo`,
  `Theme` types) lives in `src/lib.rs`; `src/main.rs` is just
  `frontend::build_ui()?.run()`. Rust's integration tests
  (`tests/clickthrough.rs`) only link a crate's library target, so the
  UI smoke test couldn't otherwise reach the generated types at all.

- **The UI smoke test drives Next/Back/Install by accessible label
  (`i_slint_backend_testing::ElementHandle`), matching SPEC.md's own
  description, but sets ComboBox-backed fields (locale, keyboard
  layout, timezone) directly via generated property setters.**
  std-widgets' `ComboBox` only implements `accessible-action-expand`,
  not a set-value action the way `LineEdit`, `Button` and `CheckBox`
  do, so there's no accessibility-level way to simulate "pick this
  option" on it. SPEC.md calls out driving Next and Back specifically,
  not every widget kind, so this reads as within scope rather than a
  compromise. The full-name → username/hostname derivation is still
  exercised for real, via `app.invoke_full_name_edited(...)`, which
  runs the exact closure a real `LineEdit`'s `edited` callback would.
  Requires `SLINT_EMIT_DEBUG_INFO`-equivalent debug info at build time
  (`CompilerConfiguration::with_debug_info(true)` in `build.rs`) — the
  `ElementHandle` API refuses to work without it.

- **Disk selection is a hand-rolled clickable list (`Rectangle` +
  `TouchArea` per row), not std-widgets' `RadioButton`.** Couldn't
  confirm `RadioButton` is actually exported from `std-widgets.slint`
  for the styles Dawn might ship with (only found `RadioButtonImpl` /
  `RadioGroupImpl` internals while checking), and a plain clickable row
  highlighted by comparing `disk.device == selected-device` needed no
  such confirmation. Revisit if a real `RadioButton` turns out to work
  fine — it would read more clearly.

- **The Disk screen's Manual-mode toggle exists but stays disabled**,
  and `state::build_install_plan` always sets `DiskMode::Erase`
  regardless of what the (currently unreachable) toggle would say.
  Manual mode needs partition assignments this screen has no UI for
  yet — that's M7. Once M7 adds the assignment table, both the toggle
  and `build_install_plan` need revisiting together.

- **Keyboard layout and timezone use a `ComboBox` with a small fixed
  list**, not the full X11 layout list or IANA zone database. SPEC.md
  asks for a "live preview field" (keyboard) and a "searchable list"
  (timezone) — the preview field exists (plain text echo, no real
  layout remapping yet); the searchable list is deferred, since neither
  a layout list nor timezone database is wired up as a real data source
  yet. The fixed list includes every value M2's own tests need
  (`us`/`""`, `Europe/Berlin`).

- **Network screen mocks Wi-Fi with a plain name/password `LineEdit`
  pair**, not a real scan-and-join flow. (Replaced in M3.) SPEC.md's own milestone table
  puts "NetworkManager Wi-Fi screen" under M3, alongside the socket
  protocol and pkexec — the D-Bus wiring belongs there, not M2.

- **Installing and Done screens are static placeholders** (a progress
  bar pinned at 0 and a fixed message, an unconditional "all done").
  (Replaced in M3.)
  Real progress/log/error events arrive over the socket in M3
  (SPEC.md's wire protocol); M2's Done-when only asks that all nine
  screens exist and clicking through builds a valid plan, which happens
  entirely before Installing is ever reached.

- **`deny.toml`'s allow-list grew by six licences** (`GPL-3.0-only`,
  `BSD-2-Clause`, `BSD-3-Clause`, `BSL-1.0`, `ISC`, `Zlib`) and ignores
  one advisory (`RUSTSEC-2026-0192`, `ttf-parser` unmaintained, no
  vulnerability, no safe upgrade, buried under winit's Wayland
  decoration rendering). Slint pulls in a much larger dependency tree
  than `plan`/`backend` needed alone (winit, resvg, arboard, atspi,
  zbus, ...); every added licence is a standard permissive OSI licence,
  checked against the actual resolved dependency tree with
  `cargo metadata`, not guessed.

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
  drop-in.** (Superseded in M3: only `[initial_session]` is edited.) greetd has no drop-in directory (unlike systemd units), so
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
  builds a throwaway archiso profile instead: releng plus the same
  plain-Arch stand-ins an online install gets (see the local mirror
  entry below), and stand-in `luminos-dawn` and `luminos-keyring`
  packages (see the offline cleanup entry below). The stand-in
  `luminos-dawn` carries `dawn-backend`, the baked-in plan, and a
  oneshot systemd service that installs it onto a second (virtio,
  `serial=dawn-target`) disk and powers off.
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

- **The test ISO installs stand-in `luminos-dawn` and `luminos-keyring`
  packages** (M3 adds `luminos-live`, and `luminos-dawn` carries the GUI
  driver instead of a plan) (`tests/e2e/stand-ins/`, built by
  `tests/e2e/build-stand-in-packages.sh` into a local `file://` repo
  that mkarchiso installs from). This was first deferred as an accepted
  gap, until CI confirmed it was the only thing failing the offline
  install: `offline_cleanup`'s `pacman -Rns luminos-dawn
  mkinitcpio-archiso` aborts outright when `luminos-dawn` isn't
  installed (`pacman -R` doesn't partially succeed), and
  `pacman-key --populate archlinux luminos` needs a `luminos` keyring.
  Everything the test adds to the live system (`dawn-backend`, the
  plan, the install-on-boot service, statically enabled from `/usr`)
  lives in the stand-in `luminos-dawn`. Step 6 then removes all of it
  from the installed system, as it will with the real package; before
  this, an offline install would have kept the service and tried to
  reinstall itself on first boot. The keyring holds a throwaway key
  generated per build, which never signs anything.

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
  non-zero, failing step 9). The list lives in
  `tests/e2e/stand-in-packages.x86_64`, and the test ISO includes it too,
  the way SPEC.md builds the real ISO from the same meta-packages: an
  offline install unsquashes the live image, and releng alone lacks
  `networkmanager`, `greetd` and `zram-generator`, so step 12 would
  fail. The e2e scripts swap this list into each online
  plan's `packages`, since the plans themselves name the real
  meta-packages (from M3, the test ISO's `installer.toml` names it
  instead). It downloads the full dependency closure once from the
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
  also what gives it access to the runner's `/dev/kvm`; the loop-device
  job also bind-mounts the host's `/dev`, since Docker's own `/dev` is a
  snapshot that never shows the loop device's new partitions. Per the
  user's decision, VM tests run in CI, so these jobs are where the
  privileged parts of the pipeline actually get exercised.

- **Microcode comes only from luminos-base's `microcode` hook.** Dawn's
  preset also set `ALL_microcode`, which mkinitcpio 42 ignores with a
  deprecation warning on every build, so the line was dropped. Agreed
  with the user after M1 merged.

- **`UncommentLine` treats an already-enabled line as done.** An offline
  install copies the live image, and an ISO that ships its locale
  already enabled in `/etc/locale.gen` would otherwise fail step 7 every
  time. A locale missing from the file entirely is still an error.
  Agreed with the user after M1 merged.

- **The QEMU end-to-end job runs on every push until M6.** SPEC.md's
  Testing table puts end-to-end runs nightly, and M6 is where the
  nightly workflow lands; until then the job (about 9 minutes) stays on
  every push, so each milestone's PR shows it. Agreed with the user.

- **Known gap, to be fixed in M3: step 7 sets neither the timezone nor
  `LANG`.** (Fixed in M3.) SPEC.md's step 7 includes the timezone, and the step's name
  says so, but no action writes `/etc/localtime` or `/etc/locale.conf`,
  and the plan's `timezone` field is unused. Found after M1 merged; the
  user chose to fix it in M3 rather than reopen M1 during M2.

- **Offline installs remove the ISO's live-only parts as one package,
  `luminos-live`.** Decided with the user; Dawn's side landed in M3. The
  LuminOS ISO is Arch's releng profile underneath, and an offline
  install copies the whole live image. Besides what step 6 already
  removes, the installed system would keep, enabled from `/etc`: the
  pacman keyring reset (`pacman-init.service` with
  `etc-pacman.d-gnupg.mount`, which wipes the keyring on every boot),
  sshd with archiso's password-login config, a volatile journal,
  lid-close suspend disabled, reflector and choose-mirror, and
  systemd-networkd/iwd next to NetworkManager. The ISO drops what a
  desktop live ISO doesn't need and packages the rest as `luminos-live`,
  installed only on the ISO, with its service enablement links inside
  the package and live-only tools as its dependencies. Step 6 removes it
  with `pacman -Rns`, like `mkinitcpio-archiso`, listed in
  `installer.toml`'s `[offline_cleanup] remove_packages` (SPEC.md). That
  list being config means an ISO without `luminos-live` yet just doesn't
  list it, since `pacman -R` fails outright on a package that isn't
  installed. Chosen over Dawn deleting a hardcoded list, as LuminOS's
  Calamares setup does, which goes stale whenever the ISO profile
  changes. The LuminOS side is in `LUMINOS-CHANGES.md`.

- **Dawn copes with archiso's copy-to-RAM itself.** Decided with the
  user; landed in M3. archiso's default (`copytoram=auto`) copies the
  image into RAM and unmounts the boot medium when it isn't an optical
  drive and there's at least 2 GiB of RAM to spare beyond the image,
  which covers most USB boots, and the LuminOS boot entries don't
  override it. Dawn will read the image from
  `/run/archiso/copytoram/airootfs.sfs` when it exists, falling back to
  the medium path, and take the kernel from the image's own
  `/usr/lib/modules/<version>/vmlinuz` instead of the medium. That
  departs from SPEC.md's "copy `vmlinuz-linux` from the medium if the
  image has no kernel in `/boot`". It will also identify the boot medium
  from the kernel command line (`archisodevice=` or `archisosearchuuid=`)
  and refuse it as a target even when unmounted, because once it's
  unmounted the existing mounted-disk check can't see it. Chosen over
  the ISO setting `copytoram=n`, which would run the live desktop off
  the USB stick.

## Planned for M3

Work the user scheduled for M3, on top of SPEC.md's own M3 scope. The
M1 entries above have the reasoning. All of it landed in M3; the M3
entries say how.

- **Offline cleanup removes `luminos-live`**, through `installer.toml`'s
  `[offline_cleanup] remove_packages`. The e2e test ISO needs a
  stand-in `luminos-live` under `tests/e2e/stand-ins/` for this, the
  way `luminos-dawn` has one. It can carry releng's live-only units, so
  the offline test also checks they're gone.
- **Offline installs cope with archiso's copy-to-RAM:**
  - the image path falls back to `/run/archiso/copytoram/`;
  - the kernel comes from the image;
  - the boot medium is refused by the UUID on the kernel command line;
  - the e2e test adds a boot with `copytoram=y`.
- **Step 7 writes the timezone** (`/etc/localtime`) **and `LANG`**
  (`/etc/locale.conf`).
- **greetd autologin stops hardcoding `command = "regreet"`**, which
  can't run without a compositor to host it. Either keep
  `luminos-desktop`'s own `[default_session]` and add only
  `[initial_session]`, or use the greeter wrapper `luminos-desktop`
  settles on (see `LUMINOS-CHANGES.md`).

## Open items for the user

- **Changes needed in LuminOS itself** (its ISO profile and
  luminos-repository) are tracked in `LUMINOS-CHANGES.md`.
