//! A badge on the app's dock icon with the running timer's elapsed
//! minutes. elementary OS's dock shows 0 as "!", so the first minute shows
//! that. Uses the Unity
//! LauncherEntry D-Bus API, which elementary OS's dock (and others) show;
//! a Flatpak app needs no extra permission to send it.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::application::TimeTrackerApplication;
use crate::config::APP_ID;
use crate::format;

const INTERFACE: &str = "com.canonical.Unity.LauncherEntry";

thread_local! {
    /// The badge as last sent: `None` when hidden.
    static SHOWN: Cell<Option<i64>> = const { Cell::new(None) };
    /// The update when the badge's number changes next.
    static NEXT: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
}

/// Keeps the badge up to date; call once at startup.
pub fn start() {
    let app = TimeTrackerApplication::get();
    update();
    app.connect_data_changed(|_| update());
    // Timers pause during suspend: catch up afterwards.
    glib::timeout_add_seconds_local(60, || {
        update();
        glib::ControlFlow::Continue
    });
    // The timer keeps running when the app quits, but nothing updates the
    // badge then.
    app.connect_shutdown(|_| send(None));
}

/// The badge for `elapsed` seconds: whole minutes.
fn badge(elapsed: i64) -> i64 {
    elapsed.max(0) / 60
}

/// Seconds from `elapsed` until the badge for it changes: the next minute.
fn until_change(elapsed: i64) -> i64 {
    60 - elapsed.max(0) % 60
}

fn update() {
    if let Some(source) = NEXT.take() {
        source.remove();
    }
    let running = TimeTrackerApplication::get().store().running_entry().ok().flatten();
    let elapsed = running.and_then(|e| e.start_at).map(|start| format::now() - start);
    let count = elapsed.map(badge);
    if SHOWN.get() != count {
        send(count);
    }
    // Wake up when the number changes; the timer's start is in whole
    // seconds, so a few milliseconds late is on time.
    if let Some(elapsed) = elapsed {
        let delay = std::time::Duration::from_millis(until_change(elapsed) as u64 * 1000 + 50);
        let source = glib::timeout_add_local_once(delay, || {
            NEXT.set(None);
            update();
        });
        NEXT.set(Some(source));
    }
}

fn send(count: Option<i64>) {
    SHOWN.set(count);
    let Some(connection) = TimeTrackerApplication::get().upcast_ref::<gio::Application>().dbus_connection() else {
        return;
    };
    let props = glib::VariantDict::new(None);
    props.insert_value("count", &count.unwrap_or(0).to_variant());
    props.insert_value("count-visible", &count.is_some().to_variant());
    // (sa{sv}): the dictionary must not be wrapped in another variant.
    let app_uri = format!("application://{APP_ID}.desktop").to_variant();
    let args = glib::Variant::tuple_from_iter([app_uri, props.end()]);
    if let Err(err) = connection.emit_signal(None, &object_path(), INTERFACE, "Update", Some(&args)) {
        glib::g_warning!("time-tracker", "Could not update the dock badge: {err}");
    }
    // Send it now: the app may be about to quit.
    let _ = connection.flush_sync(gio::Cancellable::NONE);
}

/// A per-app object path, as the API expects a number at the end.
fn object_path() -> String {
    let hash = APP_ID.bytes().fold(2_166_136_261_u32, |h, b| (h ^ u32::from(b)).wrapping_mul(16_777_619));
    format!("/com/canonical/unity/launcherentry/{hash}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_has_the_expected_format() {
        let props = glib::VariantDict::new(None);
        props.insert_value("count", &5_i64.to_variant());
        let args = glib::Variant::tuple_from_iter(["application://x.desktop".to_variant(), props.end()]);
        assert_eq!(args.type_().as_str(), "(sa{sv})");
    }

    #[test]
    fn shows_whole_minutes() {
        assert_eq!(badge(0), 0);
        assert_eq!(badge(59), 0);
        assert_eq!(badge(42 * 60 + 5), 42);
        assert_eq!(badge(2 * 3600 + 59 * 60), 179);
    }

    #[test]
    fn wakes_up_when_the_number_changes() {
        assert_eq!(until_change(0), 60);
        assert_eq!(until_change(4 * 60 + 20), 40);
        assert_eq!(until_change(59 * 60 + 59), 1);
        assert_eq!(until_change(90 * 60), 60);
    }

    #[test]
    fn object_path_ends_in_a_number() {
        let path = object_path();
        let last = path.rsplit('/').next().unwrap();
        assert!(last.chars().all(|c| c.is_ascii_digit()));
    }
}
