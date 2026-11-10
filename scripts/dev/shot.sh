#!/usr/bin/env bash
# Opens a settings page through the control socket and captures that window by ID.
#
#   scripts/dev/shot.sh --page virtual --screen agent --out /tmp/settings.png
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
support="${LUMEN_DEV_SUPPORT:-$HOME/Library/Application Support/Lumen Dev}"
page=""
screen=""
out="$root/dist/settings.png"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --page) page="$2"; shift 2 ;;
    --screen) screen="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done

args=()
[[ -n "$page" ]] && args+=(--page "$page")
[[ -n "$screen" ]] && args+=(--screen "$screen")
LUMEN_SUPPORT_DIR="$support" "$root/target/debug/lumen" settings "${args[@]}" >/dev/null
sleep 0.4

pid="$(cat "$root/dist/lumen-dev.pid" 2>/dev/null || true)"
if [[ -z "$pid" ]]; then
  echo "dev app is not running" >&2
  exit 1
fi
id="$(swift "$root/scripts/dev/window.swift" "$pid" "Lumen Settings")"
if [[ -z "$id" ]]; then
  echo "settings window not found" >&2
  exit 1
fi
mkdir -p "$(dirname "$out")"
screencapture -x -o -l "$id" "$out"
echo "$out"
