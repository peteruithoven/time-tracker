//! Store side of syncing with a connected service: accounts, the changes to
//! push, and applying pushed and pulled data.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, Row, params};

use super::{Account, Result, Store};
use crate::fibery::sync::{LogFields, Push, PushResult, Pulled, RemoteLog, RemoteProject, RemoteTask, TaskRef, hours};

const ACCOUNT_SELECT: &str = "
    SELECT id, client_id, service, workspace, user_id, user_name, mapping, mirror_timer,
           token, last_sync_at, last_error, created_at
    FROM accounts";

/// Colors handed out to projects that come from a service.
const SYNCED_COLORS: &[&str] = &["blue", "green", "orange", "purple", "red", "yellow", "brown"];

fn account_from_row(row: &Row) -> Result<Account> {
    Ok(Account {
        id: row.get(0)?,
        client_id: row.get(1)?,
        service: row.get(2)?,
        workspace: row.get(3)?,
        user_id: row.get(4)?,
        user_name: row.get(5)?,
        mapping: row.get(6)?,
        mirror_timer: row.get(7)?,
        token: row.get(8)?,
        last_sync_at: row.get(9)?,
        last_error: row.get(10)?,
        created_at: row.get(11)?,
    })
}

/// What a new connection needs.
#[derive(Debug, Clone)]
pub struct NewAccount<'a> {
    pub client_id: i64,
    pub service: &'a str,
    pub workspace: &'a str,
    pub user_id: &'a str,
    pub user_name: &'a str,
    pub mapping: &'a str,
}

