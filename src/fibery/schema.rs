//! Finds the task, project and time log databases (and their fields) in a
//! Fibery workspace schema, so connecting needs no manual setup.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which Fibery databases and fields hold tasks, projects and time logs.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Mapping {
    pub task_type: String,
    pub task_name: String,
    pub task_project: Option<String>,
    pub task_state: Option<String>,
    pub task_assignees: Option<String>,
    /// Checkbox set while a Fibery timer runs (the workspace's own timer buttons).
    pub task_timing: Option<String>,

    pub project_type: Option<String>,
    pub project_name: Option<String>,
    pub project_state: Option<String>,

    pub log_type: String,
    pub log_task: String,
    pub log_user: Option<String>,
    pub log_hours: String,
    pub log_date: String,
    pub log_note: Option<String>,
    pub log_start: Option<String>,
    pub log_end: Option<String>,
}

#[derive(Debug, Clone)]
struct Field {
    name: String,
    ty: String,
    collection: bool,
    title: bool,
    /// Read-only, formula or lookup: never written.
    computed: bool,
}

#[derive(Debug, Clone)]
struct Type {
    name: String,
    domain: bool,
    fields: Vec<Field>,
}

impl Type {
    fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Writable fields of a primitive type.
    fn writable(&self, ty: &str) -> impl Iterator<Item = &Field> {
        self.fields
            .iter()
            .filter(move |f| f.ty == ty && !f.computed && !f.name.starts_with("fibery/"))
    }

    fn title(&self) -> Option<&Field> {
        self.fields.iter().find(|f| f.title).or_else(|| {
            self.fields
                .iter()
                .find(|f| f.ty == "fibery/text" && short_name(&f.name).eq_ignore_ascii_case("name"))
        })
    }
}

