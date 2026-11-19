//! Client-specific invoice scripts. Each script is a module here, listed in
//! [`SCRIPTS`], and can be enabled per client in Clients & Projects.
//!
//! Scripts keep account details (IDs, rates) out of the code: rates come
//! from the client and project settings, other details from the script's
//! settings, and the API token from the keyring.

mod avec_sans_moneybird;

use std::collections::BTreeMap;

/// Values for a script's settings, by key.
pub type Settings = BTreeMap<String, String>;

/// A setting the script asks for when it's enabled.
#[derive(Debug)]
pub struct SettingField {
    pub key: &'static str,
    pub title: &'static str,
}

/// An invoice script as shown in the client settings.
#[derive(Debug)]
pub struct Script {
    pub id: &'static str,
    pub name: &'static str,
    /// Explains where to get the API token and what the settings mean.
    pub help: &'static str,
    pub settings: &'static [SettingField],
    /// Text for each line in the preview, e.g. "5.43 days × € 320".
    pub describe_line: fn(&ProjectHours) -> String,
    /// Checks the token and settings (blocking).
    pub check: fn(token: &str, settings: &Settings) -> Result<(), String>,
    /// Creates the invoice (blocking) and returns the URL to open.
    pub create: fn(token: &str, settings: &Settings, request: &Request) -> Result<String, String>,
}

/// Hours on one project in the invoiced month.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectHours {
    /// `None` for tasks without a project.
    pub project: Option<String>,
    pub hours: f64,
    /// The project's rate, or else the client's.
    pub hourly_rate_cents: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub year: i32,
    pub month: u32,
    pub days_in_month: u32,
    pub projects: Vec<ProjectHours>,
}

pub const SCRIPTS: &[Script] = &[avec_sans_moneybird::SCRIPT];

pub fn find(id: &str) -> Option<&'static Script> {
    SCRIPTS.iter().find(|s| s.id == id)
}

/// Settings as stored in the database (JSON).
pub fn parse_settings(json: Option<&str>) -> Settings {
    json.and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default()
}
