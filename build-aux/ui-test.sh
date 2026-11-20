#!/bin/sh
# Headless UI testing of the development build, for people and agents:
# builds with build-aux/sdk-build.sh, then runs the app on a virtual X
# display (Xvfb) with made-up demo data in a throwaway folder. Your own data,
# keyring and running apps are never touched: the app gets its own session
# bus, and `stop` only stops what `start` started.
#
#   build-aux/ui-test.sh start [--fibery] [--open PAGE] [--size WxH]
#       --fibery     connect the demo client to build-aux/fibery-mock.py
#       --open PAGE  open a page: review, clients, client:<name>,
#                    preferences or shortcuts
#       --size WxH   window size, e.g. 960x640 (default 360x560)
#   build-aux/ui-test.sh shot FILE.png    screenshot of the app's window
#   build-aux/ui-test.sh stop             stop everything, remove the data
#
# Interact with xdotool on the same display, e.g.
#   DISPLAY=:99 xdotool mousemove 180 120 click 1
#   DISPLAY=:99 xdotool key ctrl+comma
# The app log is in ~/.cache/time-tracker-ui-test/app.log, the Fibery mock's
# requests in fibery.log, the database in data/time-tracker/.
#
# Needs Xvfb, xdotool and ImageMagick's import on the host.
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
state="$HOME/.cache/time-tracker-ui-test"
display=:99
prefix="$repo/_sdk/prefix"

# All descendants of a process, deepest first.
descendants() {
  for child in $(ps -o pid= --ppid "$1" 2>/dev/null); do
    descendants "$child"
    echo "$child"
  done
}

stop_started() {
  pidfile="$state/$1.pid"
  [ -f "$pidfile" ] || return 0
  pid=$(cat "$pidfile")
  # shellcheck disable=SC2046
  kill $(descendants "$pid") "$pid" 2>/dev/null || true
  rm -f "$pidfile"
}

case "${1:-}" in
start)
  shift
  fibery=""
  open=""
  size=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --fibery) fibery=1 ;;
      --open) open="$2"; shift ;;
      --size) size="$2"; shift ;;
      *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
    shift
  done
  "$0" stop
  "$repo/build-aux/sdk-build.sh"
  mkdir -p "$state/config/glib-2.0/settings"
  # The window size comes from GSettings, kept in a keyfile here.
  if [ -n "$size" ]; then
    printf '[nl/peteruithoven/TimeTracker]\nwindow-width=%s\nwindow-height=%s\n' \
      "${size%x*}" "${size#*x}" >"$state/config/glib-2.0/settings/keyfile"
  fi

  Xvfb "$display" -screen 0 1280x900x24 >"$state/xvfb.log" 2>&1 &
  echo $! >"$state/xvfb.pid"

  demo_fibery=""
  if [ -n "$fibery" ]; then
    python3 "$repo/build-aux/fibery-mock.py" 8099 >"$state/fibery.log" 2>&1 &
    echo $! >"$state/fibery.pid"
    demo_fibery="http://127.0.0.1:8099"
  fi
  sleep 1

  # Flatpak takes the display from this environment, and sets the data
  # folders itself: those are set inside the sandbox.
  DISPLAY="$display" WAYLAND_DISPLAY="" flatpak run \
    --filesystem=home --share=network --socket=x11 --nosocket=wayland \
    --env=GDK_BACKEND=x11 --env=GSK_RENDERER=cairo --env=GTK_IM_MODULE=simple \
    --env=GSETTINGS_BACKEND=keyfile \
    --env=TIME_TRACKER_DEMO=1 --env=TIME_TRACKER_DEMO_FIBERY="$demo_fibery" \
    --env=TIME_TRACKER_OPEN="$open" \
    --command=sh org.gnome.Sdk//51 -c "
      export XDG_DATA_HOME='$state/data' XDG_CONFIG_HOME='$state/config'
      export GSETTINGS_SCHEMA_DIR='$prefix/share/glib-2.0/schemas'
      export XDG_DATA_DIRS='$prefix/share':\"\$XDG_DATA_DIRS\"
      exec dbus-run-session -- '$prefix/bin/time-tracker'" \
    >"$state/app.log" 2>&1 &
  echo $! >"$state/app.pid"

  for _ in $(seq 40); do
    if DISPLAY="$display" xdotool search --name "Time Tracker" >/dev/null 2>&1; then
      sleep 1
      echo "Time Tracker runs on $display (data in $state)"
      exit 0
    fi
    sleep 0.5
  done
  echo "The app didn't open a window; see $state/app.log" >&2
  exit 1
  ;;
shot)
  window=$(DISPLAY="$display" xdotool search --onlyvisible --name "Time Tracker" | head -n 1)
  DISPLAY="$display" import -window "${window:-root}" "${2:?Usage: ui-test.sh shot FILE.png}"
  ;;
stop)
  for name in app fibery xvfb; do
    stop_started "$name"
  done
  rm -rf "$state"
  ;;
*)
  sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
  ;;
esac
