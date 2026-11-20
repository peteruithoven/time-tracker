//! App preferences: idle detection and backups.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};

use crate::backup;
use crate::config::APP_ID;
use crate::format;

pub fn new() -> adw::PreferencesDialog {
    let settings = gio::Settings::new(APP_ID);

    let detection = adw::SwitchRow::builder()
        .title(gettext("Ask About Away Time"))
        .subtitle(gettext("When you come back while the timer runs, ask whether to keep that time"))
        .build();
    settings.bind("idle-detection", &detection, "active").build();

    let minutes = adw::SpinRow::builder()
        .title(gettext("Away After"))
        .subtitle(gettext("Minutes without keyboard or mouse input"))
        .adjustment(&gtk::Adjustment::new(10.0, 1.0, 240.0, 1.0, 5.0, 0.0))
        .build();
    settings.bind("idle-minutes", &minutes, "value").build();
    detection.bind_property("active", &minutes, "sensitive").sync_create().build();

    let group = adw::PreferencesGroup::builder().title(gettext("Idle Detection")).build();
    group.add(&detection);
    group.add(&minutes);
    let page = adw::PreferencesPage::new();
    page.add(&group);
    page.add(&backups_group(&settings));

    let dialog = adw::PreferencesDialog::new();
    dialog.add(&page);
    dialog
}

/// The rows of the Backups group, held weakly by the closures' state.
struct Backups {
    settings: gio::Settings,
    folder: glib::WeakRef<adw::ActionRow>,
    reset: glib::WeakRef<gtk::Button>,
    notice: glib::WeakRef<adw::ActionRow>,
    subfolder: glib::WeakRef<gtk::Button>,
    last: glib::WeakRef<adw::ActionRow>,
    back_up: glib::WeakRef<gtk::Button>,
}

