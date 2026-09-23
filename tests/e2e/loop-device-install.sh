#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Loop-device integration test (SPEC.md "Testing"): proves partitioning,
# formatting, mounting and pacstrap really work, against a sparse file on
# a loop device. Must run as root inside a disposable container —
# CLAUDE.md's safety rules forbid this on a developer's own machine.
#
# Online only: the offline path unsquashes an archiso medium's own image
# (SQUASHFS_IMAGE in backend/src/adapters/arch.rs), which only exists
# when actually booted from that medium — a plain container has no such
# path. Offline install is exercised for real by run-e2e.sh instead,
# which boots a real archiso-based ISO. secure_boot.enroll must also be
# false in the plan: sbctl enroll-keys needs real (or OVMF) UEFI
# variables a plain container doesn't have — that's run-e2e.sh's job
# too, from M4 onward.
#
# Usage: loop-device-install.sh <mode: online> <plan-template.json> <mirror-dir>
#
# <mirror-dir> is setup-local-mirror.sh's output: the install sees that
# mirror and nothing else, and asks for the stand-in packages it carries.

set -euo pipefail

USAGE="usage: loop-device-install.sh <online> <plan-template.json> <mirror-dir>"
MODE="${1:?$USAGE}"
PLAN_TEMPLATE="${2:?$USAGE}"
MIRROR_DIR="${3:?$USAGE}"

DISK_IMAGE="$(mktemp -u /tmp/dawn-loopdev-XXXXXX.img)"
truncate -s 40G "$DISK_IMAGE"

LOOP_DEV="$(losetup --find --show --partscan "$DISK_IMAGE")"
BY_ID_LINK="/dev/disk/by-id/dawn-loopdev-test"
PACMAN_CONF_BACKUP="$(mktemp /tmp/dawn-loopdev-pacman-XXXXXX.conf)"
cp /etc/pacman.conf "$PACMAN_CONF_BACKUP"

cleanup() {
  umount -R /mnt/verify 2>/dev/null || true
  umount -R /mnt/target 2>/dev/null || true
  rm -f "$BY_ID_LINK" "${BY_ID_LINK}-part1" "${BY_ID_LINK}-part2"
  losetup -d "$LOOP_DEV" 2>/dev/null || true
  rm -f "$DISK_IMAGE"
  cp "$PACMAN_CONF_BACKUP" /etc/pacman.conf
}
trap cleanup EXIT

# Loop devices have no serial number, so udev never gives them
# /dev/disk/by-id/ links (and a plain container has no udev at all).
# Erase mode always addresses partitions as <device>-part1/-part2
# (SPEC.md: disks are addressed by /dev/disk/by-id/ paths), so
# dawn-backend needs these to exist before it can partition and format
# anything. The partition nodes they point at only appear once
# dawn-backend partitions the disk, and only if /dev is the kernel's
# devtmpfs rather than a container's snapshot of it (ci.yml bind-mounts
# the host's /dev for this).
mkdir -p /dev/disk/by-id
ln -sf "$LOOP_DEV" "$BY_ID_LINK"
ln -sf "${LOOP_DEV}p1" "${BY_ID_LINK}-part1"
ln -sf "${LOOP_DEV}p2" "${BY_ID_LINK}-part2"

# The arch adapter runs pacstrap with -C /etc/pacman.conf, so the
# mirror's own config goes there for the length of the test.
cp "$MIRROR_DIR/pacman.conf" /etc/pacman.conf

PLAN_FILE="$(mktemp /tmp/dawn-loopdev-plan-XXXXXX.json)"
jq --arg device "$BY_ID_LINK" --slurpfile packages "$MIRROR_DIR/packages.json" \
  '.disk.device = $device | .packages = $packages[0]' "$PLAN_TEMPLATE" > "$PLAN_FILE"

echo "==> Running dawn-backend for real against $LOOP_DEV ($MODE)" >&2
cargo run --release -p backend -- --target "$BY_ID_LINK" "$PLAN_FILE"

echo "==> Verifying the install landed on disk" >&2
mkdir -p /mnt/verify
mount -o subvol=@ "${LOOP_DEV}p2" /mnt/verify

test "$(cat /mnt/verify/etc/hostname)" = "$(jq -r '.hostname' "$PLAN_FILE")"
grep -q "^$(jq -r '.user.username' "$PLAN_FILE"):" /mnt/verify/etc/passwd
grep -q "^root:!" /mnt/verify/etc/shadow
test -f /mnt/verify/boot/EFI/Linux/arch-linux.efi
test -f /mnt/verify/boot/EFI/Linux/arch-linux-fallback.efi

umount /mnt/verify
echo "==> $MODE loop-device install verified" >&2
