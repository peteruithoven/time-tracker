//! Store side of syncing with a connected service: accounts, the changes to
//! push, and applying pushed and pulled data.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, Row, params};

use super::{Account, Change, EntryView, PullImpact, Result, Store};
use crate::fibery::sync::{LogFields, Push, PushResult, Pulled, RemoteLog, RemoteProject, RemoteTask, TaskRef, hours};

const ACCOUNT_SELECT: &str = "
    SELECT id, client_id, service, workspace, user_id, user_name, mapping, mirror_timer,
           token, last_sync_at, last_error, created_at, held_pull, approved_pull
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
        held_pull: row.get(12)?,
        approved_pull: row.get(13)?,
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

    /// Holds a pull for review, or releases it (`None`).
    pub fn set_account_held(&self, id: i64, impact: Option<&PullImpact>) -> Result<()> {
        let json = impact.map(|i| serde_json::to_string(i).unwrap_or_default());
        self.conn.execute("UPDATE accounts SET held_pull = ?1 WHERE id = ?2", params![json, id])?;
        Ok(())
    }

    /// Approves the held pull: it's applied on the next sync, unless that
    /// would change more. Syncing resumes.
    pub fn approve_held_pull(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET approved_pull = held_pull, held_pull = NULL WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    /// Forgets an approval once a pull was applied.
    pub fn clear_approved_pull(&self, id: i64) -> Result<()> {
        self.conn.execute("UPDATE accounts SET approved_pull = NULL WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Entries of connected clients changed locally and not pushed yet.
    pub fn pending_count(&self) -> Result<i64> {
        self.conn.query_row(&format!("SELECT COUNT(*) {PENDING_FROM}"), [], |row| row.get(0))
    }

    /// The entries `pending_count` counts, newest first (deleted ones too).
    pub fn pending_entries(&self) -> Result<Vec<PendingEntry>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT e.id, t.name, e.date, e.start_at, e.end_at, e.duration_secs, e.deleted {PENDING_FROM}
             ORDER BY COALESCE(e.start_at, e.created_at) DESC"
        ))?;
        stmt.query_map([], |r| {
            Ok(PendingEntry {
                id: r.get(0)?,
                task_name: r.get(1)?,
                date: r.get(2)?,
                start_at: r.get(3)?,
                end_at: r.get(4)?,
                duration_secs: r.get(5)?,
                deleted: r.get(6)?,
            })
        })?
        .collect()
    }

    /// Ids of the entries waiting to sync, to mark them in lists.
    pub fn pending_entry_ids(&self) -> Result<HashSet<i64>> {
        let mut stmt = self.conn.prepare(&format!("SELECT e.id {PENDING_FROM}"))?;
        stmt.query_map([], |r| r.get(0))?.collect()
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
        self.apply_pull_in(account, pulled, now)?;
        tx.commit()
    }

    /// Applies a pull in a transaction and works out what it changes; keeps
    /// it only when `keep` says so (otherwise rolls back). Returns the impact
    /// and whether it was kept.
    pub fn apply_pull_checked(
        &self,
        account: &Account,
        pulled: &Pulled,
        now: i64,
        keep: impl FnOnce(&PullImpact) -> bool,
    ) -> Result<(PullImpact, bool)> {
        let tx = self.conn.unchecked_transaction()?;
        let (before, synced_before) = self.entry_views(account.client_id, &pulled.since, &pulled_ids(pulled), &[])?;
        self.apply_pull_in(account, pulled, now)?;
        let before_ids: Vec<i64> = before.keys().copied().collect();
        let (after, _) = self.entry_views(account.client_id, &pulled.since, &[], &before_ids)?;

        let mut impact = PullImpact {
            client: self.client(account.client_id)?.map(|c| c.name).unwrap_or_default(),
            synced_before,
            ..Default::default()
        };
        for (id, old) in &before {
            match after.get(id) {
                None => impact.removed.push(old.clone()),
                Some(new) if !old.same_as(new) => impact.changed.push(Change { before: old.clone(), after: new.clone() }),
                Some(_) => {}
            }
        }
        impact.added = after.iter().filter(|(id, _)| !before.contains_key(id)).map(|(_, e)| e.clone()).collect();
        let impact = impact.sorted();

        let kept = keep(&impact);
        if kept {
            tx.commit()?;
        }
        Ok((impact, kept))
    }

    /// The entries of a client a pull can touch: those from `since`, plus
    /// those of the pulled `remote_ids` and the `ids` outside that range.
    /// Also counts the synced ones a pull could remove.
    fn entry_views(
        &self,
        client_id: i64,
        since: &str,
        remote_ids: &[&str],
        ids: &[i64],
    ) -> Result<(std::collections::HashMap<i64, EntryView>, usize)> {
        const SELECT: &str = "SELECT e.id, e.task_id, t.name, t.project_id, p.name, e.date, e.start_at, e.end_at,
                    e.duration_secs, e.remote_id IS NOT NULL AND e.dirty = 0
             FROM entries e JOIN tasks t ON t.id = e.task_id LEFT JOIN projects p ON p.id = t.project_id
             WHERE e.deleted = 0 AND t.client_id = ?1";
        let view = |r: &Row| -> Result<(i64, EntryView, bool)> {
            let (start_at, end_at, duration): (Option<i64>, Option<i64>, Option<i64>) = (r.get(6)?, r.get(7)?, r.get(8)?);
            let secs = match (start_at, end_at) {
                (Some(start), Some(end)) => end - start,
                (Some(_), None) => 0,
                _ => duration.unwrap_or_default(),
            };
            let entry = EntryView {
                task_id: r.get(1)?,
                task: r.get(2)?,
                project_id: r.get(3)?,
                project: r.get(4)?,
                date: r.get(5)?,
                start_at,
                end_at,
                secs,
            };
            let removable = r.get::<_, bool>(9)? && !(start_at.is_some() && end_at.is_none());
            Ok((r.get(0)?, entry, removable))
        };
        let mut views = std::collections::HashMap::new();
        let mut synced = 0;
        let mut stmt = self.conn.prepare_cached(&format!("{SELECT} AND e.date >= ?2"))?;
        for row in stmt.query_map(params![client_id, since], view)? {
            let (id, entry, removable) = row?;
            synced += usize::from(removable);
            views.insert(id, entry);
        }
        let mut by_remote = self.conn.prepare_cached(&format!("{SELECT} AND e.remote_id = ?2"))?;
        for remote_id in remote_ids {
            for row in by_remote.query_map(params![client_id, remote_id], view)? {
                let (id, entry, _) = row?;
                views.entry(id).or_insert(entry);
            }
        }
        let mut by_id = self.conn.prepare_cached(&format!("{SELECT} AND e.id = ?2"))?;
        for id in ids {
            if views.contains_key(id) {
                continue;
            }
            for row in by_id.query_map(params![client_id, id], view)? {
                let (id, entry, _) = row?;
                views.insert(id, entry);
            }
        }
        Ok((views, synced))
    }

    fn apply_pull_in(&self, account: &Account, pulled: &Pulled, now: i64) -> Result<()> {
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
        let mut seen: HashSet<String> = pulled.unreadable.iter().cloned().collect();
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
        Ok(())
    }
}

