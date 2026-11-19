#!/bin/sh
# Runs cargo inside the GNOME SDK, so `cargo check`, `cargo test` and
# `cargo clippy` use the same GTK/libadwaita versions as the Flatpak build.
#
#   build-aux/cargo-sdk.sh test
#   build-aux/cargo-sdk.sh clippy
#
# Needs org.gnome.Sdk//51 and org.freedesktop.Sdk.Extension.rust-stable.
exec flatpak run \
  --filesystem=home \
  --share=network \
  --env=PATH=/usr/bin:/usr/lib/sdk/rust-stable/bin \
  --env=CARGO_HOME="$HOME/.cache/time-tracker-cargo" \
  --cwd="$(cd "$(dirname "$0")/.." && pwd)" \
  --command=cargo \
  org.gnome.Sdk//51 "$@"
