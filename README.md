# Time Tracker

A compact GNOME time tracker for freelancers. Track time on your own clients and projects, or on tasks from [Fibery](https://fibery.io), review it per client and project for invoicing, and sync time logs back to Fibery.

Built with Rust, GTK 4 and libadwaita, packaged as a Flatpak.

**Status:** early development. Tracking time on local clients, projects and tasks works; the Review view and Fibery sync are next.

## Building

The app targets the GNOME 51 runtime (libadwaita 1.8 or newer), so build and run it as a Flatpak rather than against your distribution's libraries.

Install the runtime, SDK and Rust extension once:

```bash
flatpak install --user flathub org.gnome.Platform//51 org.gnome.Sdk//51 org.freedesktop.Sdk.Extension.rust-stable//26.08
```

Build and install the development version (app ID `nl.peteruithoven.TimeTracker.Devel`, striped headerbar). Run this from the repository root:

```bash
flatpak-builder --user --install --force-clean build-dir build-aux/nl.peteruithoven.TimeTracker.Devel.json
```

Run it:

```bash
flatpak run nl.peteruithoven.TimeTracker.Devel
```

For the regular version, use `build-aux/nl.peteruithoven.TimeTracker.json` and run `nl.peteruithoven.TimeTracker`. Both can be installed side by side.

The build fetches crates from crates.io, so it needs network access.

### Faster checks while developing

A full Flatpak build recompiles everything. For quick feedback, run cargo inside the GNOME SDK instead (same library versions, incremental):

```bash
build-aux/cargo-sdk.sh test
```

```bash
build-aux/cargo-sdk.sh clippy
```

The development build keeps its data in `~/.var/app/nl.peteruithoven.TimeTracker.Devel/data/time-tracker/`.

## Project layout

| Path | What |
| --- | --- |
| `src/` | Rust sources and Blueprint UI files (`*.blp`) |
| `data/` | Desktop file, AppStream metainfo, GSettings schema, icons |
| `po/` | Translations (gettext) |
| `build-aux/` | Flatpak manifests |

## License

MIT, see [LICENSE](LICENSE).