impl Store {
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let mut stmt = self.conn.prepare(&format!("{ACCOUNT_SELECT} ORDER BY id"))?;
        stmt.query_map([], account_from_row)?.collect()
    }

    pub fn account_for_client(&self, client_id: i64) -> Result<Option<Account>> {
        let sql = format!("{ACCOUNT_SELECT} WHERE client_id = ?1");
        self.conn.query_row(&sql, [client_id], account_from_row).optional()
    }

    pub fn add_account(&self, new: &NewAccount, now: i64) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO accounts (client_id, service, workspace, user_id, user_name, mapping, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![new.client_id, new.service, new.workspace, new.user_id, new.user_name, new.mapping, now],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute("UPDATE clients SET source = ?1 WHERE id = ?2", params![new.service, new.client_id])?;
        tx.commit()?;
        Ok(id)
    }

    /// Disconnects. Synced projects, tasks and entries stay as local data.
    pub fn remove_account(&self, id: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE clients SET source = NULL WHERE id = (SELECT client_id FROM accounts WHERE id = ?1)",
            [id],
        )?;
        tx.execute("DELETE FROM accounts WHERE id = ?1", [id])?;
        tx.commit()
    }

    /// Demo data only: makes a client's projects and tasks look synced.
    pub fn demo_mark_remote(&self, client_id: i64) -> Result<()> {
        self.conn.execute("UPDATE clients SET source = 'fibery' WHERE id = ?1", [client_id])?;
        self.conn.execute(
            "UPDATE projects SET remote_id = 'demo-' || id WHERE client_id = ?1",
            [client_id],
        )?;
        self.conn
            .execute("UPDATE tasks SET remote_id = 'demo-' || id WHERE client_id = ?1", [client_id])?;
        Ok(())
    }

    pub fn set_account_mirror(&self, id: i64, mirror: bool) -> Result<()> {
        self.conn
            .execute("UPDATE accounts SET mirror_timer = ?1 WHERE id = ?2", params![mirror, id])?;
        Ok(())
    }

    /// Stores a changed mapping (JSON, see `fibery::Mapping`).
    pub fn set_account_mapping(&self, id: i64, mapping: &str) -> Result<()> {
        self.conn
            .execute("UPDATE accounts SET mapping = ?1 WHERE id = ?2", params![mapping, id])?;
        Ok(())
    }

    pub fn set_account_token(&self, id: i64, token: Option<&str>) -> Result<()> {
        self.conn
            .execute("UPDATE accounts SET token = ?1 WHERE id = ?2", params![token, id])?;
        Ok(())
    }

    /// Records the outcome of a sync; `last_sync_at` only moves on success.
    pub fn set_account_synced(&self, id: i64, now: i64, error: Option<&str>) -> Result<()> {
        match error {
            None => self.conn.execute(
                "UPDATE accounts SET last_sync_at = ?1, last_error = NULL WHERE id = ?2",
                params![now, id],
            )?,
            Some(error) => self
                .conn
                .execute("UPDATE accounts SET last_error = ?1 WHERE id = ?2", params![error, id])?,
        };
        Ok(())
    }

    /// Entries of connected clients changed locally and not pushed yet.
    pub fn pending_count(&self) -> Result<i64> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM entries e
             JOIN tasks t ON t.id = e.task_id
             JOIN accounts a ON a.client_id = t.client_id
             WHERE e.dirty = 1 AND NOT (e.start_at IS NOT NULL AND e.end_at IS NULL AND a.mirror_timer = 0)",
            [],
            |row| row.get(0),
        )
    }

    /// Local changes for `account` to send: new tasks first, then entries.
    pub fn pending_pushes(&self, account: &Account) -> Result<Vec<Push>> {
        let mut pushes = Vec::new();

        // Tasks created in the app after connecting are created remotely too.
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.name, p.remote_id FROM tasks t
             LEFT JOIN projects p ON p.id = t.project_id
             WHERE t.client_id = ?1 AND t.remote_id IS NULL AND t.done = 0 AND t.created_at >= ?2
             ORDER BY t.id",
        )?;
        let new_tasks: Vec<(i64, String, Option<String>)> = stmt
            .query_map(params![account.client_id, account.created_at], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<Result<_>>()?;
        let new_task_ids: HashSet<i64> = new_tasks.iter().map(|t| t.0).collect();

        let mut stmt = self.conn.prepare(
            "SELECT e.id, e.updated_at, e.remote_id, e.deleted, e.task_id, t.remote_id, e.date,
                    e.start_at, e.end_at, e.duration_secs, e.note
             FROM entries e JOIN tasks t ON t.id = e.task_id
             WHERE t.client_id = ?1 AND e.dirty = 1
             ORDER BY e.id",
        )?;
        let rows = stmt.query_map([account.client_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<i64>>(7)?,
                r.get::<_, Option<i64>>(8)?,
                r.get::<_, Option<i64>>(9)?,
                r.get::<_, String>(10)?,
            ))
        })?;

        let mut entry_pushes = Vec::new();
        for row in rows {
            let (id, updated_at, remote_id, deleted, task_id, task_remote, date, start, end, duration, note) = row?;
            if deleted {
                if let Some(remote_id) = remote_id {
                    entry_pushes.push(Push::DeleteLog { local_id: id, remote_id });
                }
                continue;
            }
            let task = match task_remote {
                Some(remote) => TaskRef::Remote(remote),
                None if new_task_ids.contains(&task_id) => TaskRef::Local(task_id),
                None => continue, // A local-only task: nothing to sync.
            };
            let running = start.is_some() && end.is_none();
            if running && (!account.mirror_timer || remote_id.is_some()) {
                continue;
            }
            let secs = match (start, end) {
                (Some(start), Some(end)) => end - start,
                _ => duration.unwrap_or(0),
            };
            let fields = LogFields { task, date, hours: hours(secs), start, end, note };
            entry_pushes.push(match remote_id {
                Some(remote_id) => Push::UpdateLog {
                    local_id: id,
                    updated_at,
                    remote_id,
                    stopped: account.mirror_timer && start.is_some(),
                    fields,
                },
                None => Push::CreateLog { local_id: id, updated_at, fields },
            });
        }

        pushes.extend(new_tasks.into_iter().map(|(local_id, name, project)| Push::CreateTask {
            local_id,
            name,
            project,
        }));
        pushes.extend(entry_pushes);
        Ok(pushes)
    }

    /// Stores remote ids and clears `dirty` for what was pushed, unless the
    /// entry changed again meanwhile.
    pub fn apply_pushes(&self, results: &[PushResult]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for result in results {
            let Ok(remote_id) = &result.result else { continue };
            match &result.push {
                Push::CreateTask { local_id, .. } => {
                    tx.execute("UPDATE tasks SET remote_id = ?1 WHERE id = ?2", params![remote_id, local_id])?;
                }
                Push::CreateLog { local_id, updated_at, .. } => {
                    tx.execute(
                        "UPDATE entries SET remote_id = ?1,
                             dirty = CASE WHEN updated_at = ?2 THEN 0 ELSE 1 END
                         WHERE id = ?3",
                        params![remote_id, updated_at, local_id],
                    )?;
                }
                Push::UpdateLog { local_id, updated_at, .. } => {
                    tx.execute(
                        "UPDATE entries SET dirty = 0 WHERE id = ?1 AND updated_at = ?2",
                        params![local_id, updated_at],
                    )?;
                }
                Push::DeleteLog { local_id, .. } => {
                    tx.execute("DELETE FROM entries WHERE id = ?1", [local_id])?;
                }
            }
        }
        tx.commit()
    }

    /// Merges pulled projects, tasks and time logs into the client of
    /// `account`. Remote changes win, except over local changes that are
    /// still waiting to be pushed while the remote log didn't change.
    pub fn apply_pull(&self, account: &Account, pulled: &Pulled, now: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let sync = Sync { store: self, client_id: account.client_id, now };

        for project in &pulled.projects {
            sync.ensure_project(project, true)?;
        }
        for task in &pulled.tasks {
            sync.ensure_task(task, true)?;
        }
        // Tasks the picker no longer offers (finished, reassigned, …).
        let offered: HashSet<&str> = pulled.tasks.iter().map(|t| t.id.as_str()).collect();
        let mut stmt = self
            .conn
            .prepare("SELECT id, remote_id FROM tasks WHERE client_id = ?1 AND remote_id IS NOT NULL AND done = 0")?;
        let stale: Vec<i64> = stmt
            .query_map([account.client_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .filter_map(|r| r.ok())
            .filter(|(_, remote)| !offered.contains(remote.as_str()))
            .map(|(id, _)| id)
            .collect();
        drop(stmt);
        for id in stale {
            self.conn.execute("UPDATE tasks SET done = 1 WHERE id = ?1", [id])?;
        }
        let mut seen = HashSet::new();
        for log in &pulled.logs {
            seen.insert(log.id.clone());
            sync.merge_log(log)?;
        }

        // Logs deleted in Fibery: synced entries in the pulled range that
        // didn't come back. Running and unpushed entries are kept.
        let mut stmt = self.conn.prepare(
            "SELECT e.id, e.remote_id FROM entries e JOIN tasks t ON t.id = e.task_id
             WHERE t.client_id = ?1 AND e.remote_id IS NOT NULL AND e.dirty = 0
               AND e.date >= ?2 AND NOT (e.start_at IS NOT NULL AND e.end_at IS NULL)",
        )?;
        let gone: Vec<i64> = stmt
            .query_map(params![account.client_id, pulled.since], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .filter(|(_, remote)| !seen.contains(remote))
            .map(|(id, _)| id)
            .collect();
        for id in gone {
            self.conn.execute("DELETE FROM entries WHERE id = ?1", [id])?;
        }
        drop(stmt);
        tx.commit()
    }
}

/// id, dirty, remote_modified, start_at, end_at
type LocalEntry = (i64, bool, Option<String>, Option<i64>, Option<i64>);

struct Sync<'a> {
    store: &'a Store,
    client_id: i64,
    now: i64,
}

impl Sync<'_> {
    /// Local id for a remote project, creating or updating it.
    /// `update_state`: whether `archived` is known (it isn't for projects
    /// that only came along with a time log).
    fn ensure_project(&self, project: &RemoteProject, update_state: bool) -> Result<i64> {
        let conn = &self.store.conn;
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM projects WHERE remote_id = ?1 AND client_id = ?2",
                params![project.id, self.client_id],
                |r| r.get(0),
            )
            .optional()?;
        match existing {
            Some(id) => {
                conn.execute("UPDATE projects SET name = ?1 WHERE id = ?2", params![project.name, id])?;
                if update_state {
                    conn.execute(
                        "UPDATE projects SET archived = ?1 WHERE id = ?2",
                        params![project.archived, id],
                    )?;
                }
                Ok(id)
            }
            None => {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM projects WHERE client_id = ?1",
                    [self.client_id],
                    |r| r.get(0),
                )?;
                let color = SYNCED_COLORS[count as usize % SYNCED_COLORS.len()];
                conn.execute(
                    "INSERT INTO projects (client_id, name, color, remote_id, archived, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![self.client_id, project.name, color, project.id, update_state && project.archived, self.now],
                )?;
                Ok(conn.last_insert_rowid())
            }
        }
    }

    /// Local id for a remote task, creating or updating it. `update_state`:
    /// whether `hidden` is known (it isn't for tasks that only came along
    /// with a time log).
    fn ensure_task(&self, task: &RemoteTask, update_state: bool) -> Result<i64> {
        let conn = &self.store.conn;
        // The project list (pulled first) owns the projects' archived state.
        let project_id = match &task.project {
            Some(project) => Some(self.ensure_project(project, false)?),
            None => None,
        };
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM tasks WHERE remote_id = ?1 AND client_id = ?2",
                params![task.id, self.client_id],
                |r| r.get(0),
            )
            .optional()?;
        match existing {
            Some(id) => {
                conn.execute(
                    "UPDATE tasks SET name = ?1, project_id = ?2 WHERE id = ?3",
                    params![task.name, project_id, id],
                )?;
                if update_state {
                    conn.execute("UPDATE tasks SET done = ?1 WHERE id = ?2", params![task.hidden, id])?;
                }
                Ok(id)
            }
            None => {
                conn.execute(
                    "INSERT INTO tasks (client_id, project_id, name, done, remote_id, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![self.client_id, project_id, task.name, task.hidden, task.id, self.now],
                )?;
                Ok(conn.last_insert_rowid())
            }
        }
    }

    fn merge_log(&self, log: &RemoteLog) -> Result<()> {
        let conn = &self.store.conn;
        let task_id = self.ensure_task(&log.task, false)?;
        let running_remotely = log.start.is_some() && log.end.is_none() && log.hours == 0.0;
        let (start, end, duration) = match (log.start, log.end) {
            (Some(start), Some(end)) => (Some(start), Some(end), None),
            (Some(start), None) if running_remotely => (Some(start), None, None),
            _ => (None, None, Some((log.hours * 3600.0).round() as i64)),
        };

        let local: Option<LocalEntry> = conn
            .query_row(
                "SELECT id, dirty, remote_modified, start_at, end_at FROM entries WHERE remote_id = ?1",
                [&log.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;

        match local {
            Some((id, dirty, modified, local_start, local_end)) => {
                let running_locally = local_start.is_some() && local_end.is_none();
                let unpushed_change = dirty && modified.as_deref() == Some(log.modified.as_str());
                if running_locally || unpushed_change || running_remotely {
                    if !unpushed_change {
                        conn.execute(
                            "UPDATE entries SET remote_modified = ?1 WHERE id = ?2",
                            params![log.modified, id],
                        )?;
                    }
                    return Ok(());
                }
                conn.execute(
                    "UPDATE entries SET task_id = ?1, date = ?2, start_at = ?3, end_at = ?4,
                         duration_secs = ?5, note = ?6, remote_modified = ?7, dirty = 0, deleted = 0,
                         updated_at = ?8
                     WHERE id = ?9",
                    params![task_id, log.date, start, end, duration, log.note, log.modified, self.now, id],
                )?;
            }
            None => {
                if running_remotely {
                    // A timer started in Fibery: show it, unless one runs here.
                    let running_here: bool = conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM entries WHERE start_at IS NOT NULL AND end_at IS NULL)",
                        [],
                        |r| r.get(0),
                    )?;
                    if running_here {
                        return Ok(());
                    }
                }
                conn.execute(
                    "INSERT INTO entries (task_id, date, start_at, end_at, duration_secs, note,
                                          remote_id, remote_modified, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                    params![task_id, log.date, start, end, duration, log.note, log.id, log.modified, self.now],
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fibery::Error;
    use crate::store::EntryInput;

    fn connected() -> (Store, Account) {
        let store = Store::open_in_memory().unwrap();
        let client = store.create_client("Acme", 0).unwrap();
        store
            .add_account(
                &NewAccount {
                    client_id: client,
                    service: "fibery",
                    workspace: "https://acme.fibery.io",
                    user_id: "me",
                    user_name: "Peter",
                    mapping: "{}",
                },
                100,
            )
            .unwrap();
        let account = store.account_for_client(client).unwrap().unwrap();
        (store, account)
    }

    fn remote_task(id: &str) -> RemoteTask {
        RemoteTask {
            id: id.into(),
            name: format!("Task {id}"),
            project: Some(RemoteProject { id: "p1".into(), name: "Website relaunch".into(), archived: false }),
            hidden: false,
        }
    }

    fn log(id: &str, task: &str, date: &str, modified: &str) -> RemoteLog {
        RemoteLog {
            id: id.into(),
            // As pulled: a log's task is only offered if it's in the task list.
            task: RemoteTask { hidden: true, ..remote_task(task) },
            date: date.into(),
            hours: 1.5,
            start: None,
            end: None,
            note: "Remote".into(),
            modified: modified.into(),
        }
    }

    fn pulled(logs: Vec<RemoteLog>) -> Pulled {
        Pulled {
            projects: vec![RemoteProject { id: "p1".into(), name: "Website relaunch".into(), archived: false }],
            tasks: vec![remote_task("t1")],
            logs,
            since: "2026-11-01".into(),
        }
    }

    #[test]
    fn connecting_marks_the_client() {
        let (store, account) = connected();
        let client = store.client(account.client_id).unwrap().unwrap();
        assert_eq!(client.source.as_deref(), Some("fibery"));
        store.remove_account(account.id).unwrap();
        assert!(store.client(account.client_id).unwrap().unwrap().source.is_none());
    }

    #[test]
    fn pulls_projects_tasks_and_logs() {
        let (store, account) = connected();
        store
            .apply_pull(&account, &pulled(vec![log("l1", "t1", "2026-11-17", "m1"), log("l2", "t2", "2026-11-18", "m1")]), 200)
            .unwrap();

        let tasks = store.open_tasks().unwrap();
        assert_eq!(tasks.len(), 1, "only the pulled task list is offered, not the task of a log");
        assert!(tasks.iter().all(|t| t.is_remote && t.project_name.as_deref() == Some("Website relaunch")));
        let entries = store.entries_between("2026-11-01", "2026-11-30").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].duration(0), 5_400);
        assert_eq!(store.pending_count().unwrap(), 0, "pulled entries are not dirty");

        // Pulling again doesn't duplicate; a log deleted remotely disappears.
        store.apply_pull(&account, &pulled(vec![log("l1", "t1", "2026-11-17", "m1")]), 300).unwrap();
        assert_eq!(store.entries_between("2026-11-01", "2026-11-30").unwrap().len(), 1);
        assert_eq!(store.open_tasks().unwrap().len(), 1);

        // A task that's no longer in the list (finished, reassigned) is hidden.
        let empty = Pulled { tasks: Vec::new(), ..pulled(vec![log("l1", "t1", "2026-11-17", "m1")]) };
        store.apply_pull(&account, &empty, 400).unwrap();
        assert!(store.open_tasks().unwrap().is_empty());
        assert_eq!(store.entries_between("2026-11-01", "2026-11-30").unwrap().len(), 1, "its time stays");
    }

    #[test]
    fn pushes_new_and_changed_entries() {
        let (store, account) = connected();
        store.apply_pull(&account, &pulled(vec![log("l1", "t1", "2026-11-17", "m1")]), 200).unwrap();
        let task = store.open_tasks().unwrap()[0].id;
        let synced = store.entries_between("2026-11-01", "2026-11-30").unwrap()[0].id;

        // New entry, edit of a synced one, a deletion, and a local-only task.
        let input = EntryInput {
            task_id: task,
            date: "2026-11-18".into(),
            start_at: Some(1_000),
            end_at: Some(4_600),
            duration_secs: None,
            note: String::new(),
        };
        let new = store.add_entry(&input, 300).unwrap();
        store.set_entry_note(synced, "Changed", 300).unwrap();
        let local_client = store.clients().unwrap().iter().find(|c| c.source.is_none()).unwrap().id;
        let local_task = store.create_task(local_client, None, "Mine", 300).unwrap();
        store.add_entry(&EntryInput { task_id: local_task, ..input.clone() }, 300).unwrap();

        let pushes = store.pending_pushes(&account).unwrap();
        assert_eq!(pushes.len(), 2, "the local-only task's entry is not pushed");
        assert!(pushes.iter().any(
            |p| matches!(p, Push::CreateLog { local_id, fields, .. } if *local_id == new && fields.hours == 1.0)
        ));
        assert!(pushes.iter().any(
            |p| matches!(p, Push::UpdateLog { remote_id, fields, .. } if remote_id == "l1" && fields.note == "Changed")
        ));

        let results: Vec<PushResult> = pushes
            .into_iter()
            .map(|push| {
                let result = match &push {
                    Push::CreateLog { .. } => Ok(Some("l9".to_string())),
                    _ => Ok(None),
                };
                PushResult { push, result }
            })
            .collect();
        store.apply_pushes(&results).unwrap();
        assert!(store.pending_pushes(&account).unwrap().is_empty());
        assert_eq!(store.entry(new).unwrap().unwrap().id, new);

        store.delete_entry(synced).unwrap();
        assert!(store.entry(synced).unwrap().is_none(), "hidden right away");
        let pushes = store.pending_pushes(&account).unwrap();
        assert!(matches!(&pushes[..], [Push::DeleteLog { remote_id, .. }] if remote_id == "l1"));
        store
            .apply_pushes(&[PushResult { push: pushes[0].clone(), result: Ok(None) }])
            .unwrap();
        assert!(store.pending_pushes(&account).unwrap().is_empty());
    }

    #[test]
    fn failed_pushes_stay_pending_and_survive_pulls() {
        let (store, account) = connected();
        store.apply_pull(&account, &pulled(vec![log("l1", "t1", "2026-11-17", "m1")]), 200).unwrap();
        let synced = store.entries_between("2026-11-01", "2026-11-30").unwrap()[0].id;
        store.set_entry_note(synced, "Mine", 300).unwrap();

        let push = store.pending_pushes(&account).unwrap().remove(0);
        store
            .apply_pushes(&[PushResult { push, result: Err(Error::Network("offline".into())) }])
            .unwrap();
        // Remote unchanged (same modified): the local edit is kept.
        store.apply_pull(&account, &pulled(vec![log("l1", "t1", "2026-11-17", "m1")]), 400).unwrap();
        assert_eq!(store.entry(synced).unwrap().unwrap().note, "Mine");
        // Remote changed too: Fibery wins.
        store.apply_pull(&account, &pulled(vec![log("l1", "t1", "2026-11-17", "m2")]), 500).unwrap();
        assert_eq!(store.entry(synced).unwrap().unwrap().note, "Remote");
    }

    #[test]
    fn mirrors_running_timer_and_creates_tasks() {
        let (store, account) = connected();
        let project = store.create_project(account.client_id, "Local project", "blue", 200).unwrap();
        let task = store.create_task(account.client_id, Some(project), "New", 200).unwrap();
        store.start_timer(task, 1_000, "2026-11-19").unwrap();

        let pushes = store.pending_pushes(&account).unwrap();
        assert!(matches!(&pushes[0], Push::CreateTask { local_id, .. } if *local_id == task));
        assert!(matches!(&pushes[1], Push::CreateLog { fields, .. } if fields.end.is_none() && fields.task == TaskRef::Local(task)));

        store.set_account_mirror(account.id, false).unwrap();
        let account = store.account_for_client(account.client_id).unwrap().unwrap();
        assert_eq!(store.pending_pushes(&account).unwrap().len(), 1, "running entry waits without mirroring");
    }
}
