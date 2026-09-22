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
# Usage: loop-device-install.sh <mode: online> <plan-template.json> [pacman-conf-snippet]

set -euo pipefail

MODE="${1:?usage: loop-device-install.sh <online|offline> <plan-template.json> [pacman-conf-snippet]}"
PLAN_TEMPLATE="${2:?usage: loop-device-install.sh <online|offline> <plan-template.json> [pacman-conf-snippet]}"
MIRROR_SNIPPET="${3:-}"

DISK_IMAGE="$(mktemp -u /tmp/dawn-loopdev-XXXXXX.img)"
truncate -s 40G "$DISK_IMAGE"

LOOP_DEV="$(losetup --find --show --partscan "$DISK_IMAGE")"
BY_ID_LINK="/dev/disk/by-id/dawn-loopdev-test"

cleanup() {
  umount -R /mnt/verify 2>/dev/null || true
  umount -R /mnt/target 2>/dev/null || true
  rm -f "$BY_ID_LINK"
  losetup -d "$LOOP_DEV" 2>/dev/null || true
  rm -f "$DISK_IMAGE"
}
trap cleanup EXIT

mkdir -p /dev/disk/by-id
ln -sf "$LOOP_DEV" "$BY_ID_LINK"

if [ -n "$MIRROR_SNIPPET" ]; then
  cat "$MIRROR_SNIPPET" >> /etc/pacman.conf
  pacman -Sy --noconfirm
fi

PLAN_FILE="$(mktemp /tmp/dawn-loopdev-plan-XXXXXX.json)"
jq --arg device "$BY_ID_LINK" '.disk.device = $device' "$PLAN_TEMPLATE" > "$PLAN_FILE"

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
