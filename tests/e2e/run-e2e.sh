#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# M3's Done-when check: "Dawn installs a VM end to end from the GUI,
# online and offline; a forced failure shows the error screen with log",
# and M4's: "In OVMF Setup Mode, the installed system boots with Secure
# Boot enforcing." Each scenario boots the test ISO in QEMU, where
# gui_driver clicks through Dawn's real GUI (DECISIONS.md, M3), then
# boots the installed disk and logs in on its serial console as the new
# user, with the password given in the GUI. Root, /dev/kvm, and a
# disposable environment required; see CLAUDE.md.
#
# Usage:
#   run-e2e.sh prepare
#     Builds dawn-backend and gui_driver, the local mirror and the test
#     ISO, once, for every scenario.
#   run-e2e.sh <online|offline|fail-then-offline|secure-boot>
#     online: the mirror answers, so the GUI skips the Network screen
#       and pacstrap installs from the mirror.
#     offline: no mirror answers, so the Network screen shows; the
#       driver skips it and the install unsquashes the live image.
#     fail-then-offline: the mirror serves its databases but no
#       packages, so pacstrap fails in step 5. The error screen must
#       show the step, its log and the offline option, which then
#       installs from the image archiso copied to RAM.
#     secure-boot: online, on OVMF's Secure Boot build in Setup Mode,
#       keeping the GUI's Secure Boot checkbox; the installed system must
#       then boot with Secure Boot enforcing.
#
# Works in DAWN_E2E_WORK (default /tmp/dawn-e2e); its serial logs stay
# there for CI to upload.

set -euo pipefail
cd "$(dirname "$0")/../.."

E2E_DIR="tests/e2e"
WORK="${DAWN_E2E_WORK:-/tmp/dawn-e2e}"
ISO="$WORK/dawn-e2e.iso"
MIRROR="$WORK/mirror"
MIRROR_DB_ONLY="$WORK/mirror-db-only"
SERVER_PID="$WORK/http-server.pid"
# Answers::fixture's user in frontend/tests/driver/mod.rs. The password
# is never printed, whatever else the install shows.
USERNAME="ada"
PASSWORD="correct-horse-battery-staple"

stop_mirror() {
  if [ -f "$SERVER_PID" ]; then
    kill "$(cat "$SERVER_PID")" 2>/dev/null || true
    rm -f "$SERVER_PID"
  fi
}

prepare() {
  rm -rf "$WORK"
  mkdir -p "$WORK"
  cargo build --release -p backend
  cargo build --release -p frontend --example gui_driver

  # 10.0.2.2 is SLIRP's address for the host running QEMU; the ISO's
  # pacman.conf points there. Only the files are wanted here: each
  # scenario serves what it needs.
  trap 'kill "$(cat "$MIRROR/http-server.pid" 2>/dev/null)" 2>/dev/null || true' EXIT
  "$E2E_DIR/setup-local-mirror.sh" "$MIRROR" 10.0.2.2
  kill "$(cat "$MIRROR/http-server.pid")"
  trap - EXIT

  # What fail-then-offline serves: the databases, so the repositories
  # look reachable to step 1, but none of the packages pacstrap then
  # asks for.
  mkdir -p "$MIRROR_DB_ONLY"
  cp -L "$MIRROR"/dawnlocal.db* "$MIRROR_DB_ONLY/"

  "$E2E_DIR/build-test-iso.sh" "$MIRROR" "$ISO"
}

fail() {
  echo "error: $*" >&2
  exit 1
}

run_scenario() {
  local scenario="$1"
  [ -f "$ISO" ] || fail "no test ISO in $WORK; run '$0 prepare' first"
  local install_log="$WORK/install-$scenario.log"
  local login_log="$WORK/login-$scenario.log"
  local target="$WORK/target-$scenario.qcow2"
  # A fresh variable store, so the firmware starts in Setup Mode.
  local vars="$WORK/vars-$scenario.fd"
  rm -f "$vars"

  trap stop_mirror EXIT
  case "$scenario" in
    online|secure-boot) "$E2E_DIR/serve-mirror.sh" "$MIRROR" "$SERVER_PID" ;;
    fail-then-offline) "$E2E_DIR/serve-mirror.sh" "$MIRROR_DB_ONLY" "$SERVER_PID" ;;
    offline) ;;
  esac

  qemu-img create -f qcow2 "$target" 40G
  echo "==> Installing ($scenario) through the GUI" >&2
  "$E2E_DIR/qemu-run.sh" install "$ISO" "$target" "$vars" "$scenario" "$install_log" 1800
  stop_mirror

  # The stand-in luminos-live was really installed and running on the
  # live system, so its absence later means something.
  grep -q "DAWN-E2E-LIVE-ONLY-UNIT-RAN" "$install_log" \
    || fail "the live-only unit never ran on the live system"
  if [ "$scenario" = "fail-then-offline" ]; then
    grep -q "DAWN-E2E-ERROR-SCREEN-OK" "$install_log" \
      || fail "the error screen was never checked"
  fi
  if grep -q "$PASSWORD" "$install_log"; then
    fail "the user's password showed on the console"
  fi

  echo "==> Booting the installed disk and logging in as $USERNAME" >&2
  local report
  report="$("$E2E_DIR/qemu-run.sh" verify-login "$target" "$vars" "$scenario" \
    "$login_log" "$USERNAME" "$PASSWORD" 300)"
  echo "$report" >&2
  case "$report" in
    DAWN-E2E-LOGGED-IN*) ;;
    *) fail "logging in on the installed system printed no report" ;;
  esac
  if [ "$scenario" = "secure-boot" ] && [ "$report" != "DAWN-E2E-LOGGED-IN secure_boot=1" ]; then
    fail "the installed system didn't boot with Secure Boot enforcing"
  fi
  # Online installs never had it; offline ones must have removed it
  # (installer.toml's [offline_cleanup] remove_packages).
  if grep -q "DAWN-E2E-LIVE-ONLY-UNIT-RAN" "$login_log"; then
    fail "the installed system still runs luminos-live's units"
  fi

  echo "==> $scenario: installed through the GUI; $USERNAME logged in on the installed system" >&2
  rm -f "$target" "$vars"
}

case "${1:-}" in
  prepare) prepare ;;
  online|offline|fail-then-offline|secure-boot) run_scenario "$1" ;;
  *)
    echo "usage: run-e2e.sh prepare | online | offline | fail-then-offline | secure-boot" >&2
    exit 2
    ;;
esac
