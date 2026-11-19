//! Aggregates a month of entries into a client › project › task tree with
//! a total per week, for the Review view.

use crate::format;
use crate::store::Entry;

/// A week (Monday–Sunday) clipped to the month, as day numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Week {
    pub first: u32,
    pub last: u32,
}

impl Week {
    pub fn label(&self) -> String {
        if self.first == self.last {
            self.first.to_string()
        } else {
            format!("{}–{}", self.first, self.last)
        }
    }

    pub fn contains(&self, day: u32) -> bool {
        (self.first..=self.last).contains(&day)
    }
}

pub fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap_year(year) => 29,
        _ => 28,
    }
}

/// Day of the week, 0 = Monday … 6 = Sunday (Sakamoto's method).
pub fn weekday(year: i32, month: u32, day: u32) -> u32 {
    const OFFSETS: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if month < 3 { year - 1 } else { year };
    let sunday_based =
        (y + y / 4 - y / 100 + y / 400 + OFFSETS[month as usize - 1] + day as i32).rem_euclid(7);
    ((sunday_based + 6) % 7) as u32
}

pub fn weeks_of_month(year: i32, month: u32) -> Vec<Week> {
    let mut weeks: Vec<Week> = Vec::new();
    for day in 1..=days_in_month(year, month) {
        match weeks.last_mut() {
            Some(week) if weekday(year, month, day) != 0 => week.last = day,
            _ => weeks.push(Week { first: day, last: day }),
        }
    }
    weeks
}

/// Seconds per week plus the total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Totals {
    pub weeks: Vec<i64>,
    pub total: i64,
}

impl Totals {
    fn new(weeks: usize) -> Self {
        Self { weeks: vec![0; weeks], total: 0 }
    }

    fn add(&mut self, week: usize, secs: i64) {
        self.weeks[week] += secs;
        self.total += secs;
    }
}

#[derive(Debug, Clone)]
pub struct TaskNode {
    pub task_id: i64,
    pub name: String,
    pub totals: Totals,
}

#[derive(Debug, Clone)]
pub struct ProjectNode {
    /// `None` groups the client's tasks without a project.
    pub project_id: Option<i64>,
    pub name: Option<String>,
    pub color: Option<String>,
    pub totals: Totals,
    pub tasks: Vec<TaskNode>,
}

#[derive(Debug, Clone)]
pub struct ClientNode {
    pub client_id: i64,
    pub name: String,
    pub is_remote: bool,
    pub totals: Totals,
    pub projects: Vec<ProjectNode>,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub weeks: Vec<Week>,
    pub clients: Vec<ClientNode>,
    pub totals: Totals,
}

/// Builds the tree from entries of one month. A running entry counts up to
/// `now`. Clients, projects and tasks are sorted by name; "no project" last.
pub fn build(entries: &[Entry], weeks: &[Week], now: i64) -> Report {
    let n = weeks.len();
    let mut report = Report { weeks: weeks.to_vec(), clients: Vec::new(), totals: Totals::new(n) };

    for entry in entries {
        let day: u32 = entry.date.get(8..10).and_then(|d| d.parse().ok()).unwrap_or(0);
        let Some(week) = weeks.iter().position(|w| w.contains(day)) else {
            continue;
        };
        let secs = entry.duration(now);

        let client = match report.clients.iter().position(|c| c.client_id == entry.client_id) {
            Some(i) => &mut report.clients[i],
            None => {
                report.clients.push(ClientNode {
                    client_id: entry.client_id,
                    name: entry.client_name.clone(),
                    is_remote: entry.client_is_remote,
                    totals: Totals::new(n),
                    projects: Vec::new(),
                });
                report.clients.last_mut().expect("just pushed")
            }
        };
        let project = match client.projects.iter().position(|p| p.project_id == entry.project_id) {
            Some(i) => &mut client.projects[i],
            None => {
                client.projects.push(ProjectNode {
                    project_id: entry.project_id,
                    name: entry.project_name.clone(),
                    color: entry.color.clone(),
                    totals: Totals::new(n),
                    tasks: Vec::new(),
                });
                client.projects.last_mut().expect("just pushed")
            }
        };
        let task = match project.tasks.iter().position(|t| t.task_id == entry.task_id) {
            Some(i) => &mut project.tasks[i],
            None => {
                project.tasks.push(TaskNode {
                    task_id: entry.task_id,
                    name: entry.task_name.clone(),
                    totals: Totals::new(n),
                });
                project.tasks.last_mut().expect("just pushed")
            }
        };

        task.totals.add(week, secs);
        project.totals.add(week, secs);
        client.totals.add(week, secs);
        report.totals.add(week, secs);
    }

    let key = |s: &str| s.to_lowercase();
    report.clients.sort_by_key(|c| key(&c.name));
    for client in &mut report.clients {
        client
            .projects
            .sort_by_key(|p| (p.name.is_none(), key(p.name.as_deref().unwrap_or_default())));
        for project in &mut client.projects {
            project.tasks.sort_by_key(|t| key(&t.name));
        }
    }
    report
}

