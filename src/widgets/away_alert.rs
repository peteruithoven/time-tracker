//! Asks what to do with time you were away while the timer ran.

use std::cell::Cell;

use adw::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::{gio, glib};

use crate::application::TimeTrackerApplication;
use crate::format;
use crate::idle::Away;
use crate::widgets::EntryDialog;

const NOTIFICATION_ID: &str = "away";

thread_local! {
    /// An alert is open; later away periods are left to it.
    static OPEN: Cell<bool> = const { Cell::new(false) };
}

/// Shows the alert on `window`, plus a notification in case the window is
/// in the background.
pub fn present(window: &gtk::Window, away: Away) {
    if OPEN.get() {
        return;
    }
    let app = TimeTrackerApplication::get();
    let Some(entry) = app.store().entry(away.entry_id).ok().flatten() else {
        return;
    };
    OPEN.set(true);

    let title = gettext("Away for {}").replace("{}", &duration_words(away.back - away.since));
    let since = moment(away.since);
    let body = gettext("No activity since {time} while tracking “{task}”. What should happen with that time?")
        .replace("{time}", &since)
        .replace("{task}", &entry.task_name);

    let alert = adw::AlertDialog::new(Some(&title), Some(&body));
    // Stacked responses show bottom-up, so this puts Keep Time on top.
    alert.add_responses(&[
        ("stop", &gettext("_Stop Timer at {}").replace("{}", &since)),
        ("assign", &gettext("_Assign to Another Task…")),
        ("discard", &gettext("_Discard Time")),
        ("keep", &gettext("_Keep Time")),
    ]);
    alert.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    alert.set_prefer_wide_layout(false);
    alert.set_default_response(Some("keep"));
    alert.set_close_response("keep");
    alert.connect_response(None, glib::clone!(
        #[weak]
        window,
        move |_, response| {
            OPEN.set(false);
            TimeTrackerApplication::get().withdraw_notification(NOTIFICATION_ID);
            respond(&window, away, response);
        }
    ));
    alert.present(Some(window));
    window.present();

    let notification = gio::Notification::new(&title);
    notification.set_body(Some(&gettext("Choose what to do with the time you were away")));
    app.send_notification(Some(NOTIFICATION_ID), &notification);
}

fn respond(window: &gtk::Window, away: Away, response: &str) {
    let app = TimeTrackerApplication::get();
    let store = app.store();
    // Only act on the entry that was running; it may have been stopped since.
    let still_running = store.running_entry().ok().flatten().is_some_and(|e| e.id == away.entry_id);
    if !still_running || response == "keep" {
        return;
    }
    let result = match response {
        "discard" => resume(away),
        "stop" => store.stop_timer(away.since).map(|_| ()),
        "assign" => {
            let dialog = EntryDialog::new_assign(away.since, away.back);
            dialog.connect_saved(move |range| {
                // The dialog added the entry; take its time out of the timer.
                let away = match range {
                    Some((since, back)) => Away { since, back, ..away },
                    None => away,
                };
                if let Err(err) = resume(away) {
                    glib::g_critical!("time-tracker", "Could not split the timer: {err}");
                }
            });
            dialog.present(Some(window));
            Ok(())
        }
        _ => Ok(()),
    };
    if let Err(err) = result {
        glib::g_critical!("time-tracker", "Could not change the timer: {err}");
    }
    app.data_changed();
}

/// Ends the running entry when you left and continues it from your return.
fn resume(away: Away) -> crate::store::Result<()> {
    let store = TimeTrackerApplication::get().store();
    let running = store.running_entry()?.is_some_and(|e| e.id == away.entry_id);
    if running {
        store.resume_after_away(away.since, away.back, &format::local_date(away.back), format::now())?;
    }
    Ok(())
}

/// "10:38", or with the date when it wasn't today.
fn moment(ts: i64) -> String {
    let date = format::local_date(ts);
    if date == format::today() {
        format::clock_time(ts)
    } else {
        format!("{} {}", format::short_date(&date), format::clock_time(ts))
    }
}

/// "42 Minutes", "1 Hour 5 Minutes".
fn duration_words(secs: i64) -> String {
    let minutes = (secs / 60).max(1);
    let (hours, minutes) = (minutes / 60, minutes % 60);
    let hours_text = ngettext("{} Hour", "{} Hours", hours as u32).replace("{}", &hours.to_string());
    let minutes_text = ngettext("{} Minute", "{} Minutes", minutes as u32).replace("{}", &minutes.to_string());
    match (hours, minutes) {
        (0, _) => minutes_text,
        (_, 0) => hours_text,
        _ => format!("{hours_text} {minutes_text}"),
    }
}
