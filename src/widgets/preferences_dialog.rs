//! App preferences: for now, idle detection.

use adw::prelude::*;
use gettextrs::gettext;
use gtk::gio;

use crate::config::APP_ID;

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

    let dialog = adw::PreferencesDialog::new();
    dialog.add(&page);
    dialog
}
