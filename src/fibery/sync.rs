//! The network half of a sync: push local changes, pull projects, tasks and
//! the user's time logs. Pure data in, data out; the store applies results
//! on the main thread.

use serde_json::{Map, Value, json};

use super::{Client, Error, Mapping};

/// A task in a push: already in Fibery, or created earlier in this push.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskRef {
    Remote(String),
    Local(i64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogFields {
    pub task: TaskRef,
    pub date: String,
    /// Hours to two decimals, like Fibery's own timer.
    pub hours: f64,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Push {
    CreateTask { local_id: i64, name: String, project: Option<String> },
    CreateLog { local_id: i64, updated_at: i64, fields: LogFields },
    UpdateLog { local_id: i64, updated_at: i64, remote_id: String, fields: LogFields, stopped: bool },
    DeleteLog { local_id: i64, remote_id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PushResult {
    pub push: Push,
    /// Remote id of a created task or log.
    pub result: Result<Option<String>, Error>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteProject {
    pub id: String,
    pub name: String,
    pub finished: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteTask {
    pub id: String,
    pub name: String,
    pub project: Option<RemoteProject>,
    pub finished: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteLog {
    pub id: String,
    pub task: RemoteTask,
    pub date: String,
    pub hours: f64,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub note: String,
    pub modified: String,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub mapping: Mapping,
    pub user_id: String,
    pub pushes: Vec<Push>,
    /// Pull time logs dated on or after this day (YYYY-MM-DD); `None` skips pulling.
    pub pull_since: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Pulled {
    pub projects: Vec<RemoteProject>,
    pub tasks: Vec<RemoteTask>,
    pub logs: Vec<RemoteLog>,
    pub since: String,
}

#[derive(Debug, Clone, Default)]
pub struct Response {
    pub pushed: Vec<PushResult>,
    pub pulled: Option<Pulled>,
    /// Pull failed (pushes may still have succeeded).
    pub error: Option<Error>,
}

pub fn to_iso(ts: i64) -> String {
    gtk::glib::DateTime::from_unix_utc(ts)
        .and_then(|dt| dt.format_iso8601())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

pub fn from_iso(text: &str) -> Option<i64> {
    gtk::glib::DateTime::from_iso8601(text, None).ok().map(|dt| dt.to_unix())
}

/// Hours rounded to two decimals.
pub fn hours(secs: i64) -> f64 {
    (secs as f64 / 36.0).round() / 100.0
}

pub fn run(client: &Client, request: Request) -> Response {
    let mut response = Response::default();
    let mut created_tasks: Vec<(i64, String)> = Vec::new();

    for push in request.pushes {
        let result = push_one(client, &request.mapping, &request.user_id, &push, &created_tasks);
        if let (Push::CreateTask { local_id, .. }, Ok(Some(id))) = (&push, &result) {
            created_tasks.push((*local_id, id.clone()));
        }
        // Without a connection nothing else will work either.
        let offline = matches!(result, Err(Error::Network(_)) | Err(Error::Unauthorized));
        response.pushed.push(PushResult { push, result: result.clone() });
        if offline {
            response.error = result.err();
            return response;
        }
    }

    if let Some(since) = request.pull_since {
        match pull(client, &request.mapping, &request.user_id, &since) {
            Ok(pulled) => response.pulled = Some(pulled),
            Err(err) => response.error = Some(err),
        }
    }
    response
}

fn push_one(
    client: &Client,
    m: &Mapping,
    user_id: &str,
    push: &Push,
    created_tasks: &[(i64, String)],
) -> Result<Option<String>, Error> {
    match push {
        Push::CreateTask { name, project, .. } => {
            let mut entity = Map::new();
            entity.insert(m.task_name.clone(), json!(name));
            if let (Some(field), Some(project)) = (&m.task_project, project) {
                entity.insert(field.clone(), json!({ "fibery/id": project }));
            }
            let id = client.create(&m.task_type, Value::Object(entity))?;
            if let Some(assignees) = &m.task_assignees {
                client.add_to_collection(&m.task_type, assignees, &id, &[user_id])?;
            }
            Ok(Some(id))
        }
        Push::CreateLog { fields, .. } => {
            let mut entity = log_entity(m, fields, created_tasks)?;
            if let Some(user) = &m.log_user {
                entity.insert(user.clone(), json!({ "fibery/id": user_id }));
            }
            let id = client.create(&m.log_type, Value::Object(entity))?;
            // Mirror the workspace's own timer: tick "Timing" while running.
            if fields.end.is_none() {
                set_timing(client, m, &fields.task, created_tasks, true)?;
            }
            Ok(Some(id))
        }
        Push::UpdateLog { remote_id, fields, stopped, .. } => {
            let mut entity = log_entity(m, fields, created_tasks)?;
            entity.insert("fibery/id".into(), json!(remote_id));
            client.update(&m.log_type, Value::Object(entity))?;
            if *stopped {
                set_timing(client, m, &fields.task, created_tasks, false)?;
            }
            Ok(None)
        }
        Push::DeleteLog { remote_id, .. } => {
            client.delete(&m.log_type, remote_id)?;
            Ok(None)
        }
    }
}

fn resolve_task(task: &TaskRef, created_tasks: &[(i64, String)]) -> Result<String, Error> {
    match task {
        TaskRef::Remote(id) => Ok(id.clone()),
        TaskRef::Local(local) => created_tasks
            .iter()
            .find(|(l, _)| l == local)
            .map(|(_, id)| id.clone())
            .ok_or_else(|| Error::Api("The task isn't in Fibery yet".into())),
    }
}

fn log_entity(m: &Mapping, f: &LogFields, created_tasks: &[(i64, String)]) -> Result<Map<String, Value>, Error> {
    let task = resolve_task(&f.task, created_tasks)?;
    let mut entity = Map::new();
    entity.insert(m.log_task.clone(), json!({ "fibery/id": task }));
    entity.insert(m.log_date.clone(), json!(f.date));
    entity.insert(m.log_hours.clone(), json!(f.hours));
    if let Some(note) = &m.log_note {
        entity.insert(note.clone(), json!(f.note));
    }
    if let Some(start) = &m.log_start {
        entity.insert(start.clone(), f.start.map_or(Value::Null, |ts| json!(to_iso(ts))));
    }
    if let Some(end) = &m.log_end {
        entity.insert(end.clone(), f.end.map_or(Value::Null, |ts| json!(to_iso(ts))));
    }
    Ok(entity)
}

fn set_timing(client: &Client, m: &Mapping, task: &TaskRef, created: &[(i64, String)], on: bool) -> Result<(), Error> {
    let Some(timing) = &m.task_timing else {
        return Ok(());
    };
    let id = resolve_task(task, created)?;
    client.update(&m.task_type, json!({ "fibery/id": id, timing: on }))
}

/// Selects id, name and "finished" of a project or task.
fn entity_select(name_field: &str, state_field: Option<&str>) -> Value {
    let mut select = json!({ "id": "fibery/id", "name": name_field });
    if let Some(state) = state_field {
        select["final"] = json!([state, "workflow/Final"]);
    }
    select
}

fn project_from(row: &Value) -> Option<RemoteProject> {
    Some(RemoteProject {
        id: row["id"].as_str()?.to_string(),
        name: row["name"].as_str().unwrap_or_default().to_string(),
        finished: row["final"].as_bool().unwrap_or(false),
    })
}

fn task_from(row: &Value) -> Option<RemoteTask> {
    Some(RemoteTask {
        id: row["id"].as_str()?.to_string(),
        name: row["name"].as_str().unwrap_or_default().to_string(),
        project: row.get("project").and_then(project_from),
        finished: row["final"].as_bool().unwrap_or(false),
    })
}

fn task_select(m: &Mapping) -> Value {
    let mut select = entity_select(&m.task_name, m.task_state.as_deref());
    if let (Some(field), Some(name)) = (&m.task_project, &m.project_name) {
        let mut project = json!({ "id": [field, "fibery/id"], "name": [field, name] });
        if let Some(state) = &m.project_state {
            project["final"] = json!([field, state, "workflow/Final"]);
        }
        // Flattened: Fibery selects related primitives with paths.
        select["project_id"] = project["id"].clone();
        select["project_name"] = project["name"].clone();
        if let Some(fin) = project.get("final") {
            select["project_final"] = fin.clone();
        }
    }
    select
}

/// Moves flattened project columns into a nested "project" object.
fn nest_project(mut row: Value) -> Value {
    if let Some(id) = row.get("project_id").and_then(Value::as_str).map(str::to_string) {
        row["project"] = json!({
            "id": id,
            "name": row["project_name"].clone(),
            "final": row.get("project_final").cloned().unwrap_or(Value::Null),
        });
    }
    row
}

fn pull(client: &Client, m: &Mapping, user_id: &str, since: &str) -> Result<Pulled, Error> {
    let mut pulled = Pulled { since: since.to_string(), ..Default::default() };

    if let (Some(ty), Some(name)) = (&m.project_type, &m.project_name) {
        let rows = client.query(
            json!({
                "q/from": ty,
                "q/select": entity_select(name, m.project_state.as_deref()),
                "q/limit": 1000
            }),
            json!({}),
        )?;
        pulled.projects = rows.iter().filter_map(project_from).collect();
    }

    // Tasks assigned to the user (the task picker's list).
    let mut task_query = json!({
        "q/from": m.task_type,
        "q/select": task_select(m),
        "q/limit": 1000
    });
    if let Some(assignees) = &m.task_assignees {
        task_query["q/where"] = json!(["q/in", [assignees, "fibery/id"], "$me"]);
    }
    let rows = client.query(task_query, json!({ "$me": [user_id] }))?;
    pulled.tasks = rows.into_iter().map(nest_project).filter_map(|r| task_from(&r)).collect();

    // The user's time logs since `since`, with their tasks.
    let mut select = json!({
        "id": "fibery/id",
        "modified": "fibery/modification-date",
        "date": m.log_date,
        "hours": m.log_hours,
        "task_id": [m.log_task, "fibery/id"],
        "task_name": [m.log_task, m.task_name],
    });
    if let Some(state) = &m.task_state {
        select["task_final"] = json!([m.log_task, state, "workflow/Final"]);
    }
    if let (Some(field), Some(name)) = (&m.task_project, &m.project_name) {
        select["project_id"] = json!([m.log_task, field, "fibery/id"]);
        select["project_name"] = json!([m.log_task, field, name]);
    }
    for (key, field) in [("note", &m.log_note), ("start", &m.log_start), ("end", &m.log_end)] {
        if let Some(field) = field {
            select[key] = json!(field);
        }
    }
    let mut conditions = vec![json!([">=", [m.log_date], "$since"])];
    if let Some(user) = &m.log_user {
        conditions.push(json!(["=", [user, "fibery/id"], "$user"]));
    }
    let mut offset = 0;
    loop {
        let rows = client.query(
            json!({
                "q/from": m.log_type,
                "q/select": select,
                "q/where": std::iter::once(json!("q/and")).chain(conditions.iter().cloned()).collect::<Vec<_>>(),
                "q/order-by": [[["fibery/id"], "q/asc"]],
                "q/offset": offset,
                "q/limit": 500
            }),
            json!({ "$since": since, "$user": user_id }),
        )?;
        let count = rows.len();
        for row in rows {
            let Some(log) = log_from(&row) else { continue };
            pulled.logs.push(log);
        }
        if count < 500 {
            break;
        }
        offset += count;
    }
    Ok(pulled)
}

fn log_from(row: &Value) -> Option<RemoteLog> {
    let project = row["project_id"].as_str().map(|id| RemoteProject {
        id: id.to_string(),
        name: row["project_name"].as_str().unwrap_or_default().to_string(),
        finished: false,
    });
    Some(RemoteLog {
        id: row["id"].as_str()?.to_string(),
        task: RemoteTask {
            id: row["task_id"].as_str()?.to_string(),
            name: row["task_name"].as_str().unwrap_or_default().to_string(),
            project,
            finished: row["task_final"].as_bool().unwrap_or(false),
        },
        date: row["date"].as_str()?.to_string(),
        hours: number(&row["hours"]),
        start: row["start"].as_str().and_then(from_iso),
        end: row["end"].as_str().and_then(from_iso),
        note: row["note"].as_str().unwrap_or_default().to_string(),
        modified: row["modified"].as_str().unwrap_or_default().to_string(),
    })
}

/// Fibery returns decimals as numbers or strings.
fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_hours_like_fibery() {
        assert_eq!(hours(3_600), 1.0);
        assert_eq!(hours(12_528), 3.48);
        assert_eq!(hours(70), 0.02);
        assert_eq!(hours(0), 0.0);
    }

    #[test]
    fn converts_iso_times() {
        assert_eq!(from_iso("2026-11-19T13:32:14.570Z"), Some(1_795_095_134));
        let ts = 1_795_095_134;
        assert_eq!(from_iso(&to_iso(ts)), Some(ts));
        assert_eq!(from_iso("nope"), None);
    }

    #[test]
    fn parses_pulled_logs() {
        let row = json!({
            "id": "log-1", "modified": "2026-11-19T13:33:24.756Z", "date": "2026-11-19",
            "hours": "0.02", "task_id": "task-1", "task_name": "Briefing", "task_final": false,
            "project_id": "p-1", "project_name": "Website relaunch", "note": null,
            "start": "2026-11-19T13:32:14.570Z", "end": "2026-11-19T13:33:23.217Z"
        });
        let log = log_from(&row).unwrap();
        assert_eq!(log.hours, 0.02);
        assert_eq!(log.task.project.as_ref().unwrap().name, "Website relaunch");
        assert_eq!(log.end.unwrap() - log.start.unwrap(), 69);
        assert_eq!(log.note, "");
    }

    #[test]
    fn nests_project_columns() {
        let row = nest_project(json!({"id": "t", "name": "T", "project_id": "p", "project_name": "P", "project_final": true}));
        let task = task_from(&row).unwrap();
        assert_eq!(task.project, Some(RemoteProject { id: "p".into(), name: "P".into(), finished: true }));
    }
}
