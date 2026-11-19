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
            SIGNALS.get_or_init(|| vec![glib::subclass::Signal::builder("data-changed").build()])
        }
    }

    impl ApplicationImpl for TimeTrackerApplication {
        fn startup(&self) {
            self.parent_startup();
            gtk::Window::set_default_icon_name(APP_ID);

            let data_dir = glib::user_data_dir().join("time-tracker");
            std::fs::create_dir_all(&data_dir).expect("Could not create the data directory");
            let store = Store::open(&data_dir.join("time-tracker.db"), &gettext("Personal"))
                .expect("Could not open the database");
            self.store.set(Rc::new(store)).expect("Store initialized twice");

            let app = self.obj();
            app.setup_actions();
            app.setup_accels();
        }

        fn activate(&self) {
            let app = self.obj();
            let window = app
                .active_window()
                .unwrap_or_else(|| TimeTrackerWindow::new(&*app).upcast());
            window.present();
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
    pub fn data_changed(&self) {
        self.emit_by_name::<()>("data-changed", &[]);
    }

    pub fn connect_data_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("data-changed", false, move |args| {
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
        self.add_action_entries([quit, about, clients]);
    }

    fn setup_accels(&self) {
        self.set_accels_for_action("app.quit", &["<Control>q"]);
        self.set_accels_for_action("window.close", &["<Control>w"]);
        self.set_accels_for_action("win.export", &["<Control>e"]);
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
