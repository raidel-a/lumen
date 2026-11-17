#!/usr/bin/env bash
# End-to-end Sparkle self-update test, entirely local and entirely against the project's own dev
# instance ("Lumen Dev", bundle id pro.lumen.Lumen.dev, a dedicated support dir). It never touches
# the installed /Applications/Lumen.app or any app this script did not start.
#
# It builds version N and N+1 of the dev bundle, publishes N+1 through a locally served, EdDSA
# signed appcast, launches N pointed at that feed, and watches N update itself to N+1 and relaunch.
# The update enclosure is the ditto zip, signed by the private key in the login keychain and
# verified by Sparkle against the SUPublicEDKey embedded in the bundle.
#
# Everything created here lives under .build/update-test and is cleaned up on exit; the only
# long-lived process started is a localhost file server, stopped by its exact recorded PID.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
work="$root/.build/update-test"
serve="$work/serve"
oldver="${LUMEN_TEST_OLD_VERSION:-0.0.1}"
newver="${LUMEN_TEST_NEW_VERSION:-0.0.2}"
port="${LUMEN_TEST_PORT:-8765}"
support="$HOME/Library/Application Support/Lumen Update Test"
devapp="$root/dist/Lumen Dev.app"
plist="$devapp/Contents/Info.plist"
bid="pro.lumen.Lumen.dev"

server_pid=""
cleanup() {
  LUMEN_DEV_SUPPORT="$support" "$root/scripts/dev/stop.sh" >/dev/null 2>&1 || true
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
  fi
  # Drop the test-only Sparkle defaults (automatic silent updates) so a later manual run of the
  # dev app behaves normally. The first-launch flag is left in place.
  defaults delete "$bid" SUAutomaticallyUpdate >/dev/null 2>&1 || true
  defaults delete "$bid" SUEnableAutomaticChecks >/dev/null 2>&1 || true
  defaults delete "$bid" SUScheduledCheckInterval >/dev/null 2>&1 || true
  defaults delete "$bid" SUFeedURL >/dev/null 2>&1 || true
  defaults delete "$bid" SULastCheckTime >/dev/null 2>&1 || true
  rm -rf "$support"
}
trap cleanup EXIT

