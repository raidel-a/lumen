#!/usr/bin/env bash
# Stops only the dev instance started by run.sh, by exact PID and after checking its command.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
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
