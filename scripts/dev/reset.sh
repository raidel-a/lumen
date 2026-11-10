#!/usr/bin/env bash
# Deletes the dev support directory. The installed app's data is never touched.
set -euo pipefail

support="${LUMEN_DEV_SUPPORT:-$HOME/Library/Application Support/Lumen Dev}"
if [[ "${1:-}" != "--yes" ]]; then
  read -r -p "Delete $support? [y/N] " answer
  [[ "$answer" == "y" || "$answer" == "Y" ]] || exit 0
fi
rm -rf "$support"
echo "deleted $support"
