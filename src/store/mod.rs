//! Local SQLite storage for clients, projects, tasks and time entries.
//!
//! Every change is committed immediately, so a running timer survives a crash
//! or reboot: it is simply an entry with a start time and no end time.

mod models;

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};

pub use models::{Client, Entry, Project, Task};

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
"#];

const TASK_SELECT: &str = "
    SELECT t.id, t.name, t.client_id, c.name, c.source, t.project_id, p.name, p.color,
           t.remote_id IS NOT NULL, t.last_used_at
    FROM tasks t
    JOIN clients c ON c.id = t.client_id
    LEFT JOIN projects p ON p.id = t.project_id";

const ENTRY_SELECT: &str = "
    SELECT e.id, e.task_id, t.name, p.name, c.name, p.color, t.remote_id IS NOT NULL,
           e.date, e.start_at, e.end_at, e.duration_secs, e.note
    FROM entries e
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

    pub fn clients(&self) -> Result<Vec<Client>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, billable, hourly_rate_cents, source
             FROM clients WHERE archived = 0 ORDER BY name COLLATE NOCASE",
        )?;
        stmt.query_map([], |row| {
            Ok(Client {
                id: row.get(0)?,
                name: row.get(1)?,
                billable: row.get(2)?,
                hourly_rate_cents: row.get(3)?,
                source: row.get(4)?,
            })
        })?
        .collect()
    }

    pub fn create_client(&self, name: &str, now: i64) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO clients (name, created_at) VALUES (?1, ?2)",
            params![name.trim(), now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Active projects, ordered by client and name.
    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.name, p.color, p.client_id, c.name, p.remote_id IS NOT NULL
             FROM projects p JOIN clients c ON c.id = p.client_id
             WHERE p.archived = 0 AND c.archived = 0
             ORDER BY c.name COLLATE NOCASE, p.name COLLATE NOCASE",
        )?;
        stmt.query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                client_id: row.get(3)?,
                client_name: row.get(4)?,
                is_remote: row.get(5)?,
            })
        })?
        .collect()
    }

    pub fn create_project(&self, client_id: i64, name: &str, color: &str, now: i64) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO projects (client_id, name, color, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![client_id, name.trim(), color, now],
        )?;
        Ok(self.conn.last_insert_rowid())
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
            "INSERT INTO entries (task_id, date, start_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3, ?3)",
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

    pub fn set_entry_note(&self, entry_id: i64, note: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE entries SET note = ?1, updated_at = ?2 WHERE id = ?3",
            params![note, now, entry_id],
        )?;
        Ok(())
    }
}

fn stop_running(conn: &Connection, now: i64) -> Result<usize> {
    conn.execute(
        "UPDATE entries SET end_at = MAX(start_at, ?1), updated_at = ?1
         WHERE start_at IS NOT NULL AND end_at IS NULL",
        [now],
    )
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
    fn migrations_are_idempotent() {
        let store = Store::open_in_memory().unwrap();
        store.migrate().unwrap();
        assert_eq!(store.clients().unwrap().len(), 1);
    }
}
