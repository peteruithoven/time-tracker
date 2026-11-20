#!/bin/sh
# Fast, incremental development build inside the GNOME SDK: Meson installs
# the app into _sdk/prefix in the repository (a second or two after the first
# build), instead of a full Flatpak build. Run it with build-aux/ui-test.sh,
# or install the real Devel Flatpak when you want to use it (see README).
#
#   build-aux/sdk-build.sh
#
# Needs org.gnome.Sdk//51 and org.freedesktop.Sdk.Extension.rust-stable.
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
setup=""
if [ ! -f "$repo/_sdk/build.ninja" ]; then
  setup="meson setup _sdk -Dprofile=development --prefix='$repo/_sdk/prefix' >/dev/null &&"
fi
exec flatpak run \
  --filesystem="$repo" \
  --share=network \
  --env=PATH=/usr/bin:/usr/lib/sdk/rust-stable/bin \
  --cwd="$repo" \
  --command=sh \
  org.gnome.Sdk//51 -c "$setup meson install -C _sdk --quiet"
