#!/usr/bin/env bash
# Inspect and drive the running dev instance through agent-desktop (the accessibility-tree CLI).
# It is a richer alternative to ax.swift: JSON snapshots with a ref per element, for both the
# menu-bar extra and the settings window.
#
# agent-desktop is pinned in this repo's mise.toml, so run it through mise.
#
# Usage:
#   scripts/dev/axcli.sh menu            snapshot the menu-bar extra (interactive elements)
#   scripts/dev/axcli.sh window          snapshot the settings window (interactive elements)
#   scripts/dev/axcli.sh raw -- <args>   run agent-desktop with your own args, app preset
#
# It only ever targets the dev app ("Lumen Dev"), never a production Lumen the user is running.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
pidfile="$root/dist/lumen-dev.pid"
app="Lumen Dev"

ad() { mise exec --cd "$root" -- agent-desktop "$@"; }

if ! mise exec --cd "$root" -- agent-desktop version >/dev/null 2>&1; then
  echo "axcli: agent-desktop is not available; run 'mise install' in $root" >&2
  exit 1
fi

if [[ ! -f "$pidfile" ]] || ! kill -0 "$(cat "$pidfile")" 2>/dev/null; then
  echo "axcli: no dev instance running; start one with scripts/dev/run.sh" >&2
  exit 1
fi

cmd="${1:-menu}"
case "$cmd" in
  menu)
    ad snapshot --app "$app" --surface menu-bar-extras -i
    ;;
  window)
    ad snapshot --app "$app" --surface window -i
    ;;
  raw)
    shift
    [[ "${1:-}" == "--" ]] && shift
    ad --app "$app" "$@"
    ;;
  *)
    echo "axcli: unknown command '$cmd' (expected menu, window, or raw)" >&2
    exit 1
    ;;
esac
