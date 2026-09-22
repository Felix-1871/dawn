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
# infrastructure), so this mirrors plain Arch's `base` group as a v1
# stand-in — enough to prove the pipeline installs and boots. Swap
# PACKAGES below for the real meta-packages once luminos-repository
# exists; see DECISIONS.md.
#
# Usage: setup-local-mirror.sh <output-dir>
# Serves the repo over HTTP on port 8080 in the background; prints the
# server's PID to stdout on the last line so the caller can stop it.

set -euo pipefail

OUT_DIR="${1:?usage: setup-local-mirror.sh <output-dir>}"
PACKAGES=(base linux mkinitcpio)
REPO_NAME="dawnlocal"

mkdir -p "$OUT_DIR"
pacman -Sy --noconfirm

echo "==> Downloading packages into the local mirror (one-time, from the real Arch mirror)" >&2
pacman -Syw --noconfirm --cachedir "$OUT_DIR" "${PACKAGES[@]}"

echo "==> Building the repo database" >&2
repo-add "$OUT_DIR/$REPO_NAME.db.tar.gz" "$OUT_DIR"/*.pkg.tar.*

echo "==> Serving $OUT_DIR on :8080" >&2
(cd "$OUT_DIR" && python3 -m http.server 8080 >/tmp/dawn-local-mirror.log 2>&1 &)
sleep 1
SERVER_PID="$(pgrep -f 'http.server 8080' | head -1)"

cat > "$OUT_DIR/pacman.conf.snippet" <<EOF
[$REPO_NAME]
SigLevel = Optional TrustAll
Server = http://10.0.2.2:8080
EOF

echo "$SERVER_PID"
