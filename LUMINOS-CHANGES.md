# Changes needed in LuminOS

Dawn replaces Calamares, and some of what it relies on lives outside
this repository: in the LuminOS ISO profile
([Lumin-OS/LuminOS](https://github.com/Lumin-OS/LuminOS)) and in
[Lumin-OS/luminos-repository](https://github.com/Lumin-OS/luminos-repository).
This file lists those changes. It's based on SPEC.md's "Changes outside
Dawn" and on both repositories as of September 2026: an X11/i3 ISO built
from Arch's releng profile, with Calamares and a prebuilt package
repository. Dawn's own side of each point is in SPEC.md and
DECISIONS.md.

## luminos-repository

### `luminos-base` and `luminos-desktop`

The meta-packages both the ISO and online installs are built from
(SPEC.md). Neither exists yet.

- `luminos-base` must depend on:
  - `btrfs-progs`. Dawn's root filesystem is btrfs, and mkinitcpio's
    `fsck` hook needs `fsck.btrfs`; without it `mkinitcpio -P` fails
    and no UKIs are built.
  - `networkmanager`, `sudo`, `zram-generator` and `sbctl`. Dawn's
    Secure Boot setup runs sbctl inside the new system, and sbctl's
    pacman hook re-signs the bootloader and UKIs after updates.
  - `amd-ucode` and `intel-ucode`. The UKIs get microcode from
    mkinitcpio's `microcode` hook, which takes it from these packages.
- `luminos-base` ships systemd-based HOOKS, including `microcode` and
  `fsck`, as a drop-in in `/etc/mkinitcpio.conf.d/`. The mkinitcpio
  package owns `/etc/mkinitcpio.conf`, so another package can't ship it.
  The drop-in doesn't affect the live ISO's initramfs: archiso's preset
  names its own config, and mkinitcpio then skips the drop-ins. Dawn
  writes only the preset and the kernel command line, not the HOOKS (see
  DECISIONS.md).
- `luminos-desktop` depends on Hyprland, greetd and regreet, and ships
  the dotfiles in `/etc/skel`:
  - The default Hyprland config is `hyprland.lua`, and it loads
    `~/.config/hypr/keyboard.lua` with `require("keyboard")`. Hyprland
    0.56 uses `hyprland.lua` whenever it exists and treats
    `hyprland.conf` as legacy. Dawn writes the chosen keyboard layout
    into `keyboard.lua`, never into `hyprland.lua` itself. (Decided with
    the user; until M5, Dawn still writes a `keyboard.conf` in the old
    format. See DECISIONS.md.)
  - Ship a default `/etc/skel/.config/hypr/keyboard.lua` too (for
    example `hl.config({ input = { kb_layout = "us" } })`), so users
    created outside Dawn don't start with a `require` of a missing file.
    Dawn overwrites it.
  - greetd reads a config file of `luminos-desktop`'s own,
    `/etc/greetd/luminos.toml`: a `greetd.service` drop-in starts it with
    `--config /etc/greetd/luminos.toml`. The greetd package owns
    `/etc/greetd/config.toml`, so `luminos-desktop` can't ship that one.
  - greetd needs a stable command for the `[default_session]` in
    `luminos.toml`. regreet has to run inside a compositor (Hyprland,
    per DECISIONS.md), so the command is something like a
    `luminos-greeter` wrapper that starts Hyprland with a greeter config
    running regreet. That section is `luminos-desktop`'s to write; Dawn
    leaves it alone.
  - Dawn's autologin option adds an `[initial_session]` to
    `/etc/greetd/luminos.toml` on the installed system, with the new
    user and `command = "start-hyprland"`, and keeps everything else in
    the file. Hyprland 0.56 calls starting without `start-hyprland`
    "strongly discouraged". (Decided with the user; until M5, Dawn still
    edits `/etc/greetd/config.toml` with `command = "Hyprland"`. See
    DECISIONS.md.)
  - The login screen should use the keyboard layout chosen in Dawn, or
    a password with layout-dependent characters won't work there. Dawn
    writes it where `localectl set-x11-keymap` does,
    `/etc/X11/xorg.conf.d/00-keyboard.conf`, so `localectl status` (or
    systemd-localed's `X11Layout` and `X11Variant` over D-Bus) reports
    it; the greeter's compositor config should take its layout from
    there. Hyprland sessions get theirs from the `keyboard.lua` Dawn
    writes into `/etc/skel`.

### `luminos-keyring`

A package with `/usr/share/pacman/keyrings/luminos.gpg` and
`luminos-trusted` (optionally `luminos-revoked`), holding the key the
repository is signed with. It belongs on the ISO and in `luminos-base`.
An offline install's cleanup runs
`pacman-key --populate archlinux luminos`, which fails without it, and
online installs need the key to verify packages from this repository.

### Signing

Online installs pacstrap from this repository through the ISO's
`pacman.conf`, which should verify signatures (SPEC.md: "with the ISO's
pacman.conf, which includes the LuminOS repo and its signing key").
Today the packages carry no `.sig` files, and `dbupdate.sh` renames the
database signature to `luminos-repository-db.sig`, a name pacman never
asks for; it looks for `luminos-repository.db.sig`. Sign the packages
(`makepkg --sign`, or `repo-add` with signatures included) and keep the
names pacman expects.

### `luminos-dawn`

Dawn itself (SPEC.md), replacing `calamares`, `calamares-git` and
`calamares-settings`; `ckbcomp` was only there for Calamares. Besides
the binaries, it should carry everything Dawn-specific the live session
needs: the polkit action and rule, the desktop entry,
`/etc/dawn/installer.toml` and the LuminOS branding folder. An offline
install's step 6 removes `luminos-dawn` with `pacman -Rns`, so anything
it owns also disappears from the installed system; anything placed in
the ISO's `airootfs/` instead would stay behind.

- Install the backend as `/usr/bin/dawn-backend`. The GUI starts it
  through pkexec by that path, and the polkit action matches on it.
- The polkit files are in Dawn's `config/polkit/`:
  `org.luminos.dawn.policy` goes to `/usr/share/polkit-1/actions/`, and
  `50-luminos-dawn.rules`, which lets the live user `luminos` start the
  backend without a password, to `/usr/share/polkit-1/rules.d/`.
- Runtime dependencies: `polkit` (pkexec), `fontconfig` (the GUI
  links it) and `xkeyboard-config` (the Keyboard screen's list of
  layouts) for the GUI; for the backend, `arch-install-scripts`
  (pacstrap, arch-chroot, genfstab), `util-linux`, `btrfs-progs`,
  `dosfstools`, `parted` (partprobe), `squashfs-tools` (unsquashfs),
  `gnupg` and `curl` (it checks the repositories are reachable before
  an online install). The language and timezone lists come from glibc
  and tzdata, which every system has.
- The Done screen points people whose firmware wasn't in Setup Mode at
  how to set up Secure Boot later: `branding.toml`'s `secure_boot_url`,
  the Arch Wiki's sbctl section, since LuminOS has no page of its own
  for it.

### `luminos-live`

The ISO's live-only parts, packaged so an offline install can remove
them in one step (decided with Dawn). It's installed only on the ISO:
list it in `packages.x86_64`, never in the meta-packages. The
"Live-only parts" item below says what goes in it. It isn't published
in this repository after all: the LuminOS repository builds it from
its `luminos-live/` with `build.sh`, for its own ISO builds only
([LuminOS#3](https://github.com/Lumin-OS/LuminOS/pull/3)).

Dawn's mechanism is done: an offline install removes every package in
`installer.toml`'s `[offline_cleanup] remove_packages`. Add
`luminos-live` there (the `installer.toml` that `luminos-dawn` ships)
in the same ISO build that starts installing it: `pacman -R` fails
outright on a package that isn't installed, and without the entry,
offline installs keep the live-only parts. LuminOS#3 makes the ISO
install it, so it merges together with that Dawn change, which is for
M5. Dawn's e2e test builds a stand-in `luminos-live` from releng's
keyring reset, volatile journal and do-not-suspend units, and checks
that none of it survives an offline install.

## ISO profile (the LuminOS repository's `releng/`)

[LuminOS#3](https://github.com/Lumin-OS/LuminOS/pull/3) does every item
below. The ISO builds once luminos-repository has the meta-packages,
`luminos-keyring` and `luminos-dawn`.

- **Desktop**: Hyprland on Wayland in place of i3 on X11 (the i3
  packages and `/home/luminos/.xinitrc`). *Done in LuminOS#3: the tty1
  autologin starts it with `start-hyprland`.*
- **`packages.x86_64`**: built from `luminos-base` and `luminos-desktop`
  plus live-only extras (SPEC.md), with `luminos-dawn` in place of
  Calamares. Keep `mkinitcpio-archiso`: the live boot needs it, and
  Dawn's step 6 removes it from offline installs. *Done in LuminOS#3,
  which also lists `luminos-keyring`, `luminos-live`, `syslinux`
  (mkarchiso requires it in the list) and `zsh` (root's shell, which
  offline installs keep).*
- **`pacman.conf`**: add `[luminos-repository]` with its `Server` line.
  Dawn's online install runs pacstrap with the live system's own
  `/etc/pacman.conf`, so the repository has to be in the live image's
  copy too, not only in the profile's build config. *Done in LuminOS#3,
  in both copies.*
- **Live networking**: NetworkManager in place of releng's
  systemd-networkd, iwd and resolved. Dawn's Network screen scans and
  joins Wi-Fi through NetworkManager over D-Bus, and step 7 copies the
  joined network's profile from `/etc/NetworkManager/system-connections/`.
  The ISO doesn't include `networkmanager` at all today. Dawn saves the
  joined network as a system-wide profile with its password in the
  file, so step 7 can copy a working connection; NetworkManager only
  allows that without a password prompt through the polkit rule its Arch
  package ships, for `wheel` users in a local session. That's one more
  reason to keep the live user in `wheel`. *Done in LuminOS#3.*
- **Live user**: keep it named `luminos` (step 6 runs
  `userdel -r luminos`, and Dawn's polkit rule names it) and in `wheel`.
  Dawn runs as this user in the live desktop session. Step 6 also
  removes the tty1 autologin drop-in at
  `/etc/systemd/system/getty@tty1.service.d/autologin.conf`. If the
  Hyprland live session logs in through greetd instead, Dawn's step 6
  has to remove that configuration instead, so tell Dawn. *Done in
  LuminOS#3. The live session still logs in through the tty1 autologin,
  not greetd, but the drop-in moved into `luminos-live`, at
  `/usr/lib/systemd/system/getty@tty1.service.d/autologin.conf`, so
  removing the package removes it. Step 6's deletion of the `/etc` path
  is now dead code, to drop in M5.*
- **polkit**: the ISO needs `polkit` (a dependency of `luminos-dawn`)
  for pkexec, and the live session must be an active logind session
  (a normal autologin is): Dawn's "Restart now" asks logind to reboot,
  which it allows without a password only for active sessions. *Done in
  LuminOS#3: polkit comes with `luminos-dawn`, and the tty1 autologin is
  an active logind session.*
- **Dawn in the live session** (SPEC.md): a Hyprland window rule that
  opens Dawn floating, centred, at a fixed size, matched on its app_id
  `luminos-dawn` (set in M2, see DECISIONS.md), plus an autostart entry
  and a launcher bind. Start Dawn from inside the live Hyprland session
  (`exec-once` or a `bind` does), so it inherits
  `HYPRLAND_INSTANCE_SIGNATURE` and `XDG_RUNTIME_DIR`: the Keyboard
  screen switches the session's layout through Hyprland's control
  socket, with a `hyprland.conf` or a `hyprland.lua` configuration. Put these in the live user's own Hyprland config
  or in `luminos-dawn`, not in `/etc/skel`, so they don't carry over
  into installed systems. *Done in LuminOS#3, in the live user's own
  `hyprland.lua`. Its autostart waits for NetworkManager's startup,
  since Dawn checks the network only once, at launch.*
- **`install_dir`** stays `arch`: Dawn reads the offline image from
  `/run/archiso/bootmnt/arch/x86_64/airootfs.sfs`, or from
  `/run/archiso/copytoram/airootfs.sfs` when archiso copied it to RAM.
  *Done: LuminOS#3 keeps it, with a comment saying why.*
- **Boot entries**: leave archiso's `copytoram` default as it is. Dawn
  handles an image copied to RAM as well as one read from the medium,
  takes the kernel from the image itself, and recognises the boot
  medium from archiso's own kernel parameters (`archisosearchuuid=`,
  `archisodevice=` or `archisolabel=`), so the boot entries have to
  keep one of them (releng's use `archisosearchuuid=`, LuminOS's
  `archisodevice=UUID=`). Its e2e test boots from a USB stick with
  enough RAM for the copy, to cover this. *Done: LuminOS#3 leaves the
  boot entries as they are. A gap remains: `grub/loopback.cfg`, for
  booting the ISO file from another disk, passes none of the three.*
- **Live-only parts**: an offline install copies the whole live image,
  and the ISO enables releng's live-only services from `/etc`; step 6
  removes only some of them. First drop what a desktop live ISO doesn't
  need: sshd and its `10-archiso.conf`, cloud-init, reflector,
  choose-mirror and `Installation_guide`. Then move the live-only pieces
  that remain into `luminos-live`:
  - the pacman keyring reset: `pacman-init.service` and
    `etc-pacman.d-gnupg.mount`, which would otherwise wipe the installed
    system's keyring on every boot;
  - the volatile journal: `journald.conf.d/volatile-storage.conf`;
  - lid-close suspend disabled: `logind.conf.d/do-not-suspend.conf`;
  - the live autologin;
  - root's `.zlogin` and `.automated_script.sh`;
  - the livecd sound and speech helpers.

  Services enabled only for the live session ship their enablement
  links in the package as well (under `/usr/lib/systemd/system/*.wants/`),
  so removing the package disables them. Live-only extra packages, such
  as the rescue and install-medium tools, can be dependencies of
  `luminos-live` instead of entries in `packages.x86_64`. pacman then
  installs them as dependencies, and step 6's `pacman -Rns` removes
  them along with it. *Done in LuminOS#3, which also moves the VM
  guest, ModemManager, pcscd and time-wait-sync links, root's gpg
  smart-card config and the motd into `luminos-live`.*
