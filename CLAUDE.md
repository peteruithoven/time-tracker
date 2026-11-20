# Time Tracker: notes for agents

A GNOME time tracker for one freelancer (Peter), in Rust with GTK 4,
libadwaita and Blueprint, built with Meson and shipped as a Flatpak on the
GNOME 51 runtime. Syncs time logs with Fibery and creates invoices through
"invoice scripts" (`src/invoicing/`). See README.md for features and layout.

## Commands

Everything builds inside the GNOME SDK (the host's rustc is too old):

| What | Command | Time |
| --- | --- | --- |
| Tests | `build-aux/cargo-sdk.sh test` | seconds |
| Lints | `build-aux/cargo-sdk.sh clippy --all-targets` (keep it warning-free) | seconds |
| Fast build into `_sdk/prefix` | `build-aux/sdk-build.sh` | ~2 s incremental |
| Try the UI headless | `build-aux/ui-test.sh start …` (see below) | ~6 s |
| Install the Devel Flatpak | `flatpak-builder --user --install --force-clean build-dir build-aux/nl.peteruithoven.TimeTracker.Devel.json` | 1–2 min |
| Install the regular Flatpak | `flatpak-builder --user --install --force-clean build-dir-release build-aux/nl.peteruithoven.TimeTracker.json` | 1–2 min |
| Store screenshots | `build-aux/screenshots.sh` (uses the installed Devel Flatpak) | ~30 s |
| Timing of the sync's store work | `build-aux/cargo-sdk.sh test --release -- --ignored --nocapture` | ~1.5 min |

Use the fast build and `ui-test.sh` while working. Install the Flatpaks only
when Peter should try something, and say which one (he uses the regular
version daily; the Devel one is for trying changes).

## Checking the UI

`build-aux/ui-test.sh start [--fibery] [--open PAGE] [--size WxH]` builds,
then runs the app on a virtual X display (`:99`) with demo data in
`~/.cache/time-tracker-ui-test/`, on its own session bus.

- `--open` goes straight to `review`, `clients`, `client:<name>`,
  `preferences` or `shortcuts`; prefer it over clicking.
- `--fibery` connects the demo client "Acme Studio" to
  `build-aux/fibery-mock.py`; its requests are logged in `fibery.log`.
  Make Fibery data go wrong with a scenario, e.g.
  `curl -d '{"scenario":"delete-logs","count":6}' 127.0.0.1:8099/mock`
  (list at the top of the mock; `reset` undoes them).
- `--large` adds five years of demo history and 2,000 mock logs, and logs
  main loop stalls over 50 ms (`TIME_TRACKER_DEBUG_STALLS`) plus sync and
  snapshot timings (`G_MESSAGES_DEBUG=time-tracker`) to `app.log`. The
  `_sdk` build is a debug build: roughly 4–10× slower than release.
- `ui-test.sh shot FILE.png` captures just the window; popovers are
  separate X windows, so for those use
  `DISPLAY=:99 import -window root -crop 420x520+0+0 FILE.png`. Keep screenshots few
  and small: they cost far more tokens than text. Check data in the
  database with `sqlite3 ~/.cache/time-tracker-ui-test/data/time-tracker/time-tracker.db`.
- Interact with `DISPLAY=:99 xdotool …`; there's no window manager, so
  `windowsize` doesn't work (use `--size`). Click inside the window before
  sending keyboard shortcuts.
- `ui-test.sh stop` stops only what `start` started.
- `TIME_TRACKER_DEBUG_AWAY=<minutes>` simulates an absence (idle alert).
- The test instance writes snapshots to `data/time-tracker/backups/` in its
  own folder (its GSettings are separate too), never to Peter's folder.

Don't use GTK's Broadway backend (its server crashes on popovers).

## Safety: Peter's own app and data

- Peter's real data is in `~/.var/app/nl.peteruithoven.TimeTracker{,.Devel}/`.
  Never open it with a test instance. Flatpak overrides `DISPLAY` and
  `XDG_DATA_HOME` passed with `--env`; that's why `ui-test.sh` sets them the
  way it does.
- Never stop processes by name (`pkill time-tracker`, killing every
  `time-tracker`): Peter's app is likely running. Stop only process IDs you
  started yourself.
- Don't copy over or migrate his data without asking him to quit the app.

## Privacy

The repository may become public. Never commit client or customer names,
Fibery workspace names, Moneybird IDs, rates or tokens; tests, demo data,
the mock and screenshots use made-up names (Acme Studio, Northwind, …).
Settings like rates and template invoices come from the app's settings,
API tokens from the keyring. Never type API keys or tokens yourself; Peter
enters them. Delete screenshots of his real screen after use.

## Platform notes (learned the hard way)

- **Keyring:** libsecret through the Secret portal; oo7 failed with the host's
  D-Bus proxy. Each Flatpak app ID has its own keyring, so tokens don't move
  between the Devel and regular versions (re-enter them on the client page).
- **Idle time:** Mutter's `org.gnome.Mutter.IdleMonitor` works on elementary
  OS's Gala, but its watches never fire there: the app polls `GetIdletime`.
- **Dock badge:** Unity LauncherEntry `Update` signal, arguments `(sa{sv})`
  (not a nested variant); elementary's dock shows a count of 0 as "!".
  No extra Flatpak permission needed.
- **Fibery:** commands API with parameters (`"$name"`), 3 requests per second.
  The real workspace may be read with the Fibery tools for checking query
  syntax; don't write to it unless Peter asks, and clean up test entities.
- **Folders chosen through the portal** appear as
  `/run/user/1000/doc/<id>/<name>` in the sandbox, and stay accessible after
  restarts. Peter's xdg-desktop-portal (1.18, Documents portal version 4)
  can't tell the real location (no `GetHostPaths`, no host-path xattr), so
  Preferences shows only the folder's name there.
- **gitlab.gnome.org** refuses automated page fetches; `git clone --depth 1`
  works for reading GNOME source.

## Code conventions

- GNOME HIG and libadwaita widgets; title case for titles, buttons and row
  titles. All UI strings through gettext; new files with strings go in
  `po/POTFILES.in`.
- Static UI in Blueprint (`*.blp`, listed in `src/meson.build` and
  `src/resources.gresource.xml`); dynamic lists built in code.
- After changing stored data, call `TimeTrackerApplication::data_changed()`
  (deferred and coalesced). Pages that rebuild rows use a content key
  (`Section` in `clients_dialog.rs`), so focus isn't lost and GTK doesn't
  complain about widgets removed mid-event.
- Closures on widgets hold state strongly and widgets weakly (no cycles, and
  nothing dropped too early).
- Keep logic testable apart from GTK (see `idle::Detector`, the Fibery query
  builders, `launcher::badge`).

## Git

Work happens on `main`. Commit only when Peter asks, one commit per logical
change, with a message explaining what and why, ending with the
`Co-Authored-By` line. Nothing is pushed unless he asks.

## Design

Designs are in Figma, file `spKnxEOAjPejf8U58cLV7K`: page "v2 · GNOME"
(screens A–N, light), "v2 · Dark mode", and "Icon" (app, symbolic and Devel
icons with export notes). Check a screen there before building it; for
standard preference-style pages a quick sketch or none is enough. Avoid
reading a whole page's metadata at once (it's huge); get a frame by ID.
