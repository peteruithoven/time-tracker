#!/bin/sh
# Renders the screenshots in data/screenshots/ from demo data, headless.
# Uses the installed development Flatpak, so build and install it first
# (see README). Each shot runs on a throwaway database.
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
out="$repo/data/screenshots"
mkdir -p "$out" "$HOME/.cache"
tmp=$(mktemp -d "$HOME/.cache/time-tracker-screenshots.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

# shot NAME VIEW WIDTHxHEIGHT COLOR-SCHEME
shot() {
  rm -rf "$tmp/data" "$tmp/config"
  flatpak run \
    --filesystem="$tmp" --filesystem="$out" \
    --env=GTK_IM_MODULE=simple \
    --command=sh nl.peteruithoven.TimeTracker.Devel -c "
      gtk4-broadwayd :42 >/dev/null 2>&1 &
      sleep 1
      export XDG_DATA_HOME='$tmp/data' XDG_CONFIG_HOME='$tmp/config'
      export GDK_BACKEND=broadway BROADWAY_DISPLAY=:42 ADW_DEBUG_COLOR_SCHEME=$4
      export TIME_TRACKER_DEMO=1 TIME_TRACKER_SCREENSHOT='$out/$1.png'
      export TIME_TRACKER_SCREENSHOT_VIEW=$2 TIME_TRACKER_SCREENSHOT_SIZE=$3
      dbus-run-session -- time-tracker
      kill %1"
}

shot track track 400x640 prefer-light
shot review review 960x640 prefer-light
shot track-dark track 400x640 prefer-dark
