//! Automatic snapshots of the database, just in case: one a day while the app
//! runs, and one right before a sync removes entries.
//!
//! A snapshot is a complete copy of the SQLite database made with
//! `VACUUM INTO`, without API keys, as one self-contained file. It's written
//! to a temporary file in the app's own data folder first (so the key never
//! reaches the chosen folder), then copied there as `….db.part` and renamed,
//! so sync clients like Nextcloud never pick up a half-written file.
//!
//! Files are named `time-tracker[-devel]-<host>-YYYY-MM-DD-HHMMSS[-before-sync].db`,
//! so several computers and the development build can share one folder;
//! each only ever removes its own snapshots (see [`expired`] for what's kept).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use gtk::prelude::*;
use gtk::{gio, glib};
use rusqlite::{Connection, OpenFlags};

use crate::config::{APP_ID, PROFILE};
use crate::format;

/// Daily snapshots: the newest of each of the last 7 days that have one…
pub const KEEP_DAILY: usize = 7;
/// …of each of the last 4 weeks (Monday to Sunday)…
pub const KEEP_WEEKLY: usize = 4;
/// …and of each of the last 12 months.
pub const KEEP_MONTHLY: usize = 12;
/// Snapshots taken before a sync: those of the last 30 days, at most 20.
pub const KEEP_BEFORE_SYNC_DAYS: i64 = 30;
pub const KEEP_BEFORE_SYNC_MAX: usize = 20;

/// How often to check whether today's snapshot exists yet.
const CHECK_INTERVAL: u32 = 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Daily,
    BeforeSync,
}

/// A snapshot's file name, taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    /// `time-tracker[-devel]-<host>`: which app and computer made it.
    pub prefix: String,
    /// Local date, `YYYY-MM-DD`.
    pub date: String,
    /// Local time, `HHMMSS`.
    pub time: String,
    pub kind: Kind,
}

impl Name {
    /// A new snapshot of this app on this computer, at the current time.
    pub fn now(kind: Kind) -> Self {
        let now = glib::DateTime::now_local().expect("local time");
        let stamp = |f: &str| now.format(f).map(|s| s.to_string()).unwrap_or_default();
        Self { prefix: own_prefix(), date: stamp("%Y-%m-%d"), time: stamp("%H%M%S"), kind }
    }

    pub fn parse(file: &str) -> Option<Self> {
        let stem = file.strip_suffix(".db")?;
        let (stem, kind) = match stem.strip_suffix("-before-sync") {
            Some(stem) => (stem, Kind::BeforeSync),
            None => (stem, Kind::Daily),
        };
        // `<prefix>-YYYY-MM-DD-HHMMSS`
        let split = stem.len().checked_sub(17)?;
        let (prefix, stamp) = (stem.get(..split)?, stem.get(split..)?);
        let prefix = prefix.strip_suffix('-')?;
        if !prefix.starts_with("time-tracker") {
            return None;
        }
        let digits_at = |range: std::ops::Range<usize>| stamp[range].bytes().all(|b| b.is_ascii_digit());
        let shape_ok = stamp.as_bytes()[4] == b'-' && stamp.as_bytes()[7] == b'-' && stamp.as_bytes()[10] == b'-';
        if !shape_ok || !digits_at(0..4) || !digits_at(5..7) || !digits_at(8..10) || !digits_at(11..17) {
            return None;
        }
        Some(Self { prefix: prefix.to_string(), date: stamp[..10].to_string(), time: stamp[11..].to_string(), kind })
    }

    pub fn file_name(&self) -> String {
        let suffix = match self.kind {
            Kind::Daily => "",
            Kind::BeforeSync => "-before-sync",
        };
        format!("{}-{}-{}{suffix}.db", self.prefix, self.date, self.time)
    }

    /// Unix time it was taken (local time), for showing it.
    pub fn timestamp(&self) -> Option<i64> {
        let (year, month, day) = format::date_parts(&self.date)?;
        let num = |range: std::ops::Range<usize>| self.time.get(range)?.parse::<i32>().ok();
        let dt = glib::DateTime::from_local(year, month as i32, day as i32, num(0..2)?, num(2..4)?, f64::from(num(4..6)?)).ok()?;
        Some(dt.to_unix())
    }

