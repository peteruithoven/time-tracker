# Time Tracker

A compact GNOME time tracker for freelancers. Track time on your own clients and projects, or on tasks from [Fibery](https://fibery.io), review it per client and project for invoicing, and sync time logs back to Fibery.

Built with Rust, GTK 4 and libadwaita, packaged as a Flatpak.

![Tracking time](data/screenshots/track.png)
![Reviewing a month](data/screenshots/review.png)

## Features

- **Timer:** one big start/stop button. Switching tasks stops the running entry and starts a new one. A running timer is saved immediately, so it survives crashes and restarts.
- **Dock badge:** while the timer runs, the app's dock icon shows the elapsed minutes (elementary OS's dock shows "!" during the first minute). It uses the Unity LauncherEntry API, which docks such as elementary OS's support.
- **Away time:** when you come back after 10 minutes without keyboard or mouse input (or a suspend) while the timer runs, the app asks whether to keep that time, discard it, assign it to another task or stop the timer when you left. The threshold is in Preferences. This uses GNOME's idle monitor (`org.gnome.Mutter.IdleMonitor`), which elementary OS's Gala offers too.
- **Task picker:** fuzzy search over all tasks, recent ones first, filter per client, and create tasks (and projects) on the spot.
- **Review:** a month per client › project › task with a column per week. Click any number to see, edit or add the entries behind it.
- **Clients & Projects:** hourly rates (per client, optionally per project), billable or not, archiving, marking tasks done.
- **Export:** a month's entries as CSV (Ctrl+E).
- **Fibery sync**, **backups** and **invoice scripts**, see below.
- Light and dark style, adaptive down to a 360 px wide window.

## Fibery

A client can be connected to a Fibery workspace: Clients & Projects → the client → Connect Fibery…. You need your workspace address and an API key, which you create in Fibery under Settings → API Keys.

The app finds the task, project and time log databases and their fields by itself and shows what it found before you connect. The time log database needs at least a link to the task, a date and a number for the hours; optional fields are the user, a note, start and end times, and a "Timing" checkbox on the task. To change what it found, open **Time Logs** on the client page. **Show Tasks** sets which tasks the task picker offers: yours or everyone's, which states, and how long finished tasks stay.

**What syncs**

| From Fibery to the app | From the app to Fibery |
| --- | --- |
| All projects (those in hidden states, by default the finished ones, are archived) | Tasks you create under the connected client, assigned to you |
| The tasks for the picker (by default: assigned to you, not finished) | New, edited and deleted time entries |
| Your own time logs: a year on the first sync, the last two months after that | With **Mirror Running Timer** on: the time log is created when the timer starts and finished when it stops, ticking the task's "Timing" checkbox meanwhile, like Fibery's own timer buttons |
| Time logs deleted in Fibery are removed | |

Hours are sent with two decimals, like Fibery's own timer. Totals in the app use the same rounding, so they match Fibery.

**When:** a full sync at startup, every 5 minutes, when the network comes back and with Sync Now (header bar). Local changes are pushed a few seconds after you make them. Without a connection everything keeps working and syncs later. The sync button in the header bar shows the state, with a dot while entries wait to be sent; its popover lists them, and they're marked in Today and Review too.

**Conflicts:** Fibery wins. If a time log changed in Fibery while a local edit of it was still waiting to be pushed, the Fibery version replaces the local one. Local edits that were pushed are never overwritten by older data.

**When syncing pauses:** before applying what it pulled, the app works out what that would change here. It pauses syncing (and asks) when a pull would:

- remove or change more than 5 entries (a different duration, date, task or project; a new note or a renamed task doesn't count),
- remove every synced entry of the last two months (even just one),
- lower the tracked time by more than 4 hours (removed entries plus shortened ones), or
- add more than 50 entries.

The very first sync of a client never pauses, since it brings in a year of history.

While paused, nothing changes here and nothing is pushed: your own changes wait. A banner and the header bar icon say so, and **Review** lists what would be removed, changed and added. **Keep Paused** leaves everything as it is; **Apply Changes** saves a snapshot first and then applies the changes. The app keeps checking every 5 minutes: once Fibery returns data that's within these limits again (say, after you fixed the API key's access or the Time Logs settings), syncing resumes by itself. Time logs that come back without a task or date are left alone rather than removed, and hours that read as empty keep the duration the app knows.

## Backups

