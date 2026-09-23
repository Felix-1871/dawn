#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# M1's own Done-when check: "A plan file installs into a QEMU VM disk
# both online and offline, and the VM boots to a login prompt." Ties
# together setup-local-mirror.sh, build-test-iso.sh and qemu-run.sh.
# Root, /dev/kvm, and a disposable environment required — see CLAUDE.md.
#
# Usage: run-e2e.sh <online|offline>

set -euo pipefail
cd "$(dirname "$0")/../.."

MODE="${1:?usage: run-e2e.sh <online|offline>}"
E2E_DIR="tests/e2e"
PLAN_TEMPLATE="tests/plans/erase-$MODE.json"

cargo build --release -p backend

MIRROR_DIR=""
if [ "$MODE" = "online" ]; then
  MIRROR_DIR="$(mktemp -d /tmp/dawn-mirror-XXXXXX)"
  trap 'kill "$(cat "$MIRROR_DIR/http-server.pid" 2>/dev/null)" 2>/dev/null || true' EXIT
  # 10.0.2.2 is SLIRP's address for the host running QEMU, reachable
  # from the guest that build-test-iso.sh bakes the mirror's
  # pacman.conf into.
  "$E2E_DIR/setup-local-mirror.sh" "$MIRROR_DIR" 10.0.2.2
fi

ISO="$(mktemp -u /tmp/dawn-e2e-XXXXXX.iso)"
"$E2E_DIR/build-test-iso.sh" "$PLAN_TEMPLATE" "$ISO" "$MIRROR_DIR"

TARGET_DISK="$(mktemp -u /tmp/dawn-e2e-target-XXXXXX.qcow2)"
qemu-img create -f qcow2 "$TARGET_DISK" 40G

echo "==> Installing ($MODE) into the target VM disk" >&2
"$E2E_DIR/qemu-run.sh" install "$ISO" "$TARGET_DISK" 1800

echo "==> Booting the installed disk and waiting for a login prompt" >&2
"$E2E_DIR/qemu-run.sh" verify-login "$TARGET_DISK" 300

echo "==> $MODE end-to-end install verified" >&2
rm -f "$ISO" "$TARGET_DISK"
