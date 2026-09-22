#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Drives QEMU + OVMF for the end-to-end test (SPEC.md "Testing", the
# "End-to-end" row): headless, serial console only, watching for a
# marker string within a timeout instead of a human at the screen.
# Needs /dev/kvm and OVMF firmware — CLAUDE.md: check for these before
# any VM step, and don't improvise if they're missing.
#
# Usage:
#   qemu-run.sh install <iso> <target-qcow2> <timeout-seconds>
#     Boots <iso> with <target-qcow2> as a second (virtio, serial=dawn-target)
#     disk, waits for DAWN-E2E-INSTALL-OK on the serial console.
#   qemu-run.sh verify-login <target-qcow2> <timeout-seconds>
#     Boots <target-qcow2> alone (no ISO), waits for a "login:" prompt
#     on the serial console — this is M1's actual Done-when check.

set -euo pipefail

# OVMF's install path differs by distro: Arch's edk2-ovmf vs. Ubuntu/
# Debian's ovmf package. Try both rather than assuming one.
find_ovmf() {
  local name="$1"
  for candidate in \
    "/usr/share/edk2/x64/${name}.4m.fd" \
    "/usr/share/OVMF/${name}.fd" \
    "/usr/share/ovmf/x64/${name}.fd"
  do
    if [ -f "$candidate" ]; then
      echo "$candidate"
      return 0
    fi
  done
  echo "error: could not find OVMF $name — see CLAUDE.md's VM and test environment section" >&2
  return 1
}

OVMF_CODE="$(find_ovmf OVMF_CODE)"
OVMF_VARS_TEMPLATE="$(find_ovmf OVMF_VARS)"

require_kvm() {
  if [ ! -e /dev/kvm ]; then
    echo "error: /dev/kvm is missing — see CLAUDE.md's VM and test environment section" >&2
    exit 1
  fi
}

# Runs qemu in the background, tees the serial console to a log file,
# and waits (up to $1 seconds) for $2 to appear in it.
wait_for_marker() {
  local timeout="$1" marker="$2" log="$3" qemu_pid="$4"
  local waited=0
  while ! grep -q "$marker" "$log" 2>/dev/null; do
    if ! kill -0 "$qemu_pid" 2>/dev/null; then
      echo "error: qemu exited before printing '$marker'" >&2
      cat "$log" >&2
      return 1
    fi
    if [ "$waited" -ge "$timeout" ]; then
      echo "error: timed out after ${timeout}s waiting for '$marker'" >&2
      cat "$log" >&2
      kill "$qemu_pid" 2>/dev/null || true
      return 1
    fi
    sleep 2
    waited=$((waited + 2))
  done
  kill "$qemu_pid" 2>/dev/null || true
  wait "$qemu_pid" 2>/dev/null || true
}

cmd_install() {
  require_kvm
  local iso="$1" target="$2" timeout="$3"
  local log; log="$(mktemp /tmp/dawn-qemu-install-XXXXXX.log)"
  local vars; vars="$(mktemp /tmp/dawn-ovmf-vars-XXXXXX.fd)"
  cp "$OVMF_VARS_TEMPLATE" "$vars"

  qemu-system-x86_64 \
    -machine q35,accel=kvm -cpu host -m 2048 -no-reboot \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive if=pflash,format=raw,file="$vars" \
    -cdrom "$iso" \
    -drive if=virtio,format=qcow2,file="$target",serial=dawn-target \
    -nographic -serial file:"$log" \
    -netdev user,id=net0 -device virtio-net-pci,netdev=net0 \
    &
  wait_for_marker "$timeout" "DAWN-E2E-INSTALL-OK" "$log" $!
  local status=$?
  rm -f "$vars"
  return "$status"
}

cmd_verify_login() {
  require_kvm
  local target="$1" timeout="$2"
  local log; log="$(mktemp /tmp/dawn-qemu-login-XXXXXX.log)"
  local vars; vars="$(mktemp /tmp/dawn-ovmf-vars-XXXXXX.fd)"
  cp "$OVMF_VARS_TEMPLATE" "$vars"

  qemu-system-x86_64 \
    -machine q35,accel=kvm -cpu host -m 2048 -no-reboot \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive if=pflash,format=raw,file="$vars" \
    -drive if=virtio,format=qcow2,file="$target",serial=dawn-target \
    -nographic -serial file:"$log" \
    &
  wait_for_marker "$timeout" "login:" "$log" $!
  local status=$?
  rm -f "$vars"
  return "$status"
}

case "${1:-}" in
  install) shift; cmd_install "$@" ;;
  verify-login) shift; cmd_verify_login "$@" ;;
  *)
    echo "usage: qemu-run.sh install <iso> <target-qcow2> <timeout-seconds>" >&2
    echo "       qemu-run.sh verify-login <target-qcow2> <timeout-seconds>" >&2
    exit 2
    ;;
esac
