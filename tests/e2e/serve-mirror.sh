#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Serves a directory over HTTP on port 8080 in the background, as the
# local package mirror, and writes the server's PID to <pid-file> so the
# caller can stop it. Returns once the server answers.
#
# Usage: serve-mirror.sh <dir> <pid-file>
#
# Run this directly, never inside $(...): a command substitution only
# returns once every process holding its stdout has exited, and the
# server is meant to outlive this script.

set -euo pipefail

DIR="${1:?usage: serve-mirror.sh <dir> <pid-file>}"
PID_FILE="${2:?usage: serve-mirror.sh <dir> <pid-file>}"
LOG="$PID_FILE.log"

echo "==> Serving $DIR on :8080" >&2
# Started directly, with every stream redirected, so the server never
# holds the caller's stdout or stderr open. Anything waiting for those to
# close (a $(...) capture, or the `docker exec` behind a CI step) would
# otherwise block for as long as the server runs.
python3 -m http.server 8080 --directory "$DIR" >"$LOG" 2>&1 </dev/null &
echo "$!" > "$PID_FILE"

for _ in $(seq 30); do
  # Any HTTP answer will do, even a 404 for a directory without an index.
  if curl -s -o /dev/null "http://127.0.0.1:8080/"; then
    exit 0
  fi
  sleep 1
done
echo "error: nothing answered on :8080" >&2
cat "$LOG" >&2
exit 1
