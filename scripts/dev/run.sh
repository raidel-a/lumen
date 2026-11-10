#!/usr/bin/env bash
# Builds and launches the dev bundle through LaunchServices so accessibility and bundle identity
# work, with a dedicated support directory selected by --support-dir.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
support="${LUMEN_DEV_SUPPORT:-$HOME/Library/Application Support/Lumen Dev}"
pidfile="$root/dist/lumen-dev.pid"

if [[ -f "$pidfile" ]]; then
  pid="$(cat "$pidfile")"
  if kill -0 "$pid" 2>/dev/null; then
    echo "dev instance already running (pid $pid)" >&2
    exit 1
  fi
  rm -f "$pidfile"
fi

app="$("$root/scripts/dev/bundle.sh" | tail -1)"
mkdir -p "$support"

existing="$(ps -axo pid=,command= | awk -v app="$app" -v support="$support" \
  'index($0, app "/Contents/MacOS/lumen") && index($0, support) { print $1; exit }' || true)"
if [[ -n "$existing" ]]; then
  echo "stopping previous dev instance $existing"
  kill "$existing"
  for _ in $(seq 1 50); do
    kill -0 "$existing" 2>/dev/null || break
    sleep 0.1
  done
fi
open -n -g "$app" --args --support-dir "$support"
pid="" 

for _ in $(seq 1 200); do
  if [[ -z "$pid" ]]; then
    pid="$(ps -axo pid=,command= | awk -v app="$app" -v support="$support" \
      'index($0, app "/Contents/MacOS/lumen") && index($0, support) { print $1; exit }' || true)"
  fi
  if [[ -n "$pid" && -S "$support/control.sock" ]] \
    && LUMEN_SUPPORT_DIR="$support" "$root/target/debug/lumen" settings state >/dev/null 2>&1; then
    echo "$pid" > "$pidfile"
    echo "pid $pid"
    echo "support $support"
    exit 0
  fi
  sleep 0.1
done

echo "dev app did not become ready" >&2
if [[ -n "$pid" ]]; then
  ps -p "$pid" -o pid=,command= >&2 || true
fi
exit 1
