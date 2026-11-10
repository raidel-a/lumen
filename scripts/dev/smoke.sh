#!/usr/bin/env bash
# End-to-end smoke test against a disposable support directory.
#
# It opens every settings page, starts a headless agent on a temporary virtual screen, captures
# the page, stops the agent, removes the screen, and quits cleanly. It never touches the installed
# app's support directory.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
support="${LUMEN_SMOKE_SUPPORT:-$HOME/Library/Application Support/Lumen Smoke}"
export LUMEN_DEV_SUPPORT="$support"
cli="$root/target/debug/lumen"
pidfile="$root/dist/lumen-dev.pid"

fail() {
  echo "smoke: $*" >&2
  exit 1
}

if [[ -f "$pidfile" ]] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then
  fail "a dev instance is already running; stop it first"
fi

rm -rf "$support"
trap 'LUMEN_DEV_SUPPORT="$support" "$root/scripts/dev/stop.sh" >/dev/null 2>&1 || true' EXIT

"$root/scripts/dev/run.sh" >/dev/null
pid="$(cat "$pidfile")"
echo "smoke: app pid $pid"

for page in general displays virtual about; do
  LUMEN_SUPPORT_DIR="$support" "$cli" settings --page "$page" --screen agent >/dev/null
  state="$(LUMEN_SUPPORT_DIR="$support" "$cli" settings state)"
  echo "$state" | grep -q "\"page\": \"$page\"" || fail "page $page did not open"
  echo "$state" | grep -q '"visible": true' || fail "settings window is not visible"
  echo "smoke: page $page"
done

name="Smoke Screen"
LUMEN_SUPPORT_DIR="$support" "$cli" virtual ensure --name "$name" --width 1280 --height 720 >/dev/null \
  || fail "could not create the smoke screen"
LUMEN_SUPPORT_DIR="$support" "$cli" agent start --name "$name" --headless >/dev/null \
  || fail "agent did not start"
status="$(LUMEN_SUPPORT_DIR="$support" "$cli" agent status --name "$name")"
echo "$status" | grep -q '"alive": true' || fail "agent is not alive"
echo "$status" | grep -q '"headless": true' || fail "agent is not headless"
capture="$support/smoke-page.png"
LUMEN_SUPPORT_DIR="$support" "$cli" agent screenshot --name "$name" --out "$capture" >/dev/null \
  || fail "agent screenshot failed"
[[ -s "$capture" ]] || fail "agent screenshot is empty"
echo "smoke: agent capture $(stat -f%z "$capture") bytes"

LUMEN_SUPPORT_DIR="$support" "$cli" virtual preview --name "$name" >/dev/null \
  || fail "preview did not open"
previews="$(LUMEN_SUPPORT_DIR="$support" "$cli" virtual previews)"
echo "$previews" | grep -q "\"$name\"" || fail "preview is not listed"
LUMEN_SUPPORT_DIR="$support" "$cli" virtual preview --name "$name" --hide >/dev/null \
  || fail "preview did not close"
previews="$(LUMEN_SUPPORT_DIR="$support" "$cli" virtual previews)"
if echo "$previews" | grep -q "\"$name\""; then
  fail "preview is still listed"
fi
echo "smoke: preview open and close"

LUMEN_SUPPORT_DIR="$support" "$cli" agent stop --name "$name" >/dev/null || fail "agent did not stop"
LUMEN_SUPPORT_DIR="$support" "$cli" virtual remove --name "$name" >/dev/null || fail "screen did not remove"
echo "smoke: ok"