fn short_name(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

fn mentions(name: &str, words: &[&str]) -> bool {
    let name = short_name(name).to_lowercase();
    words.iter().any(|w| name.contains(w))
}

fn parse(schema: &Value) -> Vec<Type> {
    let types = schema
        .get("fibery/types")
        .or_else(|| schema.pointer("/result/fibery/types"))
        .and_then(Value::as_array);
    let Some(types) = types else {
        return Vec::new();
    };
    types
        .iter()
        .map(|t| {
            let meta = &t["fibery/meta"];
            let fields = t["fibery/fields"]
                .as_array()
                .map(|fields| {
                    fields
                        .iter()
                        .map(|f| {
                            let meta = &f["fibery/meta"];
                            let flag = |key: &str| meta[key].as_bool().unwrap_or(false);
                            let formula = meta
                                .as_object()
                                .is_some_and(|m| m.keys().any(|k| k.starts_with("formula/")));
                            Field {
                                name: f["fibery/name"].as_str().unwrap_or_default().to_string(),
                                ty: f["fibery/type"].as_str().unwrap_or_default().to_string(),
                                collection: flag("fibery/collection?"),
                                title: flag("ui/title?"),
                                computed: flag("fibery/readonly?") || formula,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            Type {
                name: t["fibery/name"].as_str().unwrap_or_default().to_string(),
                domain: meta["fibery/domain?"].as_bool().unwrap_or(false),
                fields,
            }
        })
        .collect()
}

/// Detects the mapping, or explains what's missing.
pub fn detect(schema: &Value) -> Result<Mapping, String> {
    let types = parse(schema);
    let domain = |name: &str| types.iter().find(|t| t.name == name && t.domain);

    // Time logs: a database with a date, a number and a relation to another
    // database (the task). Prefer names like "Time log" / "Time entry".
    let mut best: Option<(i32, &Type, &Field)> = None;
    for ty in types.iter().filter(|t| t.domain) {
        let has_date = ty.writable("fibery/date").next().is_some();
        let has_number = ty.writable("fibery/decimal").next().is_some() || ty.writable("fibery/int").next().is_some();
        // The task link: prefer a target that lists these logs back
        // (Task › "Time logs"), then one named like a task.
        let task = ty
            .fields
            .iter()
            .filter(|f| !f.collection && !f.computed && f.ty != ty.name)
            .filter_map(|f| domain(&f.ty).map(|target| (f, target)))
            .max_by_key(|(f, target)| {
                let links_back = target.fields.iter().any(|t| t.collection && t.ty == ty.name);
                (links_back, mentions(&f.name, &["task", "issue", "story", "ticket"]))
            })
            .map(|(f, _)| f);
        let (true, true, Some(task)) = (has_date, has_number, task) else {
            continue;
        };
        let mut score = 1;
        if mentions(&ty.name, &["time"]) {
            score += 10;
        }
        if mentions(&ty.name, &["log", "entry", "entries", "track"]) {
            score += 5;
        }
        if ty.fields.iter().any(|f| f.ty == "fibery/user" && !f.collection && !f.computed) {
            score += 3;
        }
        if best.is_none_or(|(s, _, _)| score > s) {
            best = Some((score, ty, task));
        }
    }
    let Some((_, log, log_task)) = best else {
        return Err("No time log database found (needs a date, a number and a link to tasks).".into());
    };

    let numbers: Vec<&Field> = log.writable("fibery/decimal").chain(log.writable("fibery/int")).collect();
    let hours = numbers
        .iter()
        .find(|f| mentions(&f.name, &["spent", "hours", "duration", "time"]))
        .or(numbers.first())
        .expect("checked above");
    let date = log.writable("fibery/date").next().expect("checked above");
    let user = log
        .fields
        .iter()
        .find(|f| f.ty == "fibery/user" && !f.collection && !f.computed && !f.name.starts_with("fibery/"));
    let texts: Vec<&Field> = log.writable("fibery/text").filter(|f| !f.title).collect();
    let note = texts
        .iter()
        .find(|f| mentions(&f.name, &["note", "comment", "description"]))
        .or(texts.first());
    let date_times: Vec<&Field> = log.writable("fibery/date-time").collect();
    let start = date_times.iter().find(|f| mentions(&f.name, &["start", "begin", "from"]));
    let end = date_times.iter().find(|f| mentions(&f.name, &["end", "stop", "until"]));

    let task = domain(&log_task.ty).expect("checked above");
    let task_name = task
        .title()
        .ok_or_else(|| format!("No name field found in {}.", task.name))?;
    let task_project = task.fields.iter().find(|f| {
        !f.collection && !f.computed && domain(&f.ty).is_some() && mentions(&f.ty, &["project"])
    });
    let project = task_project.and_then(|f| domain(&f.ty));
    let state = |ty: &Type| ty.field("workflow/state").map(|f| f.name.clone());

    Ok(Mapping {
        task_type: task.name.clone(),
        task_name: task_name.name.clone(),
        task_project: task_project.map(|f| f.name.clone()),
        task_state: state(task),
        task_assignees: task
            .fields
            .iter()
            .find(|f| f.ty == "fibery/user" && f.collection)
            .map(|f| f.name.clone()),
        task_timing: task
            .writable("fibery/bool")
            .find(|f| mentions(&f.name, &["timing", "timer"]))
            .map(|f| f.name.clone()),
        project_type: project.map(|p| p.name.clone()),
        project_name: project.and_then(Type::title).map(|f| f.name.clone()),
        project_state: project.and_then(state),
        log_type: log.name.clone(),
        log_task: log_task.name.clone(),
        log_user: user.map(|f| f.name.clone()),
        log_hours: hours.name.clone(),
        log_date: date.name.clone(),
        log_note: note.map(|f| f.name.clone()),
        log_start: start.map(|f| f.name.clone()),
        log_end: end.map(|f| f.name.clone()),
    })
}

/// Database names without the space, e.g. "Time log" for "Projects/Time log".
pub fn display_name(name: &str) -> &str {
    short_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn field(name: &str, ty: &str, meta: Value) -> Value {
        json!({"fibery/name": name, "fibery/type": ty, "fibery/meta": meta})
    }

    /// Trimmed copy of a real workspace schema (names changed).
    fn schema() -> Value {
        json!({"fibery/types": [
            {"fibery/name": "fibery/user", "fibery/meta": {"fibery/domain?": false}, "fibery/fields": [
                field("user/name", "fibery/text", json!({}))]},
            {"fibery/name": "Projects/Time log", "fibery/meta": {"fibery/domain?": true}, "fibery/fields": [
                field("fibery/id", "fibery/uuid", json!({"fibery/readonly?": true})),
                field("fibery/creation-date", "fibery/date-time", json!({"fibery/readonly?": true})),
                field("fibery/created-by", "fibery/user", json!({"fibery/readonly?": true})),
                field("Projects/Name", "fibery/text", json!({"formula/formula?": true, "ui/title?": true})),
                field("Projects/End", "fibery/date-time", json!({})),
                field("Projects/Start", "fibery/date-time", json!({})),
                field("Projects/Task", "Projects/Task", json!({"fibery/relation": "r1"})),
                field("Projects/User", "fibery/user", json!({"fibery/relation": "r2"})),
                field("Projects/Time spent", "fibery/decimal", json!({})),
                field("Projects/Date", "fibery/date", json!({})),
                field("Projects/Project", "Projects/Project", json!({"formula/lookup?": true})),
                field("Projects/Notes (if any)", "fibery/text", json!({}))]},
            {"fibery/name": "Projects/Task", "fibery/meta": {"fibery/domain?": true}, "fibery/fields": [
                field("Projects/Duration", "fibery/date-range", json!({})),
                field("Projects/Timing", "fibery/bool", json!({})),
                field("Projects/name", "fibery/text", json!({"ui/title?": true})),
                field("Projects/Project", "Projects/Project", json!({"fibery/relation": "r3"})),
                field("assignments/assignees", "fibery/user", json!({"fibery/collection?": true})),
                field("Projects/Time logs", "Projects/Time log", json!({"fibery/collection?": true})),
                field("Projects/Hours spent", "fibery/decimal", json!({"formula/formula?": true})),
                field("Projects/Deadline", "fibery/date", json!({})),
                field("workflow/state", "workflow/state_Projects/Task", json!({}))]},
            {"fibery/name": "Projects/Project", "fibery/meta": {"fibery/domain?": true}, "fibery/fields": [
                field("Projects/name", "fibery/text", json!({"ui/title?": true})),
                field("Projects/Tasks", "Projects/Task", json!({"fibery/collection?": true})),
                field("workflow/state", "workflow/state_Projects/Project", json!({}))]},
            {"fibery/name": "CRM/Interaction", "fibery/meta": {"fibery/domain?": true}, "fibery/fields": [
                field("CRM/Date", "fibery/date", json!({})),
                field("CRM/Score", "fibery/int", json!({})),
                field("CRM/Project", "Projects/Project", json!({}))]}
        ]})
    }

    #[test]
    fn detects_a_typical_setup() {
        let mapping = detect(&schema()).unwrap();
        assert_eq!(mapping, Mapping {
            task_type: "Projects/Task".into(),
            task_name: "Projects/name".into(),
            task_project: Some("Projects/Project".into()),
            task_state: Some("workflow/state".into()),
            task_assignees: Some("assignments/assignees".into()),
            task_timing: Some("Projects/Timing".into()),
            project_type: Some("Projects/Project".into()),
            project_name: Some("Projects/name".into()),
            project_state: Some("workflow/state".into()),
            log_type: "Projects/Time log".into(),
            log_task: "Projects/Task".into(),
            log_user: Some("Projects/User".into()),
            log_hours: "Projects/Time spent".into(),
            log_date: "Projects/Date".into(),
            log_note: Some("Projects/Notes (if any)".into()),
            log_start: Some("Projects/Start".into()),
            log_end: Some("Projects/End".into()),
        });
    }

    #[test]
    fn finds_the_task_link_regardless_of_field_order() {
        let mut schema = schema();
        let log = &mut schema["fibery/types"][1]["fibery/fields"];
        let fields = log.as_array_mut().unwrap();
        // An unflagged lookup before the real task relation.
        fields.insert(0, field("Projects/Project", "Projects/Project", json!({})));
        assert_eq!(detect(&schema).unwrap().log_task, "Projects/Task");
    }

    #[test]
    fn explains_missing_time_logs() {
        let schema = json!({"fibery/types": [
            {"fibery/name": "A/Task", "fibery/meta": {"fibery/domain?": true}, "fibery/fields": [
                field("A/name", "fibery/text", json!({"ui/title?": true}))]}
        ]});
        assert!(detect(&schema).unwrap_err().contains("No time log database"));
    }

    #[test]
    fn shortens_names() {
        assert_eq!(display_name("Projects/Time log"), "Time log");
    }
}
