#!/usr/bin/env bash
# Version helpers shared by the bundle scripts.
#
# Releases always pass LUMEN_VERSION from the tag. Local builds use the last release tag plus a
# -dev.N suffix when HEAD is ahead of it, so Settings > About cannot claim an old release version
# while the appcast advertises a newer one.

# Prints the version to embed in a local build: the last tag, or <last>-dev.<commits ahead>.
lumen_local_version() {
  local root="$1" tag base ahead
  tag="$(git -C "$root" describe --tags --abbrev=0 2>/dev/null)" || {
    echo "0.0.0"
    return
  }
  base="${tag#v}"
  ahead="$(git -C "$root" rev-list --count "${tag}..HEAD" 2>/dev/null || echo 0)"
  if [[ "$ahead" == "0" ]]; then
    echo "$base"
  else
    echo "${base}-dev.${ahead}"
  fi
}
