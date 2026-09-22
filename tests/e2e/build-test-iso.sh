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
# Usage: build-test-iso.sh <plan.json> <output.iso> [pacman-conf-snippet]

set -euo pipefail

PLAN_FILE="${1:?usage: build-test-iso.sh <plan.json> <output.iso> [pacman-conf-snippet]}"
OUTPUT_ISO="${2:?usage: build-test-iso.sh <plan.json> <output.iso> [pacman-conf-snippet]}"
MIRROR_SNIPPET="${3:-}"

WORKDIR="$(mktemp -d /tmp/dawn-iso-build-XXXXXX)"
PROFILE="$WORKDIR/profile"
cp -r /usr/share/archiso/configs/releng "$PROFILE"

# The test target disk is a virtio drive with serial=dawn-target, so it
# always resolves to this by-id path regardless of device enumeration
# order — the same reasoning SPEC.md gives for by-id paths in general.
TARGET_DEVICE="/dev/disk/by-id/virtio-dawn-target"

mkdir -p "$PROFILE/airootfs/root"
jq --arg device "$TARGET_DEVICE" '.disk.device = $device' "$PLAN_FILE" \
  > "$PROFILE/airootfs/root/plan.json"

mkdir -p "$PROFILE/airootfs/usr/local/bin"
cp target/release/dawn-backend "$PROFILE/airootfs/usr/local/bin/dawn-backend"
chmod +x "$PROFILE/airootfs/usr/local/bin/dawn-backend"

if [ -n "$MIRROR_SNIPPET" ]; then
  mkdir -p "$PROFILE/airootfs/etc"
  cat "$MIRROR_SNIPPET" >> "$PROFILE/airootfs/etc/pacman.conf"
fi

# A template unit so the target device (passed as the instance name) ends
# up in ExecStart via %I without hardcoding it into the unit file.
mkdir -p "$PROFILE/airootfs/etc/systemd/system" \
  "$PROFILE/airootfs/etc/systemd/system/multi-user.target.wants"
cat > "$PROFILE/airootfs/etc/systemd/system/dawn-e2e-install@.service" <<'EOF'
[Unit]
Description=Dawn e2e test: install plan.json onto the target disk
After=multi-user.target network-online.target
Wants=network-online.target

[Service]
Type=oneshot
# The marker strings are what qemu-run.sh's wait loop watches for on the
# serial console.
ExecStart=/bin/sh -c '/usr/local/bin/dawn-backend --target %I /root/plan.json \
  && echo DAWN-E2E-INSTALL-OK || echo DAWN-E2E-INSTALL-FAILED'
ExecStartPost=/usr/bin/systemctl poweroff
StandardOutput=tty
StandardError=tty
TTYPath=/dev/ttyS0

[Install]
WantedBy=multi-user.target
EOF

# Instantiate it for the real target device (systemd escapes the
# instance name for the unit filename; %I in ExecStart above gives it
# back unescaped).
ESCAPED_DEVICE="$(systemd-escape "$TARGET_DEVICE")"
ln -sf "../dawn-e2e-install@.service" \
  "$PROFILE/airootfs/etc/systemd/system/multi-user.target.wants/dawn-e2e-install@${ESCAPED_DEVICE}.service"

mkarchiso -v -w "$WORKDIR/work" -o "$WORKDIR/out" "$PROFILE"
cp "$WORKDIR"/out/*.iso "$OUTPUT_ISO"
rm -rf "$WORKDIR"