The app saves a snapshot of its database every day it runs, and right before a sync removes entries (or when you change a client's Time Logs settings). A snapshot is a complete copy of the SQLite database without API keys, as one file you can open with any SQLite tool.

**Which ones are kept:** the newest snapshot of each of the last 7 days that have one, of each of the last 4 weeks (Monday to Sunday) and of each of the last 12 months. Snapshots taken before a sync are kept for 30 days, at most the 20 newest.

**Where:** by default in `backups/` next to the database (`~/.var/app/nl.peteruithoven.TimeTracker/data/time-tracker/backups/`), which `flatpak uninstall --delete-data` removes too. In Preferences → Backups you can choose another folder, for example one that Nextcloud syncs. If that folder isn't there (an unmounted drive), snapshots go to the default folder until it's back.

Files are named `time-tracker-<computer>-<date>-<time>.db` (`time-tracker-devel-…` for the development build, `…-before-sync.db` for the extra ones), so several computers can share one folder: each only ever removes its own snapshots, and nothing else in the folder. Preferences warns when the chosen folder has other files and offers to use a subfolder instead. Snapshots are written as `….db.part` and renamed when complete, so sync clients never upload half a file.

To restore one, quit the app and copy the snapshot over `time-tracker.db` (and remove `time-tracker.db-wal` and `time-tracker.db-shm` next to it). Synced entries that no longer exist in Fibery are removed again on the next sync, after a pause to review; restoring those into Fibery isn't built in yet.

**The API key** is stored in your system keyring (through the Secret portal). If no keyring is available, it's kept in the app's own database instead and every sync tries to move it to the keyring; the client page says where it's stored. Disconnecting removes the key and keeps the synced data in the app; nothing is removed from Fibery.

## Invoice scripts

An invoice script creates a month's invoice for a client in an invoicing service. Scripts are small Rust modules in [`src/invoicing/`](src/invoicing/), enabled per client under Clients & Projects → the client → Invoicing. In Review, filter on that client and use **Create Invoice**: it shows the lines first and then creates the invoice.

Scripts keep account details out of the code. Rates come from the client and project settings, other details (such as a template invoice) from the script's settings, and API tokens from the keyring.

The included **Moneybird invoice** creates a draft sales invoice in [Moneybird](https://www.moneybird.com) with a line per project, billed in days (8 hours, at the hourly rate × 8). It copies the contact, contact person, VAT and layout from an earlier invoice whose address you paste in the settings, and opens the draft in your browser for final tweaks.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| Ctrl+T | Start or stop the timer |
| Ctrl+K | Choose a task |
| Ctrl+N | Add time |
| Ctrl+1 / Ctrl+2 | Track / Review |
| Ctrl+Page Up / Ctrl+Page Down | Previous / next month |
| Ctrl+E | Export the month as CSV |
| Ctrl+, | Clients & Projects |
| Ctrl+? | All keyboard shortcuts |
| Ctrl+W / Ctrl+Q | Close / quit |

## Building

The app targets the GNOME 51 runtime (libadwaita 1.8 or newer), so build and run it as a Flatpak rather than against your distribution's libraries.

Install the runtime, SDK and Rust extension once:

```bash
flatpak install --user flathub org.gnome.Platform//51 org.gnome.Sdk//51 org.freedesktop.Sdk.Extension.rust-stable//26.08
```

Build and install the development version (app ID `nl.peteruithoven.TimeTracker.Devel`, with a striped header bar and GNOME's striped nightly icon). Run this from the repository root:

```bash
flatpak-builder --user --install --force-clean build-dir build-aux/nl.peteruithoven.TimeTracker.Devel.json
```

Run it:

```bash
flatpak run nl.peteruithoven.TimeTracker.Devel
```

For the regular version, use `build-aux/nl.peteruithoven.TimeTracker.json` with its own build folder and run `nl.peteruithoven.TimeTracker`:

```bash
flatpak-builder --user --install --force-clean build-dir-release build-aux/nl.peteruithoven.TimeTracker.json
```

Both can be installed side by side, each with its own data. To move your data from one to the other, quit both and copy `data/time-tracker/` and `config/glib-2.0/settings/keyfile` between their folders in `~/.var/app/`. API keys stay behind, as each app has its own keyring: enter them again under New API Key (Fibery) and API Token (invoice scripts) on the client page.

The build fetches crates from crates.io, so it needs network access.

### Faster checks while developing

A full Flatpak build recompiles everything. For quick feedback, run cargo inside the GNOME SDK instead (same library versions, incremental):

```bash
build-aux/cargo-sdk.sh test
```

```bash
build-aux/cargo-sdk.sh clippy
```

To try UI changes without a Flatpak build, build into `_sdk/` in the repository (about two seconds after the first build) and run the app headless with made-up demo data, optionally connected to a mock of Fibery (`build-aux/fibery-mock.py`):

```bash
build-aux/ui-test.sh start --fibery --open clients
```

```bash
build-aux/ui-test.sh shot window.png
```

```bash
build-aux/ui-test.sh stop
```

See the comments at the top of `build-aux/ui-test.sh` for the options; `build-aux/sdk-build.sh` alone only builds.

To try what happens when Fibery data goes wrong, the mock has scenarios (deleted logs, hours that read as 0 or empty, logs without a task, no logs at all, a flood of new ones, …); see the top of `build-aux/fibery-mock.py`:

```bash
curl -d '{"scenario": "delete-logs", "count": 6}' 127.0.0.1:8099/mock
```

`--large` adds five years of demo history and 2,000 logs in the mock, and logs main loop stalls over 50 ms and how long syncs and snapshots take, to check the app stays responsive with a lot of data. A timing test of the sync's store work runs with:

```bash
build-aux/cargo-sdk.sh test --release -- --ignored --nocapture
```

The development build keeps its data in `~/.var/app/nl.peteruithoven.TimeTracker.Devel/data/time-tracker/`.

### Screenshots

The screenshots in [`data/screenshots/`](data/screenshots/) (also used in the AppStream metainfo) are rendered from made-up demo data by the installed development build, headless:

```bash
build-aux/screenshots.sh
```

To change what they show, edit the demo data in [`src/demo.rs`](src/demo.rs) or the shots at the bottom of the script.

## Project layout

| Path | What |
| --- | --- |
| `src/` | Rust sources and Blueprint UI files (`*.blp`) |
| `src/store/` | SQLite storage and the store side of syncing |
| `src/fibery/` | Fibery API client, schema detection, network side of syncing |
| `src/invoicing/` | Invoice scripts |
| `data/` | Desktop file, AppStream metainfo, GSettings schema, icons, screenshots |
| `po/` | Translations (gettext) |
| `build-aux/` | Flatpak manifests, build and test scripts, the Fibery mock |

## License

MIT, see [LICENSE](LICENSE).
