//! Local SQLite storage for clients, projects, tasks and time entries.
//!
//! Every change is committed immediately, so a running timer survives a crash
//! or reboot: it is simply an entry with a start time and no end time.

mod models;

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};

pub use models::{Account, Client, Entry, Project, Task};

mod sync;

pub use sync::NewAccount;

pub type Result<T> = rusqlite::Result<T>;

/// Schema migrations, applied in order. `PRAGMA user_version` holds how many
/// have run. Never edit an existing migration; append a new one.
const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE clients (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    billable INTEGER NOT NULL DEFAULT 1,
    hourly_rate_cents INTEGER,
    source TEXT,
    archived INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE TABLE projects (
    id INTEGER PRIMARY KEY,
    client_id INTEGER NOT NULL REFERENCES clients(id),
    name TEXT NOT NULL,
    color TEXT NOT NULL DEFAULT 'blue',
    hourly_rate_cents INTEGER,
    remote_id TEXT,
    archived INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE TABLE tasks (
    id INTEGER PRIMARY KEY,
    client_id INTEGER NOT NULL REFERENCES clients(id),
    project_id INTEGER REFERENCES projects(id),
    name TEXT NOT NULL,
    done INTEGER NOT NULL DEFAULT 0,
    remote_id TEXT,
    last_used_at INTEGER,
    created_at INTEGER NOT NULL
);

CREATE TABLE entries (
    id INTEGER PRIMARY KEY,
    task_id INTEGER NOT NULL REFERENCES tasks(id),
    -- Local calendar date (YYYY-MM-DD) the entry counts towards: the start date.
    date TEXT NOT NULL,
    start_at INTEGER,
    end_at INTEGER,
    -- Only for entries without start/end times (added manually or imported).
    duration_secs INTEGER,
    note TEXT NOT NULL DEFAULT '',
    remote_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (start_at IS NOT NULL OR duration_secs IS NOT NULL)
);

CREATE INDEX entries_date ON entries(date);

-- At most one running timer.
CREATE UNIQUE INDEX entries_one_running ON entries((end_at IS NULL))
    WHERE start_at IS NOT NULL AND end_at IS NULL;
"#, r#"
-- A client connected to a service such as Fibery.
CREATE TABLE accounts (
    id INTEGER PRIMARY KEY,
    client_id INTEGER NOT NULL UNIQUE REFERENCES clients(id),
    service TEXT NOT NULL,
    workspace TEXT NOT NULL,
    user_id TEXT NOT NULL,
    user_name TEXT NOT NULL,
    -- Detected databases and fields (JSON).
    mapping TEXT NOT NULL,
    mirror_timer INTEGER NOT NULL DEFAULT 1,
    -- Only used when no system keyring is available.
    token TEXT,
    last_sync_at INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL
);

-- Changed locally since the last push.
ALTER TABLE entries ADD COLUMN dirty INTEGER NOT NULL DEFAULT 0;
-- Deleted locally, still to be deleted remotely.
ALTER TABLE entries ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE entries ADD COLUMN remote_modified TEXT;

CREATE INDEX entries_remote ON entries(remote_id);
CREATE INDEX tasks_remote ON tasks(remote_id);
CREATE INDEX projects_remote ON projects(remote_id);

CREATE VIEW live_entries AS SELECT * FROM entries WHERE deleted = 0;
"#, r#"
-- Id of an invoice script (see src/invoicing) enabled for the client.
ALTER TABLE clients ADD COLUMN invoice_script TEXT;
"#, r#"
-- Settings of the invoice script (JSON), e.g. a template invoice address.
ALTER TABLE clients ADD COLUMN invoice_settings TEXT;
"#];

const TASK_SELECT: &str = "
    SELECT t.id, t.name, t.client_id, c.name, c.source, t.project_id, p.name, p.color,
           t.remote_id IS NOT NULL, t.last_used_at, t.done
    FROM tasks t
    JOIN clients c ON c.id = t.client_id
    LEFT JOIN projects p ON p.id = t.project_id";

const CLIENT_SELECT: &str = "
    SELECT id, name, billable, hourly_rate_cents, source, archived, invoice_script, invoice_settings FROM clients";

const PROJECT_SELECT: &str = "
    SELECT p.id, p.name, p.color, p.client_id, c.name, p.remote_id IS NOT NULL,
           p.hourly_rate_cents, p.archived
    FROM projects p JOIN clients c ON c.id = p.client_id";

const ENTRY_SELECT: &str = "
    SELECT e.id, e.task_id, t.name, p.name, c.name, p.color, t.remote_id IS NOT NULL,
           e.date, e.start_at, e.end_at, e.duration_secs, e.note,
           t.client_id, c.source, t.project_id
    FROM live_entries e
    JOIN tasks t ON t.id = e.task_id
    JOIN clients c ON c.id = t.client_id
    LEFT JOIN projects p ON p.id = t.project_id";

pub struct Store {
    conn: Connection,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    /// Opens (or creates) the database at `path`. On first use a client named
    /// `default_client` is created, so there is always somewhere to put tasks.
    pub fn open(path: &Path, default_client: &str) -> Result<Self> {
        Self::init(Connection::open(path)?, default_client)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, "Personal")
    }

    fn init(conn: Connection, default_client: &str) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { conn };
        store.migrate()?;
        if store.clients()?.is_empty() {
            let id = store.create_client(default_client, 0)?;
            store.conn.execute("UPDATE clients SET billable = 0 WHERE id = ?1", [id])?;
        }
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", index as i64 + 1)?;
            tx.commit()?;
        }
        Ok(())
    }

    // ---- Clients & projects -------------------------------------------------

    /// Active clients, by name.
    pub fn clients(&self) -> Result<Vec<Client>> {
        let sql = format!("{CLIENT_SELECT} WHERE archived = 0 ORDER BY name COLLATE NOCASE");
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.query_map([], client_from_row)?.collect()
    }

    /// All clients including archived ones (those last).
    pub fn all_clients(&self) -> Result<Vec<Client>> {
        let sql = format!("{CLIENT_SELECT} ORDER BY archived, name COLLATE NOCASE");
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.query_map([], client_from_row)?.collect()
    }

    pub fn client(&self, id: i64) -> Result<Option<Client>> {
        let sql = format!("{CLIENT_SELECT} WHERE id = ?1");
        self.conn.query_row(&sql, [id], client_from_row).optional()
    }

    pub fn create_client(&self, name: &str, now: i64) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO clients (name, created_at) VALUES (?1, ?2)",
            params![name.trim(), now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Saves name, billing and archived state.
    pub fn update_client(&self, client: &Client) -> Result<()> {
        self.conn.execute(
            "UPDATE clients SET name = ?1, billable = ?2, hourly_rate_cents = ?3, archived = ?4
             WHERE id = ?5",
            params![
                client.name.trim(),
                client.billable,
                client.hourly_rate_cents,
                client.archived,
                client.id
            ],
        )?;
        Ok(())
    }

    /// Enables an invoice script with its settings (JSON), or disables it.
    pub fn set_client_invoice_script(&self, id: i64, script: Option<&str>, settings: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE clients SET invoice_script = ?1, invoice_settings = ?2 WHERE id = ?3",
            params![script, settings, id],
        )?;
        Ok(())
    }

    /// Active projects, ordered by client and name.
    pub fn projects(&self) -> Result<Vec<Project>> {
        let sql = format!(
            "{PROJECT_SELECT} WHERE p.archived = 0 AND c.archived = 0
             ORDER BY c.name COLLATE NOCASE, p.name COLLATE NOCASE"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.query_map([], project_from_row)?.collect()
    }

    /// A client's projects including archived ones (those last).
    pub fn client_projects(&self, client_id: i64) -> Result<Vec<Project>> {
        let sql = format!(
            "{PROJECT_SELECT} WHERE p.client_id = ?1 ORDER BY p.archived, p.name COLLATE NOCASE"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.query_map([client_id], project_from_row)?.collect()
    }

    pub fn project(&self, id: i64) -> Result<Option<Project>> {
        let sql = format!("{PROJECT_SELECT} WHERE p.id = ?1");
        self.conn.query_row(&sql, [id], project_from_row).optional()
    }

    pub fn create_project(&self, client_id: i64, name: &str, color: &str, now: i64) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO projects (client_id, name, color, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![client_id, name.trim(), color, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Saves name, color, rate and archived state.
    pub fn update_project(&self, project: &Project) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET name = ?1, color = ?2, hourly_rate_cents = ?3, archived = ?4
             WHERE id = ?5",
            params![
                project.name.trim(),
                project.color,
                project.hourly_rate_cents,
                project.archived,
                project.id
            ],
        )?;
        Ok(())
    }

    // ---- Tasks ---------------------------------------------------------------

    /// Open tasks in active clients and projects, most recently used first.
    pub fn open_tasks(&self) -> Result<Vec<Task>> {
        let sql = format!(
            "{TASK_SELECT}
             WHERE t.done = 0 AND c.archived = 0 AND (p.id IS NULL OR p.archived = 0)
             ORDER BY t.last_used_at DESC NULLS LAST, t.name COLLATE NOCASE"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.query_map([], task_from_row)?.collect()
    }

    pub fn task(&self, id: i64) -> Result<Option<Task>> {
        let sql = format!("{TASK_SELECT} WHERE t.id = ?1");
        self.conn.query_row(&sql, [id], task_from_row).optional()
    }

    /// The task that was tracked most recently, if any.
    pub fn last_used_task(&self) -> Result<Option<Task>> {
        let sql = format!(
            "{TASK_SELECT} WHERE t.last_used_at IS NOT NULL ORDER BY t.last_used_at DESC LIMIT 1"
        );
        self.conn.query_row(&sql, [], task_from_row).optional()
    }

    /// All tasks of a client's project (or of the client without a project),
    /// open ones first.
    pub fn project_tasks(&self, client_id: i64, project_id: Option<i64>) -> Result<Vec<Task>> {
        let sql = format!(
            "{TASK_SELECT} WHERE t.client_id = ?1 AND t.project_id IS ?2
             ORDER BY t.done, t.name COLLATE NOCASE"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.query_map(params![client_id, project_id], task_from_row)?.collect()
    }

    /// Done tasks are hidden from the task picker.
    pub fn set_task_done(&self, id: i64, done: bool) -> Result<()> {
        self.conn.execute("UPDATE tasks SET done = ?1 WHERE id = ?2", params![done, id])?;
        Ok(())
    }

    pub fn create_task(
        &self,
        client_id: i64,
        project_id: Option<i64>,
        name: &str,
        now: i64,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO tasks (client_id, project_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![client_id, project_id, name.trim(), now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    // ---- Entries & timer -----------------------------------------------------

    pub fn running_entry(&self) -> Result<Option<Entry>> {
        let sql = format!("{ENTRY_SELECT} WHERE e.start_at IS NOT NULL AND e.end_at IS NULL");
        self.conn.query_row(&sql, [], entry_from_row).optional()
    }

    /// Entries counting towards `date` (YYYY-MM-DD), newest first.
    pub fn entries_on(&self, date: &str) -> Result<Vec<Entry>> {
        let sql = format!(
            "{ENTRY_SELECT} WHERE e.date = ?1
             ORDER BY e.end_at IS NULL DESC, COALESCE(e.start_at, e.created_at) DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.query_map([date], entry_from_row)?.collect()
    }

    /// Stops the running entry (if any) and starts a new one on `task_id`.
    /// `date` is the local date of `now`.
    pub fn start_timer(&self, task_id: i64, now: i64, date: &str) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        stop_running(&tx, now)?;
        tx.execute(
            "INSERT INTO entries (task_id, date, start_at, created_at, updated_at, dirty)
             VALUES (?1, ?2, ?3, ?3, ?3, 1)",
            params![task_id, date, now],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE tasks SET last_used_at = ?1 WHERE id = ?2",
            params![now, task_id],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Stops the running entry. Returns whether one was running.
    pub fn stop_timer(&self, now: i64) -> Result<bool> {
        Ok(stop_running(&self.conn, now)? > 0)
    }

    /// Leaves an away period out of the running entry: it ends at `away_at`
    /// and the same task continues in a new entry from `back_at` (on
    /// `back_date`), with the same note. If the entry started while away,
    /// its start moves to `back_at` instead. Returns whether one was running.
    pub fn resume_after_away(&self, away_at: i64, back_at: i64, back_date: &str, now: i64) -> Result<bool> {
        let Some(running) = self.running_entry()? else {
            return Ok(false);
        };
        let tx = self.conn.unchecked_transaction()?;
        if running.start_at.is_some_and(|start| start >= away_at) {
            tx.execute(
                "UPDATE entries SET start_at = ?1, date = ?2, updated_at = ?3, dirty = 1 WHERE id = ?4",
                params![back_at, back_date, now, running.id],
            )?;
        } else {
            stop_running(&tx, away_at)?;
            tx.execute(
                "INSERT INTO entries (task_id, date, start_at, note, created_at, updated_at, dirty)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, 1)",
                params![running.task_id, back_date, back_at, running.note, now],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn set_entry_note(&self, entry_id: i64, note: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE entries SET note = ?1, updated_at = ?2, dirty = 1 WHERE id = ?3",
            params![note, now, entry_id],
        )?;
        Ok(())
    }

    pub fn entry(&self, id: i64) -> Result<Option<Entry>> {
        let sql = format!("{ENTRY_SELECT} WHERE e.id = ?1");
        self.conn.query_row(&sql, [id], entry_from_row).optional()
    }

    /// Entries with a date from `first` to `last` (inclusive, YYYY-MM-DD),
    /// oldest first.
    pub fn entries_between(&self, first: &str, last: &str) -> Result<Vec<Entry>> {
        let sql = format!(
            "{ENTRY_SELECT} WHERE e.date BETWEEN ?1 AND ?2
             ORDER BY e.date, COALESCE(e.start_at, e.created_at)"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.query_map([first, last], entry_from_row)?.collect()
    }

    /// Tracked seconds per month of `year` (index 0 = January).
    pub fn month_totals(&self, year: i32, now: i64) -> Result<[i64; 12]> {
        let mut totals = [0; 12];
        for entry in self.entries_between(&format!("{year}-01-01"), &format!("{year}-12-31"))? {
            let month: usize = entry.date.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(1);
            totals[month.clamp(1, 12) - 1] += entry.duration(now);
        }
        Ok(totals)
    }

    /// Adds an entry added after the fact. It must not be a running one.
    pub fn add_entry(&self, input: &EntryInput, now: i64) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO entries (task_id, date, start_at, end_at, duration_secs, note,
                                  created_at, updated_at, dirty)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, 1)",
            params![
                input.task_id,
                input.date,
                input.start_at,
                input.end_at,
                input.duration_secs,
                input.note,
                now
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_entry(&self, id: i64, input: &EntryInput, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE entries SET task_id = ?1, date = ?2, start_at = ?3, end_at = ?4,
                                duration_secs = ?5, note = ?6, updated_at = ?7, dirty = 1
             WHERE id = ?8",
            params![
                input.task_id,
                input.date,
                input.start_at,
                input.end_at,
                input.duration_secs,
                input.note,
                now,
                id
            ],
        )?;
        Ok(())
    }

    /// Deletes an entry. Entries that exist remotely are only marked, so the
    /// next sync can delete them there too.
    pub fn delete_entry(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE entries SET deleted = 1, dirty = 1 WHERE id = ?1 AND remote_id IS NOT NULL",
            [id],
        )?;
        self.conn
            .execute("DELETE FROM entries WHERE id = ?1 AND remote_id IS NULL", [id])?;
        Ok(())
    }
}

/// Values for adding or editing an entry. Either `start_at` (with `end_at`,
/// or without it for the running entry) or `duration_secs` is set.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryInput {
    pub task_id: i64,
    pub date: String,
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
    pub duration_secs: Option<i64>,
    pub note: String,
}

fn stop_running(conn: &Connection, now: i64) -> Result<usize> {
    conn.execute(
        "UPDATE entries SET end_at = MAX(start_at, ?1), updated_at = ?1, dirty = 1
         WHERE start_at IS NOT NULL AND end_at IS NULL AND deleted = 0",
        [now],
    )
}

fn client_from_row(row: &Row) -> Result<Client> {
    Ok(Client {
        id: row.get(0)?,
        name: row.get(1)?,
        billable: row.get(2)?,
        hourly_rate_cents: row.get(3)?,
        source: row.get(4)?,
        archived: row.get(5)?,
        invoice_script: row.get(6)?,
        invoice_settings: row.get(7)?,
    })
}

fn project_from_row(row: &Row) -> Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        color: row.get(2)?,
        client_id: row.get(3)?,
        client_name: row.get(4)?,
        is_remote: row.get(5)?,
        hourly_rate_cents: row.get(6)?,
        archived: row.get(7)?,
    })
}

fn task_from_row(row: &Row) -> Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        name: row.get(1)?,
        client_id: row.get(2)?,
        client_name: row.get(3)?,
        client_is_remote: row.get::<_, Option<String>>(4)?.is_some(),
        project_id: row.get(5)?,
        project_name: row.get(6)?,
        color: row.get(7)?,
        is_remote: row.get(8)?,
        last_used_at: row.get(9)?,
        done: row.get(10)?,
    })
}

fn entry_from_row(row: &Row) -> Result<Entry> {
    Ok(Entry {
        id: row.get(0)?,
        task_id: row.get(1)?,
        task_name: row.get(2)?,
        project_name: row.get(3)?,
        client_name: row.get(4)?,
        color: row.get(5)?,
        is_remote: row.get(6)?,
        date: row.get(7)?,
        start_at: row.get(8)?,
        end_at: row.get(9)?,
        duration_secs: row.get(10)?,
        note: row.get(11)?,
        client_id: row.get(12)?,
        client_is_remote: row.get::<_, Option<String>>(13)?.is_some(),
        project_id: row.get(14)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_task() -> (Store, i64, i64) {
        let store = Store::open_in_memory().unwrap();
        let client = store.clients().unwrap()[0].id;
        let project = store.create_project(client, "Website", "green", 0).unwrap();
        let task = store.create_task(client, Some(project), "Design", 0).unwrap();
        (store, client, task)
    }

    #[test]
    fn creates_default_client() {
        let store = Store::open_in_memory().unwrap();
        let clients = store.clients().unwrap();
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].name, "Personal");
        assert!(!clients[0].billable);
    }

    #[test]
    fn resuming_after_away_leaves_the_away_time_out() {
        let (store, _, task) = store_with_task();
        let entry = store.start_timer(task, 1_000, "2026-11-19").unwrap();
        store.set_entry_note(entry, "Sketches", 1_000).unwrap();

        assert!(store.resume_after_away(1_600, 4_000, "2026-11-19", 4_000).unwrap());

        let first = store.entry(entry).unwrap().unwrap();
        assert_eq!((first.start_at, first.end_at), (Some(1_000), Some(1_600)));
        let running = store.running_entry().unwrap().unwrap();
        assert_eq!(running.task_id, task);
        assert_eq!(running.start_at, Some(4_000));
        assert_eq!(running.note, "Sketches");
    }

    #[test]
    fn resuming_after_away_moves_an_entry_started_while_away() {
        let (store, _, task) = store_with_task();
        let entry = store.start_timer(task, 2_000, "2026-11-19").unwrap();

        store.resume_after_away(1_600, 4_000, "2026-11-20", 4_000).unwrap();

        let running = store.running_entry().unwrap().unwrap();
        assert_eq!(running.id, entry);
        assert_eq!(running.start_at, Some(4_000));
        assert_eq!(running.date, "2026-11-20");
    }

    #[test]
    fn resuming_after_away_without_a_timer_does_nothing() {
        let (store, _, _) = store_with_task();
        assert!(!store.resume_after_away(1_600, 4_000, "2026-11-19", 4_000).unwrap());
        assert!(store.running_entry().unwrap().is_none());
    }

    #[test]
    fn starting_a_task_stops_the_running_one() {
        let (store, client, first) = store_with_task();
        let second = store.create_task(client, None, "Admin", 0).unwrap();

        store.start_timer(first, 1_000, "2026-11-19").unwrap();
        store.start_timer(second, 1_600, "2026-11-19").unwrap();

        let running = store.running_entry().unwrap().unwrap();
        assert_eq!(running.task_id, second);
        assert_eq!(running.start_at, Some(1_600));

        let entries = store.entries_on("2026-11-19").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].task_id, second, "running entry comes first");
        assert_eq!(entries[1].end_at, Some(1_600));
        assert_eq!(entries[1].duration(2_000), 600);
        assert_eq!(entries[0].duration(2_000), 400);
    }

    #[test]
    fn stop_timer_ends_running_entry() {
        let (store, _, task) = store_with_task();
        assert!(!store.stop_timer(10).unwrap());

        store.start_timer(task, 100, "2026-11-19").unwrap();
        assert!(store.stop_timer(160).unwrap());
        assert!(store.running_entry().unwrap().is_none());

        let entry = &store.entries_on("2026-11-19").unwrap()[0];
        assert_eq!(entry.duration(9_999), 60);
    }

    #[test]
    fn stop_never_ends_before_start() {
        let (store, _, task) = store_with_task();
        store.start_timer(task, 500, "2026-11-19").unwrap();
        store.stop_timer(400).unwrap(); // clock went backwards
        assert_eq!(store.entries_on("2026-11-19").unwrap()[0].duration(0), 0);
    }

    #[test]
    fn tracks_last_used_task_and_notes() {
        let (store, _, task) = store_with_task();
        assert!(store.last_used_task().unwrap().is_none());

        let entry = store.start_timer(task, 100, "2026-11-19").unwrap();
        store.set_entry_note(entry, "Wireframes", 120).unwrap();

        let last = store.last_used_task().unwrap().unwrap();
        assert_eq!(last.id, task);
        assert_eq!(last.project_name.as_deref(), Some("Website"));
        assert_eq!(store.running_entry().unwrap().unwrap().note, "Wireframes");
    }

    #[test]
    fn adds_updates_and_deletes_entries() {
        let (store, client, task) = store_with_task();
        let other = store.create_task(client, None, "Admin", 0).unwrap();
        let mut input = EntryInput {
            task_id: task,
            date: "2026-11-17".into(),
            start_at: Some(1_000),
            end_at: Some(4_600),
            duration_secs: None,
            note: "Call".into(),
        };
        let id = store.add_entry(&input, 5_000).unwrap();
        let entry = store.entry(id).unwrap().unwrap();
        assert_eq!(entry.duration(0), 3_600);
        assert_eq!(entry.project_name.as_deref(), Some("Website"));
        assert_eq!(entry.client_id, client);

        input.task_id = other;
        input.start_at = None;
        input.end_at = None;
        input.duration_secs = Some(1_800);
        store.update_entry(id, &input, 6_000).unwrap();
        let entry = store.entry(id).unwrap().unwrap();
        assert_eq!((entry.task_id, entry.duration(0), entry.project_id), (other, 1_800, None));

        store.delete_entry(id).unwrap();
        assert!(store.entry(id).unwrap().is_none());
    }

    #[test]
    fn sums_month_totals_and_ranges() {
        let (store, _, task) = store_with_task();
        let manual = |date: &str, secs| EntryInput {
            task_id: task,
            date: date.into(),
            start_at: None,
            end_at: None,
            duration_secs: Some(secs),
            note: String::new(),
        };
        store.add_entry(&manual("2026-10-31", 600), 0).unwrap();
        store.add_entry(&manual("2026-11-01", 60), 0).unwrap();
        store.add_entry(&manual("2026-11-30", 120), 0).unwrap();
        store.add_entry(&manual("2027-01-01", 9_999), 0).unwrap();

        let november = store.entries_between("2026-11-01", "2026-11-30").unwrap();
        assert_eq!(november.len(), 2);
        let totals = store.month_totals(2026, 0).unwrap();
        assert_eq!((totals[9], totals[10], totals[0]), (600, 180, 0));
    }

    #[test]
    fn manages_clients_projects_and_tasks() {
        let (store, client_id, task) = store_with_task();
        let mut client = store.client(client_id).unwrap().unwrap();
        client.name = "Me".into();
        client.hourly_rate_cents = Some(8_500);
        client.archived = true;
        store.update_client(&client).unwrap();
        assert!(store.clients().unwrap().is_empty(), "archived clients are hidden");
        assert_eq!(store.all_clients().unwrap()[0], client);
        assert!(store.open_tasks().unwrap().is_empty(), "so are their tasks");

        client.archived = false;
        store.update_client(&client).unwrap();
        let mut project = store.client_projects(client_id).unwrap().remove(0);
        project.color = "red".into();
        project.hourly_rate_cents = Some(9_000);
        project.archived = true;
        store.update_project(&project).unwrap();
        assert_eq!(store.project(project.id).unwrap().unwrap(), project);
        assert!(store.projects().unwrap().is_empty());
        assert!(store.open_tasks().unwrap().is_empty(), "tasks of archived projects are hidden");

        project.archived = false;
        store.update_project(&project).unwrap();
        store.set_task_done(task, true).unwrap();
        assert!(store.open_tasks().unwrap().is_empty(), "done tasks are hidden");
        let tasks = store.project_tasks(client_id, Some(project.id)).unwrap();
        assert!(tasks[0].done);
        let no_project = store.create_task(client_id, None, "Loose", 0).unwrap();
        assert_eq!(store.project_tasks(client_id, None).unwrap()[0].id, no_project);
    }

    #[test]
    fn migrations_are_idempotent() {
        let store = Store::open_in_memory().unwrap();
        store.migrate().unwrap();
        assert_eq!(store.clients().unwrap().len(), 1);
    }
}