/// Entries as CSV, one row per entry, for spreadsheets and invoicing.
/// Hours are decimal with two digits, like Fibery's "Time spent".
pub fn csv(entries: &[Entry], now: i64) -> String {
    let mut out = String::from("Date,Client,Project,Task,Start,End,Hours,Note\n");
    for entry in entries {
        let time = |ts: Option<i64>| ts.map(format::clock_time).unwrap_or_default();
        let hours = format!("{:.2}", entry.duration(now) as f64 / 3600.0);
        let fields = [
            entry.date.as_str(),
            &entry.client_name,
            entry.project_name.as_deref().unwrap_or_default(),
            &entry.task_name,
            &time(entry.start_at),
            &time(entry.end_at),
            &hours,
            &entry.note,
        ];
        let row: Vec<String> = fields.iter().map(|f| csv_field(f)).collect();
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_weekdays_and_month_lengths() {
        assert_eq!(weekday(2026, 11, 19), 3); // Thursday
        assert_eq!(weekday(2026, 11, 1), 6); // Sunday
        assert_eq!(weekday(2024, 2, 29), 3); // Thursday
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2026, 11), 30);
    }

    #[test]
    fn splits_month_into_clipped_weeks() {
        let weeks = weeks_of_month(2026, 11);
        let labels: Vec<_> = weeks.iter().map(Week::label).collect();
        assert_eq!(labels, ["1", "2–8", "9–15", "16–22", "23–29", "30"]);

        let june = weeks_of_month(2026, 6); // starts on a Monday
        assert_eq!(june.first(), Some(&Week { first: 1, last: 7 }));
    }

    fn entry(task: i64, project: Option<i64>, client: i64, date: &str, secs: i64) -> Entry {
        Entry {
            id: 0,
            task_id: task,
            task_name: format!("Task {task}"),
            project_name: project.map(|p| format!("Project {p}")),
            client_name: format!("Client {client}"),
            color: None,
            is_remote: false,
            date: date.into(),
            start_at: None,
            end_at: None,
            duration_secs: Some(secs),
            note: String::new(),
            client_id: client,
            client_is_remote: false,
            project_id: project,
        }
    }

    #[test]
    fn exports_csv() {
        let mut first = entry(1, Some(10), 100, "2026-11-02", 5_400);
        first.note = "Call, \"quick\"".into();
        let second = entry(2, None, 100, "2026-11-03", 1_234);
        let csv = csv(&[first, second], 0);
        let lines: Vec<_> = csv.lines().collect();
        assert_eq!(lines[0], "Date,Client,Project,Task,Start,End,Hours,Note");
        assert_eq!(lines[1], r#"2026-11-02,Client 100,Project 10,Task 1,,,1.50,"Call, ""quick""""#);
        assert_eq!(lines[2], "2026-11-03,Client 100,,Task 2,,,0.34,");
    }

    #[test]
    fn builds_tree_with_week_totals() {
        let weeks = weeks_of_month(2026, 11);
        let entries = [
            entry(1, Some(10), 100, "2026-11-02", 3_600),
            entry(1, Some(10), 100, "2026-11-03", 1_800),
            entry(2, Some(10), 100, "2026-11-17", 600),
            entry(3, None, 100, "2026-11-17", 60),
            entry(4, Some(20), 200, "2026-11-30", 120),
        ];
        let report = build(&entries, &weeks, 0);

        assert_eq!(report.totals.total, 6_180);
        assert_eq!(report.totals.weeks, [0, 5_400, 0, 660, 0, 120]);

        let client = &report.clients[0];
        assert_eq!((client.client_id, client.totals.total), (100, 6_060));
        assert_eq!(client.projects.len(), 2);
        assert_eq!(client.projects[0].project_id, Some(10));
        assert_eq!(client.projects[1].project_id, None, "no-project group comes last");
        assert_eq!(client.projects[0].tasks[0].totals.weeks[1], 5_400);
        assert_eq!(report.clients[1].totals.weeks[5], 120);
    }
}
