mod application;
mod config;
mod demo;
mod fibery;
mod format;
mod fuzzy;
mod idle;
mod invoicing;
mod report;
mod screenshot;
mod secrets;
mod store;
mod sync;
mod widgets;
mod window;

use gettextrs::{bind_textdomain_codeset, bindtextdomain, gettext, textdomain};
use gtk::prelude::*;
use gtk::{gio, glib};

use self::application::TimeTrackerApplication;
use self::config::{GETTEXT_PACKAGE, LOCALEDIR, PKGDATADIR};

fn main() -> glib::ExitCode {
    bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR).expect("Unable to bind the text domain");
    bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8")
        .expect("Unable to set the text domain encoding");
    textdomain(GETTEXT_PACKAGE).expect("Unable to switch to the text domain");

    let resources = gio::Resource::load(format!("{PKGDATADIR}/resources.gresource"))
        .expect("Could not load resources");
    gio::resources_register(&resources);

    glib::set_application_name(&gettext("Time Tracker"));

    TimeTrackerApplication::new().run()
}
