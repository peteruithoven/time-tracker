use std::cell::OnceCell;
use std::rc::Rc;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};

use crate::config::{APP_ID, RESOURCE_BASE_PATH, VERSION};
use crate::store::Store;
use crate::window::TimeTrackerWindow;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct TimeTrackerApplication {
        pub store: OnceCell<Rc<Store>>,
        pub idle: OnceCell<Rc<crate::idle::Monitor>>,
        /// A `data-changed` emission is scheduled.
        pub change_pending: std::cell::Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TimeTrackerApplication {
        const NAME: &'static str = "TimeTrackerApplication";
        type Type = super::TimeTrackerApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for TimeTrackerApplication {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    glib::subclass::Signal::builder("data-changed").build(),
                    glib::subclass::Signal::builder("sync-changed").build(),
                ]
            })
        }
    }

    impl ApplicationImpl for TimeTrackerApplication {
        fn startup(&self) {
            self.parent_startup();
            gtk::Window::set_default_icon_name(APP_ID);
            crate::screenshot::configure();

            let data_dir = super::data_dir();
            std::fs::create_dir_all(&data_dir).expect("Could not create the data directory");
            let store = Store::open(&data_dir.join("time-tracker.db"), &gettext("Personal"))
                .expect("Could not open the database");
            if crate::screenshot::demo()
                && let Err(err) = crate::demo::seed(&store)
            {
                glib::g_warning!("time-tracker", "Could not add demo data: {err}");
            }
            self.store.set(Rc::new(store)).expect("Store initialized twice");

            let app = self.obj();
            app.setup_actions();
            app.setup_accels();
            crate::sync::start();
            crate::watchdog::start();
            if !crate::screenshot::enabled() {
                crate::backup::start();
                crate::launcher::start();
                let monitor = crate::idle::Monitor::start(|away| {
                    let app = super::TimeTrackerApplication::get();
                    if let Some(window) = app.active_window().or_else(|| app.windows().into_iter().next()) {
                        crate::widgets::away_alert::present(&window, away);
                    }
                });
                let _ = self.idle.set(monitor);
            }
        }

        fn activate(&self) {
            let app = self.obj();
            let window = app
                .active_window()
                .unwrap_or_else(|| TimeTrackerWindow::new(&*app).upcast());
            window.present();
            crate::screenshot::open_page(&window);
            crate::screenshot::take(&window, app.upcast_ref());
        }
    }

    impl GtkApplicationImpl for TimeTrackerApplication {}
    impl AdwApplicationImpl for TimeTrackerApplication {}
}

glib::wrapper! {
    pub struct TimeTrackerApplication(ObjectSubclass<imp::TimeTrackerApplication>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

/// The app's own data folder, holding the database (and default backups).
pub fn data_dir() -> std::path::PathBuf {
    glib::user_data_dir().join("time-tracker")
}

impl TimeTrackerApplication {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", APP_ID)
            .property("resource-base-path", RESOURCE_BASE_PATH)
            .build()
    }

    /// The running application instance.
    pub fn get() -> Self {
        gio::Application::default()
            .and_downcast::<Self>()
            .expect("Application is not running")
    }

    pub fn store(&self) -> Rc<Store> {
        self.imp().store.get().expect("Store is not initialized").clone()
    }

    /// Tells all views that stored data changed and they should reload.
    ///
    /// Emitted once after the current event, so views never rebuild widgets
    /// in the middle of handling a click or focus change on them.
    pub fn data_changed(&self) {
        if self.imp().change_pending.replace(true) {
            return;
        }
        glib::idle_add_local_once(glib::clone!(
            #[weak(rename_to = app)]
            self,
            move || {
                app.imp().change_pending.set(false);
                app.emit_by_name::<()>("data-changed", &[]);
            }
        ));
    }

    pub fn connect_data_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("data-changed", false, move |args| {
            let app = args[0].get::<Self>().expect("Signal emitted by the application");
            f(&app);
            None
        })
    }

    /// The sync status changed (see `crate::sync::status`).
    pub fn connect_sync_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("sync-changed", false, move |args| {
            let app = args[0].get::<Self>().expect("Signal emitted by the application");
            f(&app);
            None
        })
    }

    fn setup_actions(&self) {
        let quit = gio::ActionEntry::builder("quit")
            .activate(|app: &Self, _, _| app.quit())
            .build();
        let about = gio::ActionEntry::builder("about")
            .activate(|app: &Self, _, _| app.show_about())
            .build();
        let clients = gio::ActionEntry::builder("clients")
            .activate(|app: &Self, _, _| {
                crate::widgets::ClientsDialog::default().present(app.active_window().as_ref());
            })
            .build();
        let shortcuts = gio::ActionEntry::builder("shortcuts")
            .activate(|app: &Self, _, _| app.show_shortcuts())
            .build();
        let preferences = gio::ActionEntry::builder("preferences")
            .activate(|app: &Self, _, _| {
                crate::widgets::preferences_dialog::new().present(app.active_window().as_ref());
            })
            .build();
        self.add_action_entries([quit, about, clients, shortcuts, preferences]);
    }

    fn setup_accels(&self) {
        self.set_accels_for_action("app.quit", &["<Control>q"]);
        self.set_accels_for_action("window.close", &["<Control>w"]);
        for (action, accels) in [
            ("win.export", &["<Control>e"][..]),
            ("win.toggle-timer", &["<Control>t"]),
            ("win.choose-task", &["<Control>k"]),
            ("win.add-time", &["<Control>n"]),
            ("win.view-track", &["<Control>1"]),
            ("win.view-review", &["<Control>2"]),
            ("win.previous-month", &["<Control>Page_Up"]),
            ("win.next-month", &["<Control>Page_Down"]),
            ("app.clients", &["<Control>comma"]),
            ("app.shortcuts", &["<Control>question"]),
        ] {
            self.set_accels_for_action(action, accels);
        }
    }

    fn show_shortcuts(&self) {
        let builder = gtk::Builder::from_resource("/nl/peteruithoven/TimeTracker/keyboard_shortcuts.ui");
        let dialog: adw::ShortcutsDialog = builder.object("shortcuts_dialog").expect("Shortcuts dialog in resource");
        dialog.present(self.active_window().as_ref());
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name(gettext("Time Tracker"))
            .application_icon(APP_ID)
            .developer_name("Peter Uithoven")
            .version(VERSION)
            .website("https://github.com/peteruithoven/time-tracker")
            .issue_url("https://github.com/peteruithoven/time-tracker/issues")
            .license_type(gtk::License::MitX11)
            .build();
        about.present(self.active_window().as_ref());
    }
}

impl Default for TimeTrackerApplication {
    fn default() -> Self {
        Self::new()
    }
}
