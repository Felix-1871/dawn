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
#   qemu-run.sh install <iso> <target-qcow2> <vars> <scenario> <serial-log> <timeout-seconds>
#     Boots <iso> with <target-qcow2> as a second disk (virtio,
#     serial=dawn-target), passes <scenario> to the ISO's
#     dawn-e2e.service as the SMBIOS credential dawn.scenario, and waits
#     for gui_driver's DAWN-E2E-INSTALL-OK on the serial console.
#   qemu-run.sh verify-login <target-qcow2> <vars> <scenario> <serial-log> <user> <password> <timeout-seconds>
#     Boots <target-qcow2> alone (no ISO), logs in as <user> on the
#     serial console (serial-login.py) and prints its report line.
#
# <vars> is the VM's UEFI variable store, created from OVMF's empty one
# (so in Setup Mode) when it doesn't exist yet. Passing the same file to
# both commands boots the installed system with whatever the install
# wrote there: its boot entry and, for secure-boot, the enrolled keys.
# The secure-boot scenario runs OVMF's Secure Boot build, the others its
# plain one. The serial console goes to <serial-log>, for run-e2e.sh's
# own checks.

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

# The Secure Boot build is named differently again.
find_ovmf_secboot() {
  for candidate in \
    /usr/share/edk2/x64/OVMF_CODE.secboot.4m.fd \
    /usr/share/OVMF/OVMF_CODE_4M.secboot.fd \
    /usr/share/OVMF/OVMF_CODE.secboot.fd
  do
    if [ -f "$candidate" ]; then
      echo "$candidate"
      return 0
    fi
  done
  echo "error: could not find OVMF's Secure Boot build; see CLAUDE.md's VM and test environment section" >&2
  return 1
}

OVMF_VARS_TEMPLATE="$(find_ovmf OVMF_VARS)"

# Sets FIRMWARE to the qemu arguments for <scenario>'s firmware with
# <vars> as its variable store, creating the store if needed. Secure
# Boot OVMF needs SMM, and its variables flash marked secure, so only
# firmware code can write them.
firmware_for() {
  local scenario="$1" vars="$2"
  [ -f "$vars" ] || cp "$OVMF_VARS_TEMPLATE" "$vars"
  if [ "$scenario" = "secure-boot" ]; then
    FIRMWARE=(-machine q35,smm=on,accel=kvm
              -global driver=cfi.pflash01,property=secure,value=on
              -global ICH9-LPC.disable_s3=1
              -drive if=pflash,format=raw,readonly=on,file="$(find_ovmf_secboot)"
              -drive if=pflash,format=raw,file="$vars")
  else
    FIRMWARE=(-machine q35,accel=kvm
              -drive if=pflash,format=raw,readonly=on,file="$(find_ovmf OVMF_CODE)"
              -drive if=pflash,format=raw,file="$vars")
  fi
}

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
  local iso="$1" target="$2" vars="$3" scenario="$4" log="$5" timeout="$6"
  firmware_for "$scenario" "$vars"

  # The other scenarios boot the ISO as a CD, as a DVD or a VM's ISO file
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
    "${FIRMWARE[@]}" -cpu host -m "$memory" -no-reboot \
    "${medium[@]}" \
    -drive if=none,id=target,format=qcow2,file="$target" \
    -device virtio-blk-pci,drive=target,serial=dawn-target \
    -smbios type=11,value=io.systemd.credential:dawn.scenario="$scenario" \
    -nographic -serial file:"$log" \
    -netdev user,id=net0 -device virtio-net-pci,netdev=net0 \
    &
  wait_for_marker "$timeout" "DAWN-E2E-INSTALL-OK" "$log" $!
}

cmd_verify_login() {
  require_kvm
  local target="$1" vars="$2" scenario="$3" log="$4" user="$5" password="$6" timeout="$7"
  firmware_for "$scenario" "$vars"
  local socket; socket="$(mktemp -u /tmp/dawn-serial-XXXXXX.sock)"

  # bootindex=0 points the firmware straight at the disk's bootloader if
  # the variables hold no boot entry, rather than risking network boot
  # attempts on QEMU's default network card eating into the timeout. The
  # serial console is a socket serial-login.py types into, and it's
  # logged to <serial-log> as well.
  : > "$log"
  qemu-system-x86_64 \
    "${FIRMWARE[@]}" -cpu host -m 2048 -no-reboot \
    -drive if=none,id=target,format=qcow2,file="$target" \
    -device virtio-blk-pci,drive=target,serial=dawn-target,bootindex=0 \
    -chardev socket,id=console,path="$socket",server=on,wait=off,logfile="$log" \
    -serial chardev:console -display none -monitor none -parallel none \
    >&2 &
  local qemu_pid=$!
  local status=0
  python3 "$(dirname "$0")/serial-login.py" "$socket" "$user" "$password" "$timeout" \
    || status=$?
  kill "$qemu_pid" 2>/dev/null || true
  wait "$qemu_pid" 2>/dev/null || true
  rm -f "$socket"
  if [ "$status" -ne 0 ]; then
    cat "$log" >&2
  fi
  return "$status"
}

case "${1:-}" in
  install) shift; cmd_install "$@" ;;
  verify-login) shift; cmd_verify_login "$@" ;;
  *)
    echo "usage: qemu-run.sh install <iso> <target-qcow2> <vars> <scenario> <serial-log> <timeout-seconds>" >&2
    echo "       qemu-run.sh verify-login <target-qcow2> <vars> <scenario> <serial-log> <user> <password> <timeout-seconds>" >&2
    exit 2
    ;;
esac
