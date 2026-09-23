#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds the stand-in luminos-dawn and luminos-keyring packages
# (stand-ins/*/PKGBUILD) into a local pacman repo named dawn-e2e, for
# build-test-iso.sh to install into the test ISO. Must run as root inside
# a disposable container, like the rest of tests/e2e: makepkg refuses to
# run as root, so this creates a throwaway build user to run it as.
#
# Usage: build-stand-in-packages.sh <dawn-backend> <plan.json> <repo-dir>

set -euo pipefail

USAGE="usage: build-stand-in-packages.sh <dawn-backend> <plan.json> <repo-dir>"
BACKEND="${1:?$USAGE}"
PLAN_FILE="${2:?$USAGE}"
REPO_DIR="${3:?$USAGE}"
STAND_INS="$(dirname "$0")/stand-ins"
BUILD_USER="dawn-e2e-build"

id -u "$BUILD_USER" >/dev/null 2>&1 || useradd --system --no-create-home "$BUILD_USER"

BUILD_DIR="$(mktemp -d)"
KEY_HOME="$(mktemp -d)"
cleanup() {
  gpgconf --homedir "$KEY_HOME" --kill all 2>/dev/null || true
  rm -rf "$BUILD_DIR" "$KEY_HOME"
}
trap cleanup EXIT

cp -r "$STAND_INS/luminos-dawn" "$STAND_INS/luminos-keyring" "$BUILD_DIR/"
cp "$BACKEND" "$BUILD_DIR/luminos-dawn/dawn-backend"
cp "$PLAN_FILE" "$BUILD_DIR/luminos-dawn/plan.json"

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
for pkg in luminos-dawn luminos-keyring; do
  (cd "$BUILD_DIR/$pkg" && runuser -u "$BUILD_USER" -- makepkg --nodeps --noconfirm)
  cp "$BUILD_DIR/$pkg"/*.pkg.tar.zst "$REPO_DIR/"
done
repo-add "$REPO_DIR/dawn-e2e.db.tar.gz" "$REPO_DIR"/*.pkg.tar.zst