    fn sort_key(&self) -> (&str, &str) {
        (&self.date, &self.time)
    }
}

/// `time-tracker[-devel]-<host>`, with the host name reduced to letters,
/// digits and dashes.
pub fn own_prefix() -> String {
    let host: String = glib::host_name()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let devel = if PROFILE == "Devel" { "-devel" } else { "" };
    format!("time-tracker{devel}-{}", host.trim_matches('-'))
}

/// ISO week (year, week) of a `YYYY-MM-DD` date.
fn week_of(date: &str) -> Option<(i32, i32)> {
    let (year, month, day) = format::date_parts(date)?;
    let dt = glib::DateTime::from_local(year, month as i32, day as i32, 12, 0, 0.0).ok()?;
    Some((dt.week_numbering_year(), dt.week_of_year()))
}

/// Of `names` (this app's own snapshots), the ones retention removes.
/// `before_sync_cutoff` is the oldest date (`YYYY-MM-DD`) a before-sync
/// snapshot is kept for.
pub fn expired<'a>(names: &'a [Name], before_sync_cutoff: &str) -> Vec<&'a Name> {
    let mut sorted: Vec<&Name> = names.iter().collect();
    sorted.sort_by(|a, b| b.sort_key().cmp(&a.sort_key()));
    let mut keep = std::collections::HashSet::new();

    // Newest snapshot of each of the most recent periods.
    let daily: Vec<&&Name> = sorted.iter().filter(|n| n.kind == Kind::Daily).collect();
    let mut keep_newest_per = |period: &dyn Fn(&Name) -> Option<String>, count: usize| {
        let mut seen = Vec::new();
        for name in &daily {
            let Some(key) = period(name) else { continue };
            if seen.contains(&key) {
                continue;
            }
            if seen.len() == count {
                break;
            }
            seen.push(key);
            keep.insert(name.file_name());
        }
    };
    keep_newest_per(&|n| Some(n.date.clone()), KEEP_DAILY);
    keep_newest_per(&|n| week_of(&n.date).map(|(y, w)| format!("{y}-{w}")), KEEP_WEEKLY);
    keep_newest_per(&|n| Some(n.date[..7].to_string()), KEEP_MONTHLY);

    let recent_before_sync = sorted
        .iter()
        .filter(|n| n.kind == Kind::BeforeSync && n.date.as_str() >= before_sync_cutoff)
        .take(KEEP_BEFORE_SYNC_MAX);
    for name in recent_before_sync {
        keep.insert(name.file_name());
    }

    sorted.into_iter().filter(|n| !keep.contains(&n.file_name())).collect()
}

/// What's in a backup folder.
#[derive(Debug, Default)]
pub struct Contents {
    /// This app's snapshots from this computer.
    pub own: Vec<Name>,
    /// Snapshots from other computers or the other build.
    pub others: Vec<Name>,
    /// Anything else (files and folders), except hidden ones.
    pub other_files: usize,
}

impl Contents {
    pub fn newest(&self) -> Option<&Name> {
        self.own.iter().max_by(|a, b| a.sort_key().cmp(&b.sort_key()))
    }

    /// Other computers' host names, from their snapshot names.
    pub fn other_hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = self
            .others
            .iter()
            .map(|n| n.prefix.trim_start_matches("time-tracker-devel-").trim_start_matches("time-tracker-").to_string())
            .collect();
        hosts.sort();
        hosts.dedup();
        hosts
    }
}

pub fn scan(folder: &Path) -> std::io::Result<Contents> {
    let own = own_prefix();
    let mut contents = Contents::default();
    for entry in fs::read_dir(folder)? {
        let file = entry?.file_name();
        let file = file.to_string_lossy();
        if file.starts_with('.') || file.ends_with(".db.part") && Name::parse(file.trim_end_matches(".part")).is_some() {
            continue;
        }
        match Name::parse(&file) {
            Some(name) if name.prefix == own => contents.own.push(name),
            Some(name) => contents.others.push(name),
            None => contents.other_files += 1,
        }
    }
    Ok(contents)
}

/// Writes a snapshot of the database at `db` as `folder/<name>`, going
/// through a temporary file in `staging`. Blocking.
pub fn write(db: &Path, staging: &Path, folder: &Path, name: &str) -> Result<PathBuf, String> {
    let tmp = staging.join(format!(".{name}.tmp"));
    let _ = fs::remove_file(&tmp);
    let result = write_via(db, &tmp, folder, name);
    let _ = fs::remove_file(&tmp);
    result
}