short_version() { /usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$1/Contents/Info.plist"; }
signing_id() { codesign -dvvv "$1" 2>&1 | sed -n 's/^Authority=//p;s/^TeamIdentifier=//p;s/^Identifier=//p'; }

echo "== building N+1 ($newver) dev bundle =="
LUMEN_DEV_VERSION="$newver" LUMEN_DEV_BUILD="$newver" "$root/scripts/dev/bundle.sh" >/dev/null
rm -rf "$serve"
mkdir -p "$serve"
# Package N+1 as the update enclosure (ditto zip, keepParent so the archive holds Lumen Dev.app).
ditto -c -k --sequesterRsrc --keepParent "$devapp" "$serve/Lumen-Dev-$newver.zip"

echo "== generating locally served, signed appcast advertising $newver =="
"$root/vendor/Sparkle/bin/generate_appcast" \
  --download-url-prefix "http://localhost:$port/" \
  "$serve" >/dev/null
test -f "$serve/appcast.xml"

echo "== serving $serve on http://localhost:$port =="
( cd "$serve" && exec python3 -m http.server "$port" --bind 127.0.0.1 ) >/dev/null 2>&1 &
server_pid=$!
for _ in $(seq 1 50); do
  curl -fsS "http://localhost:$port/appcast.xml" >/dev/null 2>&1 && break
  sleep 0.1
done
curl -fsS "http://localhost:$port/appcast.xml" >/dev/null

echo "== building N ($oldver) dev bundle pointed at the local feed =="
LUMEN_DEV_SUPPORT="$support" "$root/scripts/dev/stop.sh" >/dev/null 2>&1 || true
# run.sh rebuilds the dev bundle via dev/bundle.sh, so pass the N version and the local feed
# through its environment. Build once here too, to set the Sparkle defaults against the right id.
LUMEN_FEED_URL="http://localhost:$port/appcast.xml" \
  LUMEN_DEV_VERSION="$oldver" LUMEN_DEV_BUILD="$oldver" "$root/scripts/dev/bundle.sh" >/dev/null

before_id="$(signing_id "$devapp")"
echo "   installed version before: $(short_version "$devapp")"

# Sparkle preferences for an unattended test: skip the first-run permission prompt, check
# automatically, and install silently so no click-through is required.
defaults write "$bid" SUHasLaunchedBefore -bool true
defaults write "$bid" SUEnableAutomaticChecks -bool true
defaults write "$bid" SUAutomaticallyUpdate -bool true
defaults write "$bid" SUScheduledCheckInterval -int 1
defaults write "$bid" SUFeedURL "http://localhost:$port/appcast.xml"
# Forget any prior "last check" so a check runs promptly.
defaults delete "$bid" SULastCheckTime >/dev/null 2>&1 || true

echo "== launching N and waiting for it to self-update to $newver =="
rm -rf "$support"
# Pass the N version + local feed through run.sh so its internal rebuild stays at N, not the
# default dev version. Without this, run.sh would rebuild Lumen Dev.app at its 0.0.0 default.
LUMEN_DEV_SUPPORT="$support" \
  LUMEN_FEED_URL="http://localhost:$port/appcast.xml" \
  LUMEN_DEV_VERSION="$oldver" LUMEN_DEV_BUILD="$oldver" \
  "$root/scripts/dev/run.sh" >/dev/null

# Wait for Sparkle to check, download, verify the EdDSA signature, and STAGE the update. Sparkle's
# Autoupdate helper is the installer that swaps the bundle; it runs once a verified update is ready
# and then waits for the host app to terminate. Require it to be seen on two consecutive checks so
# a momentary launch does not race the quit below.
echo "   waiting for Sparkle to download, verify, and stage the update"
staged=""
seen=0
updater_glob="$devapp/Contents/Frameworks/Sparkle.framework/Versions/"
for i in $(seq 1 120); do
  if ps -axo command= | grep -F "$updater_glob" | grep -qE "Autoupdate pro\.lumen\.Lumen\.dev|Updater.app/Contents/MacOS/Updater"; then
    seen=$((seen + 1))
    if [[ "$seen" -ge 2 ]]; then
      staged="yes"
      echo "   update staged after ${i}s (verified; installer waiting for the app to quit)"
      # Let the installer finish writing the staged copy before we quit the host.
      sleep 2
      break
    fi
  else
    seen=0
  fi
  sleep 1
done

# With SUAutomaticallyUpdate, Sparkle's staged installer swaps the bundle and relaunches once the
# host app terminates. Quit through the control socket (the project's clean-quit path).
if [[ -n "$staged" ]]; then
  echo "   quitting the host app so the staged installer can finish"
  LUMEN_SUPPORT_DIR="$support" "$root/target/debug/lumen" quit >/dev/null 2>&1 || true
fi

updated=""
for i in $(seq 1 90); do
  v="$(short_version "$devapp" 2>/dev/null || echo "?")"
  if [[ "$v" == "$newver" ]]; then
    updated="yes"
    echo "   installed bundle is now $newver"
    break
  fi
  sleep 1
done

after_id="$(signing_id "$devapp")"

echo
echo "== result =="
if [[ -n "$updated" ]]; then
  echo "PASS: dev app updated $oldver -> $newver via the signed local appcast"
  echo "      (Sparkle checked, downloaded, verified the EdDSA signature against the embedded"
  echo "       SUPublicEDKey, installed, and relaunched; a bad signature would have been rejected)"
else
  echo "FAIL: dev app did not reach $newver within the timeout (still $(short_version "$devapp"))" >&2
  [[ -z "$staged" ]] && echo "      the update never staged; check the feed, signature, or version compare" >&2
fi

echo
echo "code-signing identity before:"
echo "$before_id" | sed 's/^/   /'
echo "code-signing identity after:"
echo "$after_id" | sed 's/^/   /'
if [[ "$before_id" == "$after_id" ]]; then
  echo "PASS: signing identity unchanged across the update"
else
  echo "FAIL: signing identity changed across the update" >&2
fi

[[ -n "$updated" && "$before_id" == "$after_id" ]]
