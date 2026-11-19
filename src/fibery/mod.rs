//! Fibery API client. Calls block, so run them off the main thread
//! (`gio::spawn_blocking`).

pub mod schema;
pub mod sync;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub use schema::Mapping;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// Token missing, invalid or revoked.
    Unauthorized,
    /// No connection or a timeout.
    Network(String),
    /// Fibery rejected the request.
    Api(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unauthorized => write!(f, "The API token was not accepted"),
            Error::Network(msg) => write!(f, "Could not reach Fibery: {msg}"),
            Error::Api(msg) => write!(f, "Fibery: {msg}"),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Fibery allows 3 requests per second per token.
const MIN_INTERVAL: Duration = Duration::from_millis(350);
static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

fn throttle() {
    let mut last = LAST_REQUEST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(at) = *last {
        let elapsed = at.elapsed();
        if elapsed < MIN_INTERVAL {
            std::thread::sleep(MIN_INTERVAL - elapsed);
        }
    }
    *last = Some(Instant::now());
}

/// Turns "acme", "acme.fibery.io" or a full URL into
/// "https://acme.fibery.io".
pub fn normalize_workspace(input: &str) -> Option<String> {
    let input = input.trim().trim_end_matches('/');
    let host = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))
        .unwrap_or(input);
    let host = host.split('/').next()?.to_lowercase();
    if host.is_empty() || host.contains(char::is_whitespace) {
        return None;
    }
    let host = if host.contains('.') { host } else { format!("{host}.fibery.io") };
    Some(format!("https://{host}"))
}

#[derive(Clone)]
pub struct Client {
    workspace: String,
    token: String,
    agent: ureq::Agent,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("workspace", &self.workspace).finish_non_exhaustive()
    }
}

impl Client {
    /// `workspace` as returned by [`normalize_workspace`].
    pub fn new(workspace: &str, token: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .user_agent("TimeTracker (GNOME desktop app)")
            .build()
            .into();
        Self {
            workspace: workspace.to_string(),
            token: token.trim().to_string(),
            agent,
        }
    }

    /// Runs one command and returns its `result`.
    pub fn command(&self, command: &str, args: Value) -> Result<Value> {
        let url = format!("{}/api/commands", self.workspace);
        let body = json!({ "command": command, "args": args });
        for attempt in 1..=4 {
            throttle();
            let response = self
                .agent
                .post(&url)
                .header("Authorization", &format!("Token {}", self.token))
                .send_json(&body);
            let mut response = match response {
                Ok(response) => response,
                Err(err) => return Err(Error::Network(err.to_string())),
            };
            match response.status().as_u16() {
                401 | 403 => return Err(Error::Unauthorized),
                429 if attempt < 4 => {
                    std::thread::sleep(Duration::from_secs(attempt));
                    continue;
                }
                status if status >= 400 => {
                    let text = response.body_mut().read_to_string().unwrap_or_default();
                    return Err(Error::Api(format!("HTTP {status}: {}", text.chars().take(300).collect::<String>())));
                }
                _ => {}
            }
            let value: Value = response
                .body_mut()
                .with_config()
                .limit(64 * 1024 * 1024)
                .read_json()
                .map_err(|e| Error::Network(e.to_string()))?;
            // A single command answers {"success", "result"}; a batch an array.
            let value = match value {
                Value::Array(mut items) if !items.is_empty() => items.remove(0),
                other => other,
            };
            if value["success"].as_bool() == Some(false) {
                let message = value["result"]["message"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| value["result"].to_string());
                return Err(Error::Api(message));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(Error::Api("Too many requests, try again later".into()))
    }

    pub fn query(&self, query: Value, params: Value) -> Result<Vec<Value>> {
        let result = self.command("fibery.entity/query", json!({ "query": query, "params": params }))?;
        Ok(match result {
            Value::Array(rows) => rows,
            Value::Null => Vec::new(),
            other => vec![other],
        })
    }

    /// Creates an entity and returns its `fibery/id`.
    pub fn create(&self, ty: &str, entity: Value) -> Result<String> {
        let result = self.command("fibery.entity/create", json!({ "type": ty, "entity": entity }))?;
        result["fibery/id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::Api("Created entity has no id".into()))
    }

    /// `entity` must include `fibery/id`.
    pub fn update(&self, ty: &str, entity: Value) -> Result<()> {
        self.command("fibery.entity/update", json!({ "type": ty, "entity": entity }))?;
        Ok(())
    }

    pub fn delete(&self, ty: &str, id: &str) -> Result<()> {
        self.command("fibery.entity/delete", json!({ "type": ty, "entity": { "fibery/id": id } }))?;
        Ok(())
    }

    pub fn add_to_collection(&self, ty: &str, field: &str, id: &str, items: &[&str]) -> Result<()> {
        let items: Vec<Value> = items.iter().map(|i| json!({ "fibery/id": i })).collect();
        self.command(
            "fibery.entity/add-collection-items",
            json!({ "type": ty, "field": field, "entity": { "fibery/id": id }, "items": items }),
        )?;
        Ok(())
    }

    /// The token's user: id and name.
    pub fn me(&self) -> Result<(String, String)> {
        let rows = self.query(
            json!({
                "q/from": "fibery/user",
                "q/select": { "id": "fibery/id", "name": "user/name" },
                "q/where": ["=", ["fibery/id"], "$my-id"],
                "q/limit": 1
            }),
            json!({}),
        )?;
        let row = rows.first().ok_or(Error::Unauthorized)?;
        Ok((
            row["id"].as_str().unwrap_or_default().to_string(),
            row["name"].as_str().unwrap_or_default().to_string(),
        ))
    }

    pub fn schema(&self) -> Result<Value> {
        self.command("fibery.schema/query", json!({}))
    }
}

/// Checks a workspace and token, and detects the setup.
pub fn check(workspace: &str, token: &str) -> Result<(String, String, Mapping)> {
    let client = Client::new(workspace, token);
    let (user_id, user_name) = client.me()?;
    let schema = client.schema()?;
    let mapping = schema::detect(&schema).map_err(Error::Api)?;
    Ok((user_id, user_name, mapping))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_workspaces() {
        let expected = Some("https://acme.fibery.io".to_string());
        assert_eq!(normalize_workspace("acme"), expected);
        assert_eq!(normalize_workspace("acme.fibery.io"), expected);
        assert_eq!(normalize_workspace(" https://acme.fibery.io/Projects/Task/1 "), expected);
        assert_eq!(normalize_workspace("Acme.Fibery.io/"), expected);
        assert_eq!(normalize_workspace(""), None);
        assert_eq!(normalize_workspace("a b"), None);
    }
}
