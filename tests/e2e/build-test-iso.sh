#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds a minimal archiso that runs dawn-backend against a second disk
# on boot, then powers off. This stands in for the real LuminOS ISO
# (SPEC.md M5's "ISO integration" hasn't happened yet — see DECISIONS.md)
# just enough to prove M1's pipeline: a plan installs into a QEMU VM disk
# and the result boots to a login prompt. Must run as root (mkarchiso
# needs it) inside a disposable container, never on a developer machine.
#
# Like the real ISO, it's built from the same packages an online install
# gets (the plain-Arch stand-ins in stand-in-packages.x86_64), plus a
# luminos-dawn and a luminos keyring: stand-ins built by
# build-stand-in-packages.sh, which carry dawn-backend, the plan and the
# install-on-boot service.
#
# Usage: build-test-iso.sh <plan.json> <output.iso> [mirror-dir]
#
# [mirror-dir] is setup-local-mirror.sh's output, for online plans: the
# live system's pacstrap then sees that mirror and nothing else, and the
# plan asks for the stand-in packages it carries.

set -euo pipefail

E2E_DIR="$(dirname "$0")"
PLAN_FILE="${1:?usage: build-test-iso.sh <plan.json> <output.iso> [mirror-dir]}"
OUTPUT_ISO="${2:?usage: build-test-iso.sh <plan.json> <output.iso> [mirror-dir]}"
MIRROR_DIR="${3:-}"

WORKDIR="$(mktemp -d /tmp/dawn-iso-build-XXXXXX)"
PROFILE="$WORKDIR/profile"
cp -r /usr/share/archiso/configs/releng "$PROFILE"

# releng compresses its squashfs with xz, which takes about 15 minutes on
# a two-CPU CI runner. This image is thrown away after one boot, so fast
# zstd is the better trade for a somewhat bigger ISO. profiledef.sh is
# sourced, so this later assignment wins.
echo "airootfs_image_tool_options=('-comp' 'zstd' '-Xcompression-level' '1' '-b' '1M')" \
  >> "$PROFILE/profiledef.sh"

# The test target disk is a virtio drive with serial=dawn-target, so it
# always resolves to this by-id path regardless of device enumeration
# order — the same reasoning SPEC.md gives for by-id paths in general.
TARGET_DEVICE="/dev/disk/by-id/virtio-dawn-target"

if [ -n "$MIRROR_DIR" ]; then
  jq --arg device "$TARGET_DEVICE" --slurpfile packages "$MIRROR_DIR/packages.json" \
    '.disk.device = $device | .packages = $packages[0]' "$PLAN_FILE" \
    > "$WORKDIR/plan.json"
else
  jq --arg device "$TARGET_DEVICE" '.disk.device = $device' "$PLAN_FILE" \
    > "$WORKDIR/plan.json"
fi

REPO_DIR="$WORKDIR/stand-in-repo"
"$E2E_DIR/build-stand-in-packages.sh" target/release/dawn-backend \
  "$WORKDIR/plan.json" "$REPO_DIR"
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
  echo luminos-keyring
} >> "$PROFILE/packages.x86_64"

# Modern releng's live session is just root, auto-logged in — no
# separate live user (see DECISIONS.md). The real LuminOS ISO has one
# named "luminos" (confirmed by the user), which offline_cleanup's
# `userdel -r luminos` expects to exist, so this test image needs one
# too. releng's own airootfs already overrides /etc/passwd and
# /etc/shadow (with just a root entry), so appending is safe here —
# unlike /etc/group, which releng doesn't override, so creating one
# from scratch would replace the live system's real one instead of
# adding to it.
echo "luminos:x:1000:1000:LuminOS live user:/home/luminos:/usr/bin/bash" \
  >> "$PROFILE/airootfs/etc/passwd"
echo "luminos:!:1::::::" >> "$PROFILE/airootfs/etc/shadow"
mkdir -p "$PROFILE/airootfs/home/luminos"

if [ -n "$MIRROR_DIR" ]; then
  # Replaces the live image's own /etc/pacman.conf, which the arch
  # adapter's pacstrap -C reads. mkarchiso copies airootfs/ in before it
  # installs packages, and pacman.conf is a backup file in the pacman
  # package, so pacman keeps this copy and sets its own aside as
  # pacman.conf.pacnew.
  mkdir -p "$PROFILE/airootfs/etc"
  cp "$MIRROR_DIR/pacman.conf" "$PROFILE/airootfs/etc/pacman.conf"
fi

mkarchiso -v -w "$WORKDIR/work" -o "$WORKDIR/out" "$PROFILE"
cp "$WORKDIR"/out/*.iso "$OUTPUT_ISO"
rm -rf "$WORKDIR"
