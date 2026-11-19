//! Schedules syncs with connected services and tracks their status.
//!
//! A full sync (push + pull) runs at startup, every 5 minutes, when the
//! network comes back and on "Sync Now". Local changes are pushed a few
//! seconds after they happen.

use std::cell::RefCell;

use gettextrs::gettext;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::application::TimeTrackerApplication;
use crate::fibery::{self, Client, Error, Mapping};
use crate::format;
use crate::secrets;
use crate::store::Account;

const FULL_SYNC_INTERVAL: u32 = 5 * 60;
const PUSH_DELAY: u32 = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// No connected services.
    Off,
    Syncing,
    Synced(Option<i64>),
    /// Local changes waiting to be pushed.
    Pending(i64),
    Offline(i64),
    Error(String),
}

#[derive(Default)]
struct State {
    running: bool,
    /// Sync again when the running one finishes (`true` = full).
    again: Option<bool>,
    push_timer: Option<glib::SourceId>,
    status: Option<Status>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

pub fn status() -> Status {
    STATE.with_borrow(|s| s.status.clone()).unwrap_or(Status::Off)
}

fn set_status(status: Status) {
    let changed = STATE.with_borrow_mut(|s| s.status.replace(status.clone()) != Some(status));
    if changed {
        TimeTrackerApplication::get().emit_by_name::<()>("sync-changed", &[]);
    }
}

/// Starts automatic syncing; call once at startup.
pub fn start() {
    let app = TimeTrackerApplication::get();
    refresh_status();
    glib::timeout_add_seconds_local_once(2, || sync(true));
    glib::timeout_add_seconds_local(FULL_SYNC_INTERVAL, || {
        sync(true);
        glib::ControlFlow::Continue
    });
    app.connect_data_changed(|_| schedule_push());
    gio::NetworkMonitor::default().connect_network_available_notify(|monitor| {
        if monitor.is_network_available() {
            sync(true);
        }
    });
}

/// Status without syncing: off, pending or the last result.
pub fn refresh_status() {
    let store = TimeTrackerApplication::get().store();
    let accounts = store.accounts().unwrap_or_default();
    if accounts.is_empty() {
        set_status(Status::Off);
        return;
    }
    if STATE.with_borrow(|s| s.running) {
        return;
    }
    if let Some(error) = accounts.iter().find_map(|a| a.last_error.clone()) {
        set_status(Status::Error(error));
        return;
    }
    let pending = store.pending_count().unwrap_or(0);
    set_status(if pending > 0 {
        Status::Pending(pending)
    } else {
        Status::Synced(accounts.iter().filter_map(|a| a.last_sync_at).min())
    });
}

fn schedule_push() {
    let store = TimeTrackerApplication::get().store();
    if store.pending_count().unwrap_or(0) == 0 {
        refresh_status();
        return;
    }
    refresh_status();
    STATE.with_borrow_mut(|s| {
        if let Some(timer) = s.push_timer.take() {
            timer.remove();
        }
        s.push_timer = Some(glib::timeout_add_seconds_local_once(PUSH_DELAY, || {
            STATE.with_borrow_mut(|s| s.push_timer = None);
            sync(false);
        }));
    });
}

/// Syncs all connected accounts. `full` also pulls from the service.
pub fn sync(full: bool) {
    let busy = STATE.with_borrow_mut(|s| {
        if s.running {
            s.again = Some(s.again.unwrap_or(false) || full);
        }
        s.running
    });
    if busy {
        return;
    }
    if TimeTrackerApplication::get().store().accounts().unwrap_or_default().is_empty() {
        set_status(Status::Off);
        return;
    }
    STATE.with_borrow_mut(|s| s.running = true);
    glib::spawn_future_local(async move {
        let outcome = run(full).await;
        STATE.with_borrow_mut(|s| s.running = false);
        set_status(outcome);
        if let Some(full) = STATE.with_borrow_mut(|s| s.again.take()) {
            sync(full);
        }
    });
}

/// The account's API token: keyring first, database fallback. A token in
/// the database is moved to the keyring once that works.
pub async fn token_for(account: &Account) -> Option<String> {
    if let Some(token) = &account.token {
        match secrets::store_token(&account.workspace, &account.user_id, token).await {
            Ok(()) => {
                let _ = TimeTrackerApplication::get().store().set_account_token(account.id, None);
            }
            Err(err) => glib::g_warning!("time-tracker", "Keyring still unavailable: {err}"),
        }
        return Some(token.clone());
    }
    match secrets::load_token(&account.workspace, &account.user_id).await {
        Ok(token) => token,
        Err(err) => {
            glib::g_warning!("time-tracker", "Could not read the keyring: {err}");
            None
        }
    }
}

fn days_ago(days: i64) -> String {
    format::local_date(format::now() - days * 24 * 60 * 60)
}

async fn run(full: bool) -> Status {
    let app = TimeTrackerApplication::get();
    let store = app.store();
    let accounts = store.accounts().unwrap_or_default();
    if accounts.is_empty() {
        return Status::Off;
    }

    let mut offline = false;
    let mut error: Option<String> = None;
    let mut changed = false;
    for account in accounts {
        let pushes = match store.pending_pushes(&account) {
            Ok(pushes) => pushes,
            Err(err) => {
                error = Some(err.to_string());
                continue;
            }
        };
        if pushes.is_empty() && !full {
            continue;
        }
        if full || !pushes.is_empty() {
            set_status(Status::Syncing);
        }
        let Some(token) = token_for(&account).await else {
            let message = gettext("The Fibery API token is missing. Reconnect the client.");
            let _ = store.set_account_synced(account.id, format::now(), Some(&message));
            error = Some(message);
            continue;
        };
        let mapping: Mapping = match serde_json::from_str(&account.mapping) {
            Ok(mapping) => mapping,
            Err(err) => {
                error = Some(err.to_string());
                continue;
            }
        };
        // The first sync brings in a year of history; later ones two months.
        let since = days_ago(if account.last_sync_at.is_none() { 366 } else { 62 });
        let request = fibery::sync::Request {
            mapping,
            user_id: account.user_id.clone(),
            pushes,
            pull_since: full.then_some(since),
        };
        let client = Client::new(&account.workspace, &token);
        let Ok(response) = gio::spawn_blocking(move || fibery::sync::run(&client, request)).await else {
            error = Some(gettext("Syncing failed unexpectedly"));
            continue;
        };

        let now = format::now();
        if let Err(err) = store.apply_pushes(&response.pushed) {
            error = Some(err.to_string());
        }
        if let Some(pulled) = &response.pulled
            && let Err(err) = store.apply_pull(&account, pulled, now)
        {
            error = Some(err.to_string());
        }
        let push_error = response.pushed.iter().find_map(|r| r.result.clone().err());
        let sync_error = response.error.or(push_error);
        match &sync_error {
            Some(Error::Network(_)) => offline = true,
            Some(err) => error = Some(err.to_string()),
            None => {}
        }
        let message = sync_error.as_ref().filter(|e| !matches!(e, Error::Network(_))).map(|e| e.to_string());
        let _ = store.set_account_synced(account.id, now, message.as_deref());
        changed = true;
    }

    if changed {
        app.data_changed();
    }
    let pending = store.pending_count().unwrap_or(0);
    if offline {
        Status::Offline(pending)
    } else if let Some(error) = error {
        Status::Error(error)
    } else if pending > 0 {
        Status::Pending(pending)
    } else {
        Status::Synced(Some(format::now()))
    }
}

/// Short human-readable status, e.g. for the header bar popover.
pub fn describe(status: &Status) -> (String, String) {
    match status {
        Status::Off => (gettext("Not Connected"), String::new()),
        Status::Syncing => (gettext("Syncing…"), String::new()),
        Status::Synced(at) => (
            gettext("Synced"),
            at.map(|ts| gettext("Last synced at {}").replace("{}", &format::clock_time(ts)))
                .unwrap_or_default(),
        ),
        Status::Pending(n) => (
            gettextrs::ngettext("{} Entry Waiting to Sync", "{} Entries Waiting to Sync", *n as u32)
                .replace("{}", &n.to_string()),
            String::new(),
        ),
        Status::Offline(n) => (
            gettext("Offline"),
            if *n > 0 {
                gettextrs::ngettext(
                    "{} entry will sync when you're back online",
                    "{} entries will sync when you're back online",
                    *n as u32,
                )
                .replace("{}", &n.to_string())
            } else {
                gettext("Everything keeps working; changes sync later")
            },
        ),
        Status::Error(message) => (gettext("Sync Problem"), message.clone()),
    }
}