/// The ids of the pulled logs.
fn pulled_ids(pulled: &Pulled) -> Vec<&str> {
    pulled.logs.iter().map(|l| l.id.as_str()).collect()
}

/// Entries waiting to be pushed: changed locally, of a connected client.
/// A running timer only counts when it's mirrored.
const PENDING_FROM: &str = "FROM entries e
     JOIN tasks t ON t.id = e.task_id
     JOIN accounts a ON a.client_id = t.client_id
     WHERE e.dirty = 1 AND NOT (e.start_at IS NOT NULL AND e.end_at IS NULL AND a.mirror_timer = 0)";

/// An entry waiting to sync, as listed in the sync popover.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingEntry {
    pub id: i64,
    pub task_name: String,
    pub date: String,
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
    pub duration_secs: Option<i64>,
    /// Deleted locally, to be deleted in Fibery.
    pub deleted: bool,
}

/// id, dirty, remote_modified, start_at, end_at, duration_secs
type LocalEntry = (i64, bool, Option<String>, Option<i64>, Option<i64>, Option<i64>);

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
        let running_remotely = log.start.is_some() && log.end.is_none() && log.hours.unwrap_or(0.0) == 0.0;
        let local: Option<LocalEntry> = conn
            .query_row(
                "SELECT id, dirty, remote_modified, start_at, end_at, duration_secs FROM entries WHERE remote_id = ?1",
                [&log.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()?;
        // Hours that read as empty (e.g. a field that changed) keep the
        // duration known here rather than becoming 0.
        let known_secs = local.as_ref().map(|&(_, _, _, start, end, duration)| match (start, end) {
            (Some(start), Some(end)) => end - start,
            _ => duration.unwrap_or_default(),
        });
        let (start, end, duration) = match (log.start, log.end) {
            (Some(start), Some(end)) => (Some(start), Some(end), None),
            (Some(start), None) if running_remotely => (Some(start), None, None),
            _ => {
                let secs = match log.hours {
                    Some(hours) => (hours * 3600.0).round() as i64,
                    None => known_secs.unwrap_or_default(),
                };
                (None, None, Some(secs))
            }
        };

        match local {
            Some((id, dirty, modified, local_start, local_end, _)) => {
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
            hours: Some(1.5),
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
            unreadable: Vec::new(),
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
        let waiting = store.pending_entries().unwrap();
        assert_eq!(waiting.len() as i64, store.pending_count().unwrap());
        assert_eq!(store.pending_entry_ids().unwrap(), HashSet::from([new, synced]));
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

    fn eight_logs() -> Vec<RemoteLog> {
        (1..=8).map(|i| log(&format!("l{i}"), "t1", &format!("2026-11-{:02}", 10 + i), "m1")).collect()
    }

    #[test]
    fn measures_what_a_pull_changes_and_rolls_back() {
        let (store, account) = connected();
        store.apply_pull(&account, &pulled(eight_logs()), 200).unwrap();
        let account = store.account_for_client(account.client_id).unwrap().unwrap();

        let mut logs = eight_logs();
        logs.remove(0); // l1 deleted in Fibery
        logs[0].task = RemoteTask { hidden: true, ..remote_task("t2") }; // l2 on another task
        logs[1].hours = Some(0.0); // l3's hours now read as 0
        logs[2].note = "Only the note changed".into(); // l4: not a change
        logs.push(log("l9", "t1", "2026-11-20", "m1")); // added
        let next = pulled(logs);

        let (impact, kept) = store.apply_pull_checked(&account, &next, 300, |_| false).unwrap();
        assert!(!kept);
        assert_eq!(impact.client, "Acme");
        assert_eq!(impact.synced_before, 8);
        assert_eq!(impact.removed.len(), 1);
        assert_eq!(impact.removed[0].date, "2026-11-11");
        assert_eq!(impact.changed.len(), 2);
        let zeroed = impact.changed.iter().find(|c| c.after.secs == 0).unwrap();
        assert_eq!(zeroed.before.secs, 5_400);
        assert!(impact.changed.iter().any(|c| c.before.task_id != c.after.task_id));
        assert_eq!(impact.added.len(), 1);
        assert_eq!(impact.lost_secs(), 2 * 5_400);
        assert!(!impact.should_hold(), "3 hours, 3 entries: below the thresholds");

        // Rolled back: nothing changed.
        let entries = store.entries_between("2026-11-01", "2026-11-30").unwrap();
        assert_eq!(entries.len(), 8);
        assert!(entries.iter().all(|e| e.duration(0) == 5_400));

        let (_, kept) = store.apply_pull_checked(&account, &next, 300, |_| true).unwrap();
        assert!(kept);
        assert_eq!(store.entries_between("2026-11-01", "2026-11-30").unwrap().len(), 8, "one gone, one added");
    }

    #[test]
    fn an_empty_pull_means_all_entries_missing() {
        let (store, account) = connected();
        store.apply_pull(&account, &pulled(eight_logs()), 200).unwrap();
        let account = store.account_for_client(account.client_id).unwrap().unwrap();
        let (impact, kept) = store.apply_pull_checked(&account, &pulled(Vec::new()), 300, |i| !i.should_hold()).unwrap();
        assert!(!kept);
        assert!(impact.all_missing());
        assert_eq!(impact.removed.len(), 8);
        assert_eq!(store.entries_between("2026-11-01", "2026-11-30").unwrap().len(), 8);
    }

    #[test]
    fn a_task_moved_to_another_project_changes_its_entries() {
        let (store, account) = connected();
        store.apply_pull(&account, &pulled(eight_logs()), 200).unwrap();
        let account = store.account_for_client(account.client_id).unwrap().unwrap();
        let other = RemoteProject { id: "p2".into(), name: "Mobile app".into(), archived: false };
        let mut next = pulled(eight_logs());
        next.tasks[0].project = Some(other.clone());
        for log in &mut next.logs {
            log.task.project = Some(other.clone());
        }
        let (impact, _) = store.apply_pull_checked(&account, &next, 300, |_| false).unwrap();
        assert_eq!(impact.changed.len(), 8);
        assert!(impact.changed.iter().all(|c| c.after.project.as_deref() == Some("Mobile app")));
        assert!(impact.should_hold());
    }

    #[test]
    fn holds_and_approves_a_pull() {
        let (store, account) = connected();
        let impact = PullImpact { client: "Acme".into(), synced_before: 3, ..Default::default() };
        store.set_account_held(account.id, Some(&impact)).unwrap();
        let held = store.account_for_client(account.client_id).unwrap().unwrap();
        assert_eq!(held.held(), Some(impact.clone()));
        store.approve_held_pull(account.id).unwrap();
        let approved = store.account_for_client(account.client_id).unwrap().unwrap();
        assert_eq!(approved.held(), None);
        assert_eq!(approved.approved(), Some(impact));
        store.clear_approved_pull(account.id).unwrap();
        assert_eq!(store.account_for_client(account.client_id).unwrap().unwrap().approved(), None);
    }

    #[test]
    fn unreadable_logs_are_not_deleted_and_empty_hours_keep_the_duration() {
        let (store, account) = connected();
        store.apply_pull(&account, &pulled(eight_logs()), 200).unwrap();
        let mut logs = eight_logs();
        logs.remove(0); // l1 came back without a task: unreadable
        logs[0].hours = None; // l2's hours field reads as empty
        let next = Pulled { unreadable: vec!["l1".into()], ..pulled(logs) };
        let (impact, _) = store.apply_pull_checked(&account, &next, 300, |_| true).unwrap();
        assert!(impact.is_empty(), "{impact:?}");
        let entries = store.entries_between("2026-11-01", "2026-11-30").unwrap();
        assert_eq!(entries.len(), 8);
        assert!(entries.iter().all(|e| e.duration(0) == 5_400));
    }

    /// Times applying a pull the old way and with the impact check, on a
    /// database file with 2,000 synced and 8,000 local entries.
    #[test]
    #[ignore = "timing; run with: build-aux/cargo-sdk.sh test --release -- --ignored --nocapture"]
    fn times_pull_checks_on_a_large_database() {
        use std::time::Instant;
        let path = std::env::temp_dir().join(format!("time-tracker-timing-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path, "Personal").unwrap();
        let client = store.create_client("Acme", 0).unwrap();
        let new = NewAccount { client_id: client, service: "fibery", workspace: "https://acme.fibery.io", user_id: "me", user_name: "Demo", mapping: "{}" };
        store.add_account(&new, 0).unwrap();
        let account = store.account_for_client(client).unwrap().unwrap();
        let local_client = store.clients().unwrap().iter().find(|c| c.source.is_none()).unwrap().id;
        let local_task = store.create_task(local_client, None, "Mine", 0).unwrap();
        store
            .batch(|| {
                for i in 0..8_000 {
                    let date = format!("20{:02}-{:02}-{:02}", 21 + i / 2_000, 1 + (i / 160) % 12, 1 + (i / 8) % 28);
                    let input = EntryInput { task_id: local_task, date, start_at: None, end_at: None, duration_secs: Some(3_600), note: String::new() };
                    store.add_entry(&input, 0)?;
                }
                Ok(())
            })
            .unwrap();
        // A year of logs, about 5 a day; the last two months is what later syncs pull.
        let start = gtk::glib::DateTime::from_utc(2025, 11, 20, 12, 0, 0.0).unwrap();
        let date = |i: usize| start.add_days((i * 365 / 2_000) as i32).unwrap().format("%Y-%m-%d").unwrap().to_string();
        let logs: Vec<RemoteLog> = (0..2_000).map(|i| log(&format!("l{i}"), "t1", &date(i), "m1")).collect();
        let year = Pulled { since: "2025-01-01".into(), ..pulled(logs.clone()) };
        let started = Instant::now();
        store.apply_pull(&account, &year, 100).unwrap();
        println!("first sync, {} logs: {} ms", year.logs.len(), started.elapsed().as_millis());

        let recent: Vec<RemoteLog> = logs.into_iter().filter(|l| l.date.as_str() >= "2026-09-20").collect();
        let next = Pulled { since: "2026-09-20".into(), ..pulled(recent) };
        for round in 0..3 {
            let started = Instant::now();
            store.apply_pull(&account, &next, 200).unwrap();
            let plain = started.elapsed();
            let started = Instant::now();
            let (impact, _) = store.apply_pull_checked(&account, &next, 200, |_| true).unwrap();
            let checked = started.elapsed();
            let started = Instant::now();
            store.apply_pull_checked(&account, &next, 200, |_| false).unwrap();
            let rolled_back = started.elapsed();
            println!(
                "round {round}, {} logs, {} entries in range: plain {} ms, checked {} ms, checked and rolled back {} ms",
                next.logs.len(),
                impact.synced_before,
                plain.as_millis(),
                checked.as_millis(),
                rolled_back.as_millis()
            );
        }
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