fn write_via(db: &Path, tmp: &Path, folder: &Path, name: &str) -> Result<PathBuf, String> {
    let sql_err = |e: rusqlite::Error| e.to_string();
    {
        // A second, read-only connection: in WAL mode it doesn't block the app.
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let source = Connection::open_with_flags(db, flags).map_err(sql_err)?;
        let tmp_str = tmp.to_str().ok_or("Path is not valid UTF-8")?;
        source.execute("VACUUM INTO ?1", [tmp_str]).map_err(sql_err)?;
    }
    scrub(tmp).map_err(sql_err)?;

    fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let target = folder.join(name);
    let part = folder.join(format!("{name}.part"));
    fs::copy(tmp, &part).map_err(|e| e.to_string())?;
    fs::File::open(&part).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
    fs::rename(&part, &target).map_err(|e| {
        let _ = fs::remove_file(&part);
        e.to_string()
    })?;
    Ok(target)
}

/// Leaves API keys out of a copy, without leaving their bytes behind in free
/// pages, and makes it one self-contained file (no `-wal`).
fn scrub(path: &Path) -> rusqlite::Result<()> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "DELETE")?;
    conn.pragma_update(None, "secure_delete", true)?;
    conn.execute("UPDATE accounts SET token = NULL", [])?;
    conn.execute_batch("VACUUM")
}

