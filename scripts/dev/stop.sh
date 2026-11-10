#!/usr/bin/env bash
# Stops the dev instance. It asks the app to quit through its control socket first, so virtual
# displays and agent browsers are released; only then falls back to the exact recorded PID.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
support="${LUMEN_DEV_SUPPORT:-$HOME/Library/Application Support/Lumen Dev}"
pidfile="$root/dist/lumen-dev.pid"

if [[ ! -f "$pidfile" ]]; then
  echo "no dev pid file" >&2
  exit 0
fi

pid="$(cat "$pidfile")"
if ! kill -0 "$pid" 2>/dev/null; then
  echo "dev instance not running"
  rm -f "$pidfile"
  exit 0
fi

command="$(ps -p "$pid" -o command= 2>/dev/null || true)"
if [[ "$command" != *"Lumen Dev.app/Contents/MacOS/lumen"* ]]; then
  echo "pid $pid is not the dev app; refusing to stop it" >&2
  echo "$command" >&2
  exit 1
fi

# A clean quit releases CGVirtualDisplay objects. SIGTERM skips destructors and can leave ghost
# displays registered in the Window Server until logout.
if [[ -x "$root/target/debug/lumen" ]]; then
  LUMEN_SUPPORT_DIR="$support" "$root/target/debug/lumen" quit >/dev/null 2>&1 || true
  for _ in $(seq 1 50); do
    if ! kill -0 "$pid" 2>/dev/null; then
      rm -f "$pidfile"
      echo "quit $pid"
      exit 0
    fi
    sleep 0.1
  done
fi

kill "$pid"
for _ in $(seq 1 50); do
  if ! kill -0 "$pid" 2>/dev/null; then
    rm -f "$pidfile"
    echo "stopped $pid"
    exit 0
  fi
  sleep 0.1
done

echo "dev app $pid did not exit" >&2
exit 1
