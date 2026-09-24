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
  - `networkmanager`, `sudo`, `zram-generator` and `sbctl`.
  - `amd-ucode` and `intel-ucode`. The UKIs get microcode from
    mkinitcpio's `microcode` hook, which takes it from these packages.
- `luminos-base` ships `/etc/mkinitcpio.conf` with systemd-based HOOKS,
  including `microcode` and `fsck`. Dawn writes only the preset and the
  kernel command line, not the HOOKS (see DECISIONS.md).
- `luminos-desktop` depends on Hyprland, greetd and regreet, and ships
  the dotfiles in `/etc/skel`:
  - The default `hyprland.conf` must `source = ~/.config/hypr/keyboard.conf`.
    Dawn writes the chosen keyboard layout into that file, never into
    `hyprland.conf` itself.
  - Ship a default `/etc/skel/.config/hypr/keyboard.conf` too (for
    example `input { kb_layout = us }`), so users created outside Dawn
    don't start with a `source` line pointing at a missing file. Dawn
    overwrites it.
  - greetd needs a stable command for its `[default_session]`. regreet
    has to run inside a compositor (Hyprland, per DECISIONS.md), so the
    command is something like a `luminos-greeter` wrapper that starts
    Hyprland with a greeter config running regreet. Dawn's autologin
    option rewrites `/etc/greetd/config.toml` and currently writes
    `command = "regreet"`, which can't work on its own. Dawn fixes that
    in its M3, either by keeping `luminos-desktop`'s own
    `[default_session]` or by using this wrapper.

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
needs: the polkit action and the rule letting only the live user run the
backend without a password, the desktop entry, `/etc/dawn/installer.toml`
and the LuminOS branding folder. An offline install's step 6 removes
`luminos-dawn` with `pacman -Rns`, so anything it owns also disappears
from the installed system; anything placed in the ISO's `airootfs/`
instead would stay behind.

### `luminos-live`

The ISO's live-only parts, packaged so an offline install can remove
them in one step: decided with Dawn, whose side lands in its M3. It's
installed only on the ISO: list it in `packages.x86_64`, never in the
meta-packages. The "Live-only parts" item below says what goes in it.
Add it to `installer.toml`'s `[offline_cleanup] remove_packages` (which
`luminos-dawn` ships) only once the ISO actually installs it:
`pacman -R` fails outright on a package that isn't installed.

## ISO profile (the LuminOS repository's `releng/`)

- **Desktop**: Hyprland on Wayland in place of i3 on X11 (the i3
  packages and `/home/luminos/.xinitrc`).
- **`packages.x86_64`**: built from `luminos-base` and `luminos-desktop`
  plus live-only extras (SPEC.md), with `luminos-dawn` in place of
  Calamares. Keep `mkinitcpio-archiso`: the live boot needs it, and
  Dawn's step 6 removes it from offline installs.
- **`pacman.conf`**: add `[luminos-repository]` with its `Server` line.
  Dawn's online install runs pacstrap with the live system's own
  `/etc/pacman.conf`, so the repository has to be in the live image's
  copy too, not only in the profile's build config.
- **Live networking**: NetworkManager in place of releng's
  systemd-networkd, iwd and resolved. Dawn's Network screen scans and
  joins Wi-Fi through NetworkManager over D-Bus, and step 7 copies the
  joined network's profile from `/etc/NetworkManager/system-connections/`.
  The ISO doesn't include `networkmanager` at all today.
- **Live user**: keep it named `luminos` (step 6 runs
  `userdel -r luminos`) and in `wheel`. Step 6 also removes the tty1
  autologin drop-in at
  `/etc/systemd/system/getty@tty1.service.d/autologin.conf`. If the
  Hyprland live session logs in through greetd instead, Dawn's step 6
  has to remove that configuration instead, so tell Dawn.
- **Dawn in the live session** (SPEC.md): a Hyprland window rule that
  opens Dawn floating, centred, at a fixed size, matched on its app_id
  `luminos-dawn` (set in M2, see DECISIONS.md), plus an autostart entry
  and a launcher bind. Put these in the live user's own Hyprland config
  or in `luminos-dawn`, not in `/etc/skel`, so they don't carry over
  into installed systems.
- **`install_dir`** stays `arch`: Dawn reads the offline image from
  `/run/archiso/bootmnt/arch/x86_64/airootfs.sfs`, or from
  `/run/archiso/copytoram/airootfs.sfs` when archiso copied it to RAM.
- **Boot entries**: leave archiso's `copytoram` default as it is. From
  its M3, Dawn handles an image copied to RAM as well as one read from
  the medium, and takes the kernel from the image itself (decided with
  Dawn).
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
  them along with it.
