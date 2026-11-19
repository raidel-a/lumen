#!/usr/bin/env bash
# Capture the Hoy dev VM's desktop to a PNG without opening Screen Sharing, so it never disrupts
# the host desktop. It opens Hoy's VNC bridge with --no-open (a 127.0.0.1-only socket, no viewer
# window), attaches a headless VNC client (vncdotool) to wake the framebuffer and grab one frame,
# then closes the bridge.
#
#   scripts/dev/vm-shot.sh [--vm NAME] [--out PATH]
#
# Requires: hoy, and vncdo in ~/.lumen-vnc (a dedicated venv: python3 -m venv ~/.lumen-vnc &&
# ~/.lumen-vnc/bin/pip install vncdotool). The VM must have been created with --vnc/--window.
set -euo pipefail

vm="lumen-dev"
out="/tmp/vm-shot.png"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --vm)  vm="$2";  shift 2 ;;
    --out) out="$2"; shift 2 ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done

vncdo="$HOME/.lumen-vnc/bin/vncdo"
if [[ ! -x "$vncdo" ]]; then
  echo "vm-shot: vncdo not found at $vncdo" >&2
  echo "         set it up with: python3 -m venv ~/.lumen-vnc && ~/.lumen-vnc/bin/pip install vncdotool" >&2
  exit 1
fi

tmp="$(mktemp -d)"
json="$tmp/bridge.json"
trap 'kill "$bridge_pid" 2>/dev/null || true; rm -rf "$tmp"' EXIT

# Start the bridge headless: no Screen Sharing window opens. Print the password so we can connect.
hoy vnc "$vm" --no-open --show-password --json > "$json" 2>&1 &
bridge_pid=$!

# Wait for the "ready" line with the port and password.
port=""; password=""
for _ in $(seq 1 60); do
  if [[ -s "$json" ]]; then
    port="$(sed -n 's/.*"port":\([0-9]*\).*/\1/p' "$json" | head -1)"
    password="$(sed -n 's/.*"password":"\([^"]*\)".*/\1/p' "$json" | head -1)"
  fi
  [[ -n "$port" ]] && break
  sleep 0.5
done
if [[ -z "$port" ]]; then
  echo "vm-shot: VNC bridge did not become ready" >&2
  cat "$json" >&2 || true
  exit 1
fi

# Attach and capture one frame. Connecting wakes the VM framebuffer; a short pause lets it paint.
server="127.0.0.1::$port"
if [[ -n "$password" ]]; then
  "$vncdo" -s "$server" -p "$password" pause 1 capture "$out" >/dev/null 2>&1
else
  "$vncdo" -s "$server" pause 1 capture "$out" >/dev/null 2>&1
fi

if [[ -s "$out" ]]; then
  echo "$out"
else
  echo "vm-shot: capture produced no file" >&2
  exit 1
fi
