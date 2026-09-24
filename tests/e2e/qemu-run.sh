#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Drives QEMU + OVMF for the end-to-end test (SPEC.md "Testing", the
# "End-to-end" row): headless, serial console only, watching for a
# marker string within a timeout instead of a human at the screen.
# Needs /dev/kvm and OVMF firmware. CLAUDE.md: check for these before
# any VM step, and don't improvise if they're missing.
#
# Usage:
#   qemu-run.sh install <iso> <target-qcow2> <scenario> <serial-log> <timeout-seconds>
#     Boots <iso> with <target-qcow2> as a second disk (virtio,
#     serial=dawn-target), passes <scenario> to the ISO's
#     dawn-e2e.service as the SMBIOS credential dawn.scenario, and waits
#     for gui_driver's DAWN-E2E-INSTALL-OK on the serial console.
#   qemu-run.sh verify-login <target-qcow2> <serial-log> <timeout-seconds>
#     Boots <target-qcow2> alone (no ISO) and waits for a "login:" prompt
#     on the serial console.
#
# The serial console goes to <serial-log>, for run-e2e.sh's own checks.

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
  echo "error: could not find OVMF $name; see CLAUDE.md's VM and test environment section" >&2
  return 1
}

OVMF_CODE="$(find_ovmf OVMF_CODE)"
OVMF_VARS_TEMPLATE="$(find_ovmf OVMF_VARS)"

require_kvm() {
  if [ ! -e /dev/kvm ]; then
    echo "error: /dev/kvm is missing; see CLAUDE.md's VM and test environment section" >&2
    exit 1
  fi
}

# Waits (up to $1 seconds) for $2 to appear in the serial log $3, while
# qemu ($4) runs, then stops qemu.
wait_for_marker() {
  local timeout="$1" marker="$2" log="$3" qemu_pid="$4"
  local waited=0
  while ! grep -q "$marker" "$log" 2>/dev/null; do
    if ! kill -0 "$qemu_pid" 2>/dev/null; then
      # The guest powers off right after printing the marker, so it can
      # land between the check above and this one.
      if grep -q "$marker" "$log" 2>/dev/null; then
        break
      fi
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
  local iso="$1" target="$2" scenario="$3" log="$4" timeout="$5"
  local vars; vars="$(mktemp /tmp/dawn-ovmf-vars-XXXXXX.fd)"
  cp "$OVMF_VARS_TEMPLATE" "$vars"

  # online and offline boot the ISO as a CD, as a DVD or a VM's ISO file
  # would: archiso reads the live image from the medium. For
  # fail-then-offline it's a USB stick, with RAM to spare, so archiso's
  # default copytoram=auto copies the image to RAM and unmounts the stick
  # (it never does for an optical drive). The offline install that
  # follows the failure then has to find the image in
  # /run/archiso/copytoram/, and the Disk screen must not offer the
  # stick (DECISIONS.md, M1: copy-to-RAM).
  local medium memory
  if [ "$scenario" = "fail-then-offline" ]; then
    medium=(-device qemu-xhci
            -drive if=none,id=iso,format=raw,readonly=on,file="$iso"
            -device usb-storage,drive=iso,removable=on,bootindex=0)
    memory=4608
  else
    medium=(-cdrom "$iso")
    memory=2048
  fi

  # serial= goes on the virtio-blk device rather than the -drive: current
  # QEMU rejects it as a drive option. It's what gives the guest its
  # /dev/disk/by-id/virtio-dawn-target link.
  : > "$log"
  qemu-system-x86_64 \
    -machine q35,accel=kvm -cpu host -m "$memory" -no-reboot \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive if=pflash,format=raw,file="$vars" \
    "${medium[@]}" \
    -drive if=none,id=target,format=qcow2,file="$target" \
    -device virtio-blk-pci,drive=target,serial=dawn-target \
    -smbios type=11,value=io.systemd.credential:dawn.scenario="$scenario" \
    -nographic -serial file:"$log" \
    -netdev user,id=net0 -device virtio-net-pci,netdev=net0 \
    &
  local status=0
  wait_for_marker "$timeout" "DAWN-E2E-INSTALL-OK" "$log" $! || status=$?
  rm -f "$vars"
  return "$status"
}

cmd_verify_login() {
  require_kvm
  local target="$1" log="$2" timeout="$3"
  local vars; vars="$(mktemp /tmp/dawn-ovmf-vars-XXXXXX.fd)"
  cp "$OVMF_VARS_TEMPLATE" "$vars"

  # Fresh OVMF variables hold no boot entries, so bootindex=0 points the
  # firmware straight at the disk's fallback bootloader, rather than
  # risking network boot attempts on QEMU's default network card eating
  # into the timeout.
  : > "$log"
  qemu-system-x86_64 \
    -machine q35,accel=kvm -cpu host -m 2048 -no-reboot \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive if=pflash,format=raw,file="$vars" \
    -drive if=none,id=target,format=qcow2,file="$target" \
    -device virtio-blk-pci,drive=target,serial=dawn-target,bootindex=0 \
    -nographic -serial file:"$log" \
    &
  local status=0
  wait_for_marker "$timeout" "login:" "$log" $! || status=$?
  rm -f "$vars"
  return "$status"
}

case "${1:-}" in
  install) shift; cmd_install "$@" ;;
  verify-login) shift; cmd_verify_login "$@" ;;
  *)
    echo "usage: qemu-run.sh install <iso> <target-qcow2> <scenario> <serial-log> <timeout-seconds>" >&2
    echo "       qemu-run.sh verify-login <target-qcow2> <serial-log> <timeout-seconds>" >&2
    exit 2
    ;;
esac