fn backups_group(settings: &gio::Settings) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(gettext("Backups"))
        .description(gettext(
            "A snapshot of your data is saved every day and before a sync removes entries. API keys are left out.",
        ))
        .build();

    let folder = adw::ActionRow::builder().title(gettext("Folder")).subtitle_selectable(true).build();
    let reset = gtk::Button::builder()
        .icon_name("edit-undo-symbolic")
        .tooltip_text(gettext("Use the Default Folder"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let choose = gtk::Button::builder().label(gettext("Choose…")).valign(gtk::Align::Center).build();
    folder.add_suffix(&reset);
    folder.add_suffix(&choose);

    let notice = adw::ActionRow::builder().visible(false).build();
    notice.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
    let subfolder = gtk::Button::builder()
        .label(gettext("Use a Subfolder"))
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    notice.add_suffix(&subfolder);

    let last = adw::ActionRow::builder().title(gettext("Last Snapshot")).build();
    let open = gtk::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text(gettext("Open Folder"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let back_up = gtk::Button::builder().label(gettext("Back Up Now")).valign(gtk::Align::Center).build();
    last.add_suffix(&open);
    last.add_suffix(&back_up);

    group.add(&folder);
    group.add(&notice);
    group.add(&last);

    let state = Rc::new(Backups {
        settings: settings.clone(),
        folder: folder.downgrade(),
        reset: reset.downgrade(),
        notice: notice.downgrade(),
        subfolder: subfolder.downgrade(),
        last: last.downgrade(),
        back_up: back_up.downgrade(),
    });
    state.refresh(None);

    settings.connect_changed(
        Some("backup-folder"),
        glib::clone!(
            #[strong]
            state,
            move |_, _| state.refresh(None)
        ),
    );
    reset.connect_clicked(glib::clone!(
        #[strong]
        state,
        move |_| {
            let _ = state.settings.set_string("backup-folder", "");
        }
    ));
    choose.connect_clicked(glib::clone!(
        #[strong]
        state,
        move |button| {
            let window = button.root().and_downcast::<gtk::Window>();
            let dialog = gtk::FileDialog::builder().title(gettext("Choose a Folder for Snapshots")).modal(true).build();
            if let Some(folder) = backup::chosen_folder() {
                dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
            }
            let state = state.clone();
            glib::spawn_future_local(async move {
                if let Ok(file) = dialog.select_folder_future(window.as_ref()).await
                    && let Some(path) = file.path()
                {
                    let _ = state.settings.set_string("backup-folder", &path.to_string_lossy());
                }
            });
        }
    ));
    subfolder.connect_clicked(glib::clone!(
        #[strong]
        state,
        move |_| {
            let (folder, _) = backup::folder();
            let sub = folder.join(gettext("Time Tracker Backups"));
            match std::fs::create_dir_all(&sub) {
                Ok(()) => {
                    let _ = state.settings.set_string("backup-folder", &sub.to_string_lossy());
                }
                Err(err) => state.refresh(Some(&err.to_string())),
            }
        }
    ));
    open.connect_clicked(|button| {
        let window = button.root().and_downcast::<gtk::Window>();
        let (folder, _) = backup::folder();
        let _ = std::fs::create_dir_all(&folder);
        gtk::FileLauncher::new(Some(&gio::File::for_path(folder))).launch(
            window.as_ref(),
            gio::Cancellable::NONE,
            |result| {
                if let Err(err) = result {
                    glib::g_warning!("time-tracker", "Could not open the backup folder: {err}");
                }
            },
        );
    });
    back_up.connect_clicked(glib::clone!(
        #[strong]
        state,
        move |button| {
            button.set_sensitive(false);
            let state = state.clone();
            glib::spawn_future_local(async move {
                let result = backup::snapshot(backup::Kind::Daily).await;
                state.refresh(result.err().as_deref());
            });
        }
    ));
    group
}

impl Backups {
    /// Shows the folder, what's in it and the last snapshot. `error`: the
    /// last attempt to save one failed.
    fn refresh(&self, error: Option<&str>) {
        let (Some(folder_row), Some(reset), Some(notice), Some(subfolder), Some(last), Some(back_up)) = (
            self.folder.upgrade(),
            self.reset.upgrade(),
            self.notice.upgrade(),
            self.subfolder.upgrade(),
            self.last.upgrade(),
            self.back_up.upgrade(),
        ) else {
            return;
        };
        back_up.set_sensitive(true);
        let chosen = backup::chosen_folder();
        let (folder, fallback) = backup::folder();
        reset.set_visible(chosen.is_some());
        let contents = backup::scan(&folder).unwrap_or_default();

        let mut subtitle = display_path(chosen.as_deref().unwrap_or(&folder));
        if chosen.is_none() {
            subtitle = format!("{subtitle} ({})", gettext("default"));
        }
        let hosts = contents.other_hosts();
        if !hosts.is_empty() {
            subtitle.push('\n');
            subtitle.push_str(&gettext("Also holds snapshots from {}").replace("{}", &hosts.join(", ")));
        }
        folder_row.set_subtitle(&subtitle);

        // Warn about a missing folder, or one with other files in it.
        notice.set_visible(true);
        subfolder.set_visible(false);
        if fallback {
            notice.set_title(&gettext("Folder Not Available"));
            notice.set_subtitle(&gettext("Snapshots go to the default folder until it's back."));
        } else if chosen.is_some() && contents.other_files > 0 {
            notice.set_title(&gettext("This Folder Has Other Files"));
            notice.set_subtitle(&gettext(
                "Snapshots are saved next to them. Only files named time-tracker-….db from this computer are ever removed.",
            ));
            subfolder.set_visible(true);
        } else {
            notice.set_visible(false);
        }

        let newest = contents.newest().and_then(|n| n.timestamp());
        last.set_subtitle(&match (error, newest) {
            (Some(error), _) => gettext("Could not save a snapshot: {}").replace("{}", error),
            (None, Some(at)) => moment(at),
            (None, None) => gettext("None yet"),
        });
    }
}

/// A folder as the person knows it, with `~` for the home folder. A folder
/// chosen through the portal lives under `/run/user/…/doc/…` in the sandbox;
/// newer portals tell where it really is, older ones (before 1.19) don't,
/// and then only its name is shown ("Open Folder" shows where it is).
fn display_path(path: &Path) -> String {
    const HOST_PATH: &str = "xattr::document-portal.host-path";
    let host = gio::File::for_path(path)
        .query_info(HOST_PATH, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
        .ok()
        .and_then(|info| info.attribute_as_string(HOST_PATH))
        .map(|p| PathBuf::from(p.as_str()));
    let path = match host {
        Some(host) => host,
        None if portal_path(path) => {
            return path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        }
        None => path.to_path_buf(),
    };
    match path.strip_prefix(glib::home_dir()) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// A path the document portal exported into the sandbox.
fn portal_path(path: &Path) -> bool {
    let parts: Vec<_> = path.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    matches!(parts.as_slice(), [_, run, user, _, doc, ..] if run == "run" && user == "user" && doc == "doc")
}

/// "Today at 09:30" or "Wed 18 Nov at 09:30".
fn moment(ts: i64) -> String {
    let date = format::local_date(ts);
    let day = if date == format::today() { gettext("Today") } else { format::short_date(&date) };
    gettext("{day} at {time}").replace("{day}", &day).replace("{time}", &format::clock_time(ts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_portal_folders_by_name() {
        assert!(portal_path(Path::new("/run/user/1000/doc/9a6548ae/Time")));
        assert!(!portal_path(Path::new("/home/me/Nextcloud/Time")));
        assert_eq!(display_path(Path::new("/run/user/1000/doc/9a6548ae/Time")), "Time");
        let home = glib::home_dir().join("Nextcloud/Time");
        assert_eq!(display_path(&home), "~/Nextcloud/Time");
    }
}
