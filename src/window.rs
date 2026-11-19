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
        #[template_child]
        pub review_page: TemplateChild<crate::widgets::ReviewPage>,
        #[template_child]
        pub sync_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub sync_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub sync_detail: TemplateChild<gtk::Label>,
        #[template_child]
        pub sync_now_button: TemplateChild<gtk::Button>,
        pub settings: gio::Settings,
    }

    impl Default for TimeTrackerWindow {
        fn default() -> Self {
            Self {
                view_toggle: TemplateChild::default(),
                view_stack: TemplateChild::default(),
                review_page: TemplateChild::default(),
                sync_button: TemplateChild::default(),
                sync_title: TemplateChild::default(),
                sync_detail: TemplateChild::default(),
                sync_now_button: TemplateChild::default(),
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

            // Export the month shown in Review (switching to it if needed).
            let export = gio::ActionEntry::builder("export")
                .activate(|window: &super::TimeTrackerWindow, _, _| {
                    let imp = window.imp();
                    imp.view_stack.set_visible_child_name("review");
                    imp.review_page.export_csv();
                })
                .build();
            obj.add_action_entries([export]);

            self.sync_now_button.connect_clicked(|button| {
                if let Some(popover) = button.ancestor(gtk::Popover::static_type()) {
                    popover.downcast::<gtk::Popover>().expect("is a popover").popdown();
                }
                crate::sync::sync(true);
            });
            crate::application::TimeTrackerApplication::get().connect_sync_changed(glib::clone!(
                #[weak]
                obj,
                move |_| obj.update_sync_button()
            ));
            obj.update_sync_button();

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

    /// Header bar sync icon: hidden without connections, otherwise shows
    /// the state, with details in its popover.
    fn update_sync_button(&self) {
        use crate::sync::Status;
        let imp = self.imp();
        let status = crate::sync::status();
        let button = &*imp.sync_button;
        button.set_visible(status != Status::Off);
        for class in ["error", "warning", "accent"] {
            button.remove_css_class(class);
        }
        let icon = match &status {
            Status::Off | Status::Synced(_) => "emblem-synchronizing-symbolic",
            Status::Syncing => "content-loading-symbolic",
            Status::Pending(_) => {
                button.add_css_class("accent");
                "emblem-synchronizing-symbolic"
            }
            Status::Offline(_) => "network-offline-symbolic",
            Status::Error(_) => {
                button.add_css_class("warning");
                "dialog-warning-symbolic"
            }
        };
        button.set_icon_name(icon);
        let (title, detail) = crate::sync::describe(&status);
        button.set_tooltip_text(Some(&title));
        imp.sync_title.set_label(&title);
        imp.sync_detail.set_label(&detail);
        imp.sync_detail.set_visible(!detail.is_empty());
        imp.sync_now_button.set_sensitive(status != Status::Syncing);
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
