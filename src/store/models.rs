#[derive(Debug, Clone, PartialEq)]
pub struct Client {
    pub id: i64,
    pub name: String,
    pub billable: bool,
    pub hourly_rate_cents: Option<i64>,
    /// Connected service, e.g. `"fibery"`. `None` for local clients.
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub client_id: i64,
    pub client_name: String,
    pub is_remote: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: i64,
    pub name: String,
    pub client_id: i64,
    pub client_name: String,
    pub client_is_remote: bool,
    pub project_id: Option<i64>,
    pub project_name: Option<String>,
    /// Project color; `None` for tasks without a project.
    pub color: Option<String>,
    pub is_remote: bool,
    pub last_used_at: Option<i64>,
}

impl Task {
    /// "Project · Client", or just the client for tasks without a project.
    pub fn context(&self) -> String {
        match &self.project_name {
            Some(project) => format!("{project} · {}", self.client_name),
            None => self.client_name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: i64,
    pub task_id: i64,
    pub task_name: String,
    pub project_name: Option<String>,
    pub client_name: String,
    pub color: Option<String>,
    pub is_remote: bool,
    pub date: String,
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
    pub duration_secs: Option<i64>,
    pub note: String,
}

impl Entry {
    pub fn is_running(&self) -> bool {
        self.start_at.is_some() && self.end_at.is_none()
    }

    /// Length in seconds; a running entry counts up to `now`.
    pub fn duration(&self, now: i64) -> i64 {
        match (self.start_at, self.end_at, self.duration_secs) {
            (Some(start), Some(end), _) => (end - start).max(0),
            (Some(start), None, _) => (now - start).max(0),
            (None, _, Some(secs)) => secs.max(0),
            (None, _, None) => 0,
        }
    }
}
