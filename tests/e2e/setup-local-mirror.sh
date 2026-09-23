#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds the pinned local package mirror that every e2e test installs
# from — CLAUDE.md: "Never point tests at public Arch or LuminOS
# mirrors; use the pinned local mirror." Downloading the packages here,
# once, from the real Arch mirror is how that local mirror gets built in
# the first place; nothing downstream (pacstrap inside the test VM/loop
# device) ever sees a public mirror URL.
#
# luminos-base and luminos-desktop don't exist yet (SPEC.md's "Changes
# outside Dawn" lists luminos-repository as separate, not-yet-built
# infrastructure), so this mirrors plain Arch stand-ins for them: `base`
# and a kernel, plus the parts of those meta-packages the pipeline itself
# relies on — NetworkManager and greetd for step 12's `systemctl enable`,
# sudo for step 8's drop-in, zram-generator for step 7's config. Swap
# PACKAGES below for the real meta-packages once luminos-repository
# exists; see DECISIONS.md.
#
# Usage: setup-local-mirror.sh <output-dir> [server-host]
# Besides the repo itself, <output-dir> gets two files for the tests:
#   pacman.conf    a complete pacman config listing this mirror and
#                  nothing else, for pacstrap's -C
#   packages.json  PACKAGES as a JSON array, for a plan's "packages"
# Serves the repo over HTTP on port 8080 in the background and writes the
# server's PID to <output-dir>/http-server.pid so the caller can stop it.
# Run this directly, never inside $(...): a command substitution only
# returns once every process holding its stdout has exited, and the
# server is meant to outlive this script.
#
# server-host is what pacman.conf tells clients to connect to,
# and it depends on who's connecting: a plain container reaches this
# same host at 127.0.0.1 (the default), while a QEMU guest must instead
# use 10.0.2.2, SLIRP's address for the host running QEMU.

set -euo pipefail

OUT_DIR="${1:?usage: setup-local-mirror.sh <output-dir> [server-host]}"
SERVER_HOST="${2:-127.0.0.1}"
PACKAGES=(base linux mkinitcpio networkmanager greetd sudo zram-generator)
REPO_NAME="dawnlocal"

mkdir -p "$OUT_DIR"
# Recent pacman sandboxes downloads under a dedicated unprivileged user
# (pacman.conf's DownloadUser, set by default in the archlinux Docker
# image) — it needs write access to our cachedir, which mktemp -d
# otherwise creates mode 700 owned by root.
chmod 777 "$OUT_DIR"

# Resolve against an empty, throwaway package database. Against this
# host's own, -w skips every dependency the host already has installed,
# and a from-scratch pacstrap from this mirror then fails on them.
DB_DIR="$(mktemp -d)"
trap 'rm -rf "$DB_DIR"' EXIT
chmod 755 "$DB_DIR"

echo "==> Downloading packages into the local mirror (one-time, from the real Arch mirror)" >&2
pacman -Syw --noconfirm --dbpath "$DB_DIR" --cachedir "$OUT_DIR" "${PACKAGES[@]}"

echo "==> Building the repo database" >&2
shopt -s nullglob
PACKAGE_FILES=("$OUT_DIR"/*.pkg.tar.zst "$OUT_DIR"/*.pkg.tar.xz)
repo-add "$OUT_DIR/$REPO_NAME.db.tar.gz" "${PACKAGE_FILES[@]}"

# A whole config, not a snippet to append: an appended repo still leaves
# the host's [core] and [extra] ahead of it, and pacstrap would quietly
# take every package from those public mirrors instead of this one.
cat > "$OUT_DIR/pacman.conf" <<EOF
[options]
Architecture = auto

[$REPO_NAME]
SigLevel = Optional TrustAll
Server = http://$SERVER_HOST:8080
EOF

jq -n '$ARGS.positional' --args "${PACKAGES[@]}" > "$OUT_DIR/packages.json"

echo "==> Serving $OUT_DIR on :8080" >&2
# Started directly, with every stream redirected, so the server never
# holds the caller's stdout or stderr open. Anything waiting for those to
# close (a $(...) capture, or the `docker exec` behind a CI step) would
# otherwise block for as long as the server runs.
python3 -m http.server 8080 --directory "$OUT_DIR" \
  >"$OUT_DIR/http-server.log" 2>&1 </dev/null &
echo "$!" > "$OUT_DIR/http-server.pid"

for _ in $(seq 30); do
  if curl -fs -o /dev/null "http://127.0.0.1:8080/$REPO_NAME.db"; then
    exit 0
  fi
  sleep 1
done
echo "error: the local mirror never answered on :8080" >&2
cat "$OUT_DIR/http-server.log" >&2
exit 1
