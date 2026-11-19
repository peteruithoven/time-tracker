use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use crate::config::{APP_ID, PROFILE};

mod imp {
    use super::*;

    #[derive(Debug, gtk::CompositeTemplate)]
    #[template(resource = "/nl/peteruithoven/TimeTracker/window.ui")]
    pub struct TimeTrackerWindow {
        #[template_child]
        pub view_toggle: TemplateChild<adw::ToggleGroup>,
        #[template_child]
        pub view_stack: TemplateChild<adw::ViewStack>,
        pub settings: gio::Settings,
    }

    impl Default for TimeTrackerWindow {
        fn default() -> Self {
            Self {
                view_toggle: TemplateChild::default(),
                view_stack: TemplateChild::default(),
                settings: gio::Settings::new(APP_ID),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TimeTrackerWindow {
        const NAME: &'static str = "TimeTrackerWindow";
        type Type = super::TimeTrackerWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            crate::widgets::TrackPage::ensure_type();
            crate::widgets::ReviewPage::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TimeTrackerWindow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // Striped headerbar for development builds.
            if PROFILE == "Devel" {
                obj.add_css_class("devel");
            }

            // Restore the last view, then keep the toggle group and the stack in sync.
            self.settings
                .bind("last-view", &*self.view_stack, "visible-child-name")
                .build();
            self.view_stack
                .bind_property("visible-child-name", &*self.view_toggle, "active-name")
                .bidirectional()
                .sync_create()
                .build();

            obj.load_window_size();
        }
    }

    impl WidgetImpl for TimeTrackerWindow {}

    impl WindowImpl for TimeTrackerWindow {
        fn close_request(&self) -> glib::Propagation {
            if let Err(err) = self.obj().save_window_size() {
                glib::g_warning!("time-tracker", "Failed to save window state: {err}");
            }
            self.parent_close_request()
        }
    }

    impl ApplicationWindowImpl for TimeTrackerWindow {}
    impl AdwApplicationWindowImpl for TimeTrackerWindow {}
}

glib::wrapper! {
    pub struct TimeTrackerWindow(ObjectSubclass<imp::TimeTrackerWindow>)
        @extends gtk::Widget, gtk::Window, gtk::ApplicationWindow, adw::ApplicationWindow,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl TimeTrackerWindow {
    pub fn new(application: &impl IsA<gtk::Application>) -> Self {
        glib::Object::builder()
            .property("application", application)
            .build()
    }

    fn save_window_size(&self) -> Result<(), glib::BoolError> {
        let settings = &self.imp().settings;
        let (width, height) = self.default_size();
        settings.set_int("window-width", width)?;
        settings.set_int("window-height", height)?;
        settings.set_boolean("is-maximized", self.is_maximized())?;
        Ok(())
    }

    fn load_window_size(&self) {
        let settings = &self.imp().settings;
        self.set_default_size(settings.int("window-width"), settings.int("window-height"));
        if settings.boolean("is-maximized") {
            self.maximize();
        }
    }
}