/// Removes this computer's snapshots that retention doesn't keep, and
/// leftover `.part` files of earlier attempts. Blocking.
pub fn prune(folder: &Path, before_sync_cutoff: &str) -> std::io::Result<usize> {
    let contents = scan(folder)?;
    let mut removed = 0;
    for name in expired(&contents.own, before_sync_cutoff) {
        match fs::remove_file(folder.join(name.file_name())) {
            Ok(()) => removed += 1,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    let own = own_prefix();
    for entry in fs::read_dir(folder)?.flatten() {
        let file = entry.file_name().to_string_lossy().to_string();
        if let Some(name) = file.strip_suffix(".part").and_then(Name::parse)
            && name.prefix == own
        {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(removed)
}

// ---- In the app --------------------------------------------------------------

/// The default folder: next to the database, in the app's data folder.
pub fn default_folder() -> PathBuf {
    crate::application::data_dir().join("backups")
}

/// The folder chosen in Preferences, if any.
pub fn chosen_folder() -> Option<PathBuf> {
    let folder = gio::Settings::new(APP_ID).string("backup-folder");
    (!folder.is_empty()).then(|| PathBuf::from(folder.as_str()))
}

/// Where snapshots go now, and whether that's the default folder because
/// the chosen one isn't available (e.g. an unmounted drive).
pub fn folder() -> (PathBuf, bool) {
    match chosen_folder() {
        Some(folder) if folder.is_dir() => (folder, false),
        Some(_) => (default_folder(), true),
        None => (default_folder(), false),
    }
}

/// Takes a snapshot now and prunes old ones, off the main thread.
pub async fn snapshot(kind: Kind) -> Result<PathBuf, String> {
    let app = crate::application::TimeTrackerApplication::get();
    let db = app.store().path().ok_or("The database has no file")?;
    let staging = crate::application::data_dir();
    let (folder, _) = folder();
    let name = Name::now(kind).file_name();
    let cutoff = format::local_date(format::now() - KEEP_BEFORE_SYNC_DAYS * 24 * 60 * 60);
    let result = gio::spawn_blocking(move || {
        let started = Instant::now();
        let path = write(&db, &staging, &folder, &name)?;
        let written = started.elapsed();
        if let Err(err) = prune(&folder, &cutoff) {
            glib::g_warning!("time-tracker", "Could not remove old snapshots: {err}");
        }
        glib::g_debug!(
            "time-tracker",
            "Snapshot {} written in {} ms, pruned in {} ms",
            path.display(),
            written.as_millis(),
            (started.elapsed() - written).as_millis()
        );
        Ok(path)
    })
    .await
    .map_err(|_| "Taking a snapshot failed unexpectedly".to_string())?;
    if let Err(err) = &result {
        glib::g_warning!("time-tracker", "Could not take a snapshot: {err}");
    }
    result
}

/// Takes today's snapshot if there isn't one yet.
fn snapshot_if_due() {
    let (folder, _) = folder();
    let today = format::today();
    let done = scan(&folder).is_ok_and(|c| c.own.iter().any(|n| n.kind == Kind::Daily && n.date == today));
    if !done {
        glib::spawn_future_local(async {
            let _ = snapshot(Kind::Daily).await;
        });
    }
}

/// Starts daily snapshots; call once at startup.
pub fn start() {
    glib::timeout_add_seconds_local_once(60, snapshot_if_due);
    glib::timeout_add_seconds_local(CHECK_INTERVAL, || {
        snapshot_if_due();
        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(date: &str, time: &str, kind: Kind) -> Name {
        Name { prefix: own_prefix(), date: date.into(), time: time.into(), kind }
    }

    #[test]
    fn parses_and_formats_names() {
        let n = Name::parse("time-tracker-my-laptop-2026-11-20-093000-before-sync.db").unwrap();
        assert_eq!(n.prefix, "time-tracker-my-laptop");
        assert_eq!(n.date, "2026-11-20");
        assert_eq!(n.time, "093000");
        assert_eq!(n.kind, Kind::BeforeSync);
        assert_eq!(n.file_name(), "time-tracker-my-laptop-2026-11-20-093000-before-sync.db");
        assert_eq!(Name::parse("time-tracker-desk-2026-11-20-093000.db").unwrap().kind, Kind::Daily);
        for other in ["notes.db", "time-tracker.db", "time-tracker-desk-2026-1-20-093000.db", "photo-2026-11-20-093000.db", "time-tracker-desk-2026-11-20-0930xx.db"] {
            assert!(Name::parse(other).is_none(), "{other}");
        }
    }

    #[test]
    fn keeps_7_daily_4_weekly_12_monthly() {
        // A snapshot every day (and a second one on some days) for 400 days,
        // ending Friday 20 November 2026.
        let end = glib::DateTime::from_local(2026, 11, 20, 12, 0, 0.0).unwrap();
        let mut names = Vec::new();
        for days_ago in 0..400 {
            let date = end.add_days(-days_ago).unwrap().format("%Y-%m-%d").unwrap().to_string();
            names.push(name(&date, "090000", Kind::Daily));
            if days_ago % 5 == 0 {
                names.push(name(&date, "170000", Kind::Daily));
            }
        }
        let expired = expired(&names, "2026-10-21");
        let kept: Vec<String> = names
            .iter()
            .filter(|n| !expired.contains(n))
            .map(|n| format!("{} {}", n.date, n.time))
            .collect();
        assert_eq!(
            kept,
            [
                // Days: 14–20 Nov (the 20th and 15th have two; the newest stays).
                "2026-11-20 170000",
                "2026-11-19 090000",
                "2026-11-18 090000",
                "2026-11-17 090000",
                "2026-11-16 090000",
                "2026-11-15 170000",
                "2026-11-14 090000",
                // Weeks: this week and the 15th's are covered; the newest
                // (Sundays) of the two weeks before.
                "2026-11-08 090000",
                "2026-11-01 090000",
                // Months: November is covered; the newest of each (last
                // days) back to December 2025.
                "2026-10-31 170000",
                "2026-09-30 090000",
                "2026-08-31 090000",
                "2026-07-31 090000",
                "2026-06-30 090000",
                "2026-05-31 090000",
                "2026-04-30 090000",
                "2026-03-31 090000",
                "2026-02-28 170000",
                "2026-01-31 090000",
                "2025-12-31 090000",
            ]
        );
    }

    #[test]
    fn keeps_recent_before_sync_snapshots() {
        let mut names = vec![name("2026-09-01", "100000", Kind::BeforeSync), name("2026-11-20", "080000", Kind::Daily)];
        for i in 0..25 {
            names.push(name("2026-11-19", &format!("10{i:02}00"), Kind::BeforeSync));
        }
        let expired = expired(&names, "2026-10-21");
        assert_eq!(expired.len(), 6, "the old one and the 5 oldest of the 25 recent ones");
        assert!(expired.iter().all(|n| n.kind == Kind::BeforeSync));
        assert!(expired.iter().any(|n| n.date == "2026-09-01"));
        assert!(!expired.iter().any(|n| n.time == "102400"), "the newest stays");
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("time-tracker-backup-test-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn snapshot_leaves_out_the_token_and_stands_alone() {
        let dir = temp_dir("write");
        let db = dir.join("time-tracker.db");
        let token = "secret-token-1234567890";
        {
            let store = crate::store::Store::open(&db, "Personal").unwrap();
            let client = store.clients().unwrap()[0].id;
            let account = store
                .add_account(
                    &crate::store::NewAccount {
                        client_id: client,
                        service: "fibery",
                        workspace: "https://acme.fibery.io",
                        user_id: "me",
                        user_name: "Demo",
                        mapping: "{}",
                    },
                    0,
                )
                .unwrap();
            store.set_account_token(account, Some(token)).unwrap();
            // Keep it open: the app holds the database while snapshotting.
            let folder = dir.join("backups");
            let path = write(&db, &dir, &folder, "time-tracker-test-2026-11-20-090000.db").unwrap();
            store.set_account_token(account, None).unwrap();

            let bytes = fs::read(&path).unwrap();
            assert!(!bytes.windows(token.len()).any(|w| w == token.as_bytes()), "token bytes left in the file");
            assert!(!folder.join("time-tracker-test-2026-11-20-090000.db-wal").exists());
            assert!(fs::read_dir(&folder).unwrap().count() == 1, "no .part or other files left");
            assert!(fs::read_dir(&dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")));

            let copy = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            let mode: String = copy.pragma_query_value(None, "journal_mode", |r| r.get(0)).unwrap();
            assert_eq!(mode, "delete");
            let version: i64 = copy.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
            let live: i64 = Connection::open(&db).unwrap().pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
            assert_eq!(version, live);
            let accounts: i64 = copy.query_row("SELECT COUNT(*) FROM accounts WHERE token IS NULL", [], |r| r.get(0)).unwrap();
            assert_eq!(accounts, 1);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_while_the_app_writes() {
        let dir = temp_dir("concurrent");
        let db = dir.join("time-tracker.db");
        let store = crate::store::Store::open(&db, "Personal").unwrap();
        // A write transaction stays open while the snapshot is taken, and
        // can still commit afterwards.
        let writer = Connection::open(&db).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE; UPDATE clients SET name = 'Changed';").unwrap();
        let path = write(&db, &dir, &dir.join("backups"), "time-tracker-test-2026-11-20-090000.db").unwrap();
        writer.execute_batch("COMMIT").unwrap();
        let copy = Connection::open(&path).unwrap();
        let name: String = copy.query_row("SELECT name FROM clients", [], |r| r.get(0)).unwrap();
        assert_eq!(name, "Personal", "the snapshot shows the state before the open transaction");
        assert_eq!(store.clients().unwrap()[0].name, "Changed");
        drop(store);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prunes_only_own_snapshots() {
        let dir = temp_dir("prune");
        let own = own_prefix();
        let mut files = vec!["notes.txt".to_string(), "time-tracker-other-host-2020-01-01-090000.db".to_string()];
        for day in 1..=9 {
            files.push(format!("{own}-2026-11-{day:02}-090000.db"));
        }
        files.push(format!("{own}-2026-11-09-100000.db.part"));
        for file in &files {
            fs::write(dir.join(file), b"x").unwrap();
        }
        let removed = prune(&dir, "2026-10-21").unwrap();
        // Days keep 3–9 Nov; Sunday the 1st is the newest of its week, so
        // only the 2nd goes.
        let left: Vec<String> = {
            let mut v: Vec<String> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
            v.sort();
            v
        };
        assert!(left.contains(&"notes.txt".to_string()));
        assert!(left.contains(&"time-tracker-other-host-2020-01-01-090000.db".to_string()));
        assert!(!left.iter().any(|f| f.ends_with(".part")));
        assert_eq!(removed, 1, "only the 2nd: the 1st is the newest of its week; left: {left:?}");
        let contents = scan(&dir).unwrap();
        assert_eq!(contents.own.len(), 8);
        assert_eq!(contents.other_files, 1);
        assert_eq!(contents.other_hosts(), ["other-host"]);
        let _ = fs::remove_dir_all(&dir);
    }
}
