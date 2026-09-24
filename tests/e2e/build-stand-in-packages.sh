#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds the stand-in luminos-dawn, luminos-live and luminos-keyring
# packages (stand-ins/*/PKGBUILD) into a local pacman repo named
# dawn-e2e, for build-test-iso.sh to install into the test ISO. Must run
# as root inside a disposable container, like the rest of tests/e2e:
# makepkg refuses to run as root, so this creates a throwaway build user
# to run it as.
#
# Usage: build-stand-in-packages.sh <dawn-backend> <gui_driver> <installer.toml> <live-only-dir> <repo-dir>
#
# <live-only-dir> holds the live ISO's live-only files, laid out as they
# install; they become the stand-in luminos-live.

set -euo pipefail

USAGE="usage: build-stand-in-packages.sh <dawn-backend> <gui_driver> <installer.toml> <live-only-dir> <repo-dir>"
BACKEND="${1:?$USAGE}"
DRIVER="${2:?$USAGE}"
CONFIG="${3:?$USAGE}"
LIVE_ONLY="${4:?$USAGE}"
REPO_DIR="${5:?$USAGE}"
STAND_INS="$(dirname "$0")/stand-ins"
POLKIT="$(dirname "$0")/../../config/polkit"
BUILD_USER="dawn-e2e-build"

id -u "$BUILD_USER" >/dev/null 2>&1 || useradd --system --no-create-home "$BUILD_USER"

BUILD_DIR="$(mktemp -d)"
KEY_HOME="$(mktemp -d)"
cleanup() {
  gpgconf --homedir "$KEY_HOME" --kill all 2>/dev/null || true
  rm -rf "$BUILD_DIR" "$KEY_HOME"
}
trap cleanup EXIT

cp -r "$STAND_INS/luminos-dawn" "$STAND_INS/luminos-live" "$STAND_INS/luminos-keyring" "$BUILD_DIR/"
cp "$BACKEND" "$BUILD_DIR/luminos-dawn/dawn-backend"
cp "$DRIVER" "$BUILD_DIR/luminos-dawn/gui_driver"
cp "$CONFIG" "$BUILD_DIR/luminos-dawn/installer.toml"
cp "$POLKIT/org.luminos.dawn.policy" "$POLKIT/50-luminos-dawn.rules" "$BUILD_DIR/luminos-dawn/"
cp -a "$LIVE_ONLY" "$BUILD_DIR/luminos-live/live-only"

# The keyring's public key file and trust entry are all
# `pacman-key --populate` needs; the private half is thrown away with
# KEY_HOME.
gpg --homedir "$KEY_HOME" --batch --pinentry-mode loopback --passphrase '' \
  --quick-gen-key 'LuminOS e2e throwaway key <e2e@luminos.invalid>' ed25519 sign never
gpg --homedir "$KEY_HOME" --export > "$BUILD_DIR/luminos-keyring/luminos.gpg"
FINGERPRINT="$(gpg --homedir "$KEY_HOME" --with-colons --list-keys \
  | awk -F: '$1 == "fpr" { print $10; exit }')"
echo "$FINGERPRINT:4:" > "$BUILD_DIR/luminos-keyring/luminos-trusted"

chown -R "$BUILD_USER" "$BUILD_DIR"
mkdir -p "$REPO_DIR"
for pkg in luminos-dawn luminos-live luminos-keyring; do
  (cd "$BUILD_DIR/$pkg" && runuser -u "$BUILD_USER" -- makepkg --nodeps --noconfirm)
  cp "$BUILD_DIR/$pkg"/*.pkg.tar.zst "$REPO_DIR/"
done
repo-add "$REPO_DIR/dawn-e2e.db.tar.gz" "$REPO_DIR"/*.pkg.tar.zst
