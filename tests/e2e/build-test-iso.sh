#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds the e2e test ISO: a minimal archiso whose live system runs Dawn's
# GUI, driven by gui_driver, against a second disk on boot, then powers
# off. It stands in for the real LuminOS ISO (SPEC.md M5's "ISO
# integration" hasn't happened yet; see DECISIONS.md) just enough to
# prove M3's Done-when: Dawn installs a VM end to end from the GUI. One
# image serves every scenario; qemu-run.sh names the scenario at boot.
# Must run as root (mkarchiso needs it) inside a disposable container,
# never on a developer machine.
#
# Like the real ISO, it's built from the same packages an online install
# gets (the plain-Arch stand-ins in stand-in-packages.x86_64), plus
# stand-ins for luminos-dawn, luminos-live and a luminos keyring, built
# by build-stand-in-packages.sh.
#
# Usage: build-test-iso.sh <mirror-dir> <output.iso>
#
# <mirror-dir> is setup-local-mirror.sh's output, built for a QEMU guest
# (server host 10.0.2.2). The live system's pacman.conf is that mirror's,
# so online installs see it and nothing else, and installer.toml asks for
# the stand-in packages it carries. Expects `cargo build --release` to
# have built dawn-backend and the gui_driver example.

set -euo pipefail

USAGE="usage: build-test-iso.sh <mirror-dir> <output.iso>"
MIRROR_DIR="${1:?$USAGE}"
OUTPUT_ISO="${2:?$USAGE}"
E2E_DIR="$(dirname "$0")"

WORKDIR="$(mktemp -d /tmp/dawn-iso-build-XXXXXX)"
PROFILE="$WORKDIR/profile"
cp -r /usr/share/archiso/configs/releng "$PROFILE"

# releng compresses its squashfs with xz, which takes about 15 minutes on
# a two-CPU CI runner. This image is thrown away after the test, so fast
# zstd is the better trade for a somewhat bigger ISO. profiledef.sh is
# sourced, so this later assignment wins.
echo "airootfs_image_tool_options=('-comp' 'zstd' '-Xcompression-level' '1' '-b' '1M')" \
  >> "$PROFILE/profiledef.sh"

# releng's live-only units, which LUMINOS-CHANGES.md moves into
# luminos-live: the pacman keyring reset (which would wipe an installed
# system's keyring on every boot), the volatile journal and
# lid-close suspend disabled. They move from the profile's /etc into the
# stand-in luminos-live's /usr, so they stay on the live system and an
# offline install's step 6 removes them with the package.
LIVE_ONLY="$WORKDIR/live-only"
move_live_only() {
  local from="$PROFILE/airootfs/$1" to="$LIVE_ONLY/$2"
  if [ ! -e "$from" ]; then
    echo "error: releng no longer has /$1; update build-test-iso.sh" >&2
    exit 1
  fi
  mkdir -p "$(dirname "$to")"
  mv "$from" "$to"
}
move_live_only etc/systemd/system/pacman-init.service usr/lib/systemd/system/pacman-init.service
move_live_only etc/systemd/system/etc-pacman.d-gnupg.mount usr/lib/systemd/system/etc-pacman.d-gnupg.mount
move_live_only etc/systemd/journald.conf.d/volatile-storage.conf usr/lib/systemd/journald.conf.d/volatile-storage.conf
move_live_only etc/systemd/logind.conf.d/do-not-suspend.conf usr/lib/systemd/logind.conf.d/do-not-suspend.conf
# The enablement link moves too, but as a new relative link: the old one
# may point into /etc.
rm "$PROFILE/airootfs/etc/systemd/system/multi-user.target.wants/pacman-init.service"
mkdir -p "$LIVE_ONLY/usr/lib/systemd/system/multi-user.target.wants"
ln -s ../pacman-init.service "$LIVE_ONLY/usr/lib/systemd/system/multi-user.target.wants/pacman-init.service"

# SPEC.md's installer.toml, with the stand-in packages in place of the
# meta-packages, and luminos-live removed from offline installs as the
# real ISO's will be once it ships one (LUMINOS-CHANGES.md).
CONFIG="$WORKDIR/installer.toml"
cat > "$CONFIG" <<EOF
branding = "luminos"
adapter = "arch"

[source]
packages = $(jq -c . "$MIRROR_DIR/packages.json")
squashfs = "/run/archiso/bootmnt/arch/x86_64/airootfs.sfs"

[defaults]
filesystem = "btrfs"
admin_group = "wheel"
min_disk_gib = 32

[screens]
autologin_option = true

[offline_cleanup]
remove_packages = ["mkinitcpio-archiso", "luminos-live"]
EOF

REPO_DIR="$WORKDIR/stand-in-repo"
"$E2E_DIR/build-stand-in-packages.sh" target/release/dawn-backend \
  target/release/examples/gui_driver "$CONFIG" "$LIVE_ONLY" "$REPO_DIR"
cat >> "$PROFILE/pacman.conf" <<EOF

[dawn-e2e]
SigLevel = Optional TrustAll
Server = file://$REPO_DIR
EOF
{
  echo
  echo "# Added by Dawn's build-test-iso.sh."
  cat "$E2E_DIR/stand-in-packages.x86_64"
  echo luminos-dawn
  echo luminos-live
  echo luminos-keyring
} >> "$PROFILE/packages.x86_64"

# Modern releng's live session is just root, auto-logged in, with no
# separate live user (see DECISIONS.md). The real LuminOS ISO has one
# named "luminos", which the GUI runs as, the polkit rule lets start the
# install backend, and step 6's `userdel -r luminos` removes, so this
# test image needs one too. releng's own airootfs already overrides
# /etc/passwd and /etc/shadow (with just a root entry), so appending is
# safe here, unlike /etc/group, which releng doesn't override: creating
# one from scratch would replace the live system's real one instead of
# adding to it.
echo "luminos:x:1000:1000:LuminOS live user:/home/luminos:/usr/bin/bash" \
  >> "$PROFILE/airootfs/etc/passwd"
echo "luminos:!:1::::::" >> "$PROFILE/airootfs/etc/shadow"
mkdir -p "$PROFILE/airootfs/home/luminos"
echo 'file_permissions["/home/luminos"]="1000:1000:750"' >> "$PROFILE/profiledef.sh"

# Replaces the live image's own /etc/pacman.conf, which the arch
# adapter's pacstrap -C reads. mkarchiso copies airootfs/ in before it
# installs packages, and pacman.conf is a backup file in the pacman
# package, so pacman keeps this copy and sets its own aside as
# pacman.conf.pacnew.
mkdir -p "$PROFILE/airootfs/etc"
cp "$MIRROR_DIR/pacman.conf" "$PROFILE/airootfs/etc/pacman.conf"

mkarchiso -v -w "$WORKDIR/work" -o "$WORKDIR/out" "$PROFILE"
cp "$WORKDIR"/out/*.iso "$OUTPUT_ISO"
rm -rf "$WORKDIR"
