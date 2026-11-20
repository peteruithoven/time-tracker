use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::{gio, glib};

use crate::config::{APP_ID, PROFILE};
use crate::format;

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
        pub track_page: TemplateChild<crate::widgets::TrackPage>,
        #[template_child]
        pub review_page: TemplateChild<crate::widgets::ReviewPage>,
        #[template_child]
        pub sync_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub sync_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub sync_dot: TemplateChild<gtk::Box>,
        #[template_child]
        pub sync_popover: TemplateChild<gtk::Popover>,
        #[template_child]
        pub sync_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub sync_last: TemplateChild<gtk::Label>,
        #[template_child]
        pub sync_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub sync_detail: TemplateChild<gtk::Label>,
        #[template_child]
        pub sync_now_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub sync_banner: TemplateChild<adw::Banner>,
        pub settings: gio::Settings,
    }

    impl Default for TimeTrackerWindow {
        fn default() -> Self {
            Self {
                view_toggle: TemplateChild::default(),
                view_stack: TemplateChild::default(),
                track_page: TemplateChild::default(),
                review_page: TemplateChild::default(),
                sync_button: TemplateChild::default(),
                sync_icon: TemplateChild::default(),
                sync_dot: TemplateChild::default(),
                sync_popover: TemplateChild::default(),
                sync_list: TemplateChild::default(),
                sync_last: TemplateChild::default(),
                sync_title: TemplateChild::default(),
                sync_detail: TemplateChild::default(),
                sync_now_button: TemplateChild::default(),
                sync_banner: TemplateChild::default(),
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
            if PROFILE == "Devel" && !crate::screenshot::enabled() {
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
            type Window = super::TimeTrackerWindow;
            let show = |name: &'static str| {
                gio::ActionEntry::builder(&format!("view-{name}"))
                    .activate(move |window: &Window, _, _| window.imp().view_stack.set_visible_child_name(name))
                    .build()
            };
            let toggle_timer = gio::ActionEntry::builder("toggle-timer")
                .activate(|window: &Window, _, _| window.imp().track_page.toggle_timer())
                .build();
            let choose_task = gio::ActionEntry::builder("choose-task")
                .activate(|window: &Window, _, _| {
                    let imp = window.imp();
                    imp.view_stack.set_visible_child_name("track");
                    imp.track_page.choose_task();
                })
                .build();
            let add_time = gio::ActionEntry::builder("add-time")
                .activate(|window: &Window, _, _| window.imp().review_page.add_time_now())
                .build();
            let month = |name: &str, delta: i32| {
                gio::ActionEntry::builder(name)
                    .activate(move |window: &Window, _, _| {
                        let imp = window.imp();
                        imp.view_stack.set_visible_child_name("review");
                        imp.review_page.step_month(delta);
                    })
                    .build()
            };
            obj.add_action_entries([
                export,
                show("track"),
                show("review"),
                toggle_timer,
                choose_task,
                add_time,
                month("previous-month", -1),
                month("next-month", 1),
            ]);

            self.sync_now_button.connect_clicked(glib::clone!(
                #[weak]
                obj,
                move |button| {
                    if let Some(popover) = button.ancestor(gtk::Popover::static_type()) {
                        popover.downcast::<gtk::Popover>().expect("is a popover").popdown();
                    }
                    // "Review…" while a sync is held.
                    if obj.imp().sync_banner.is_revealed() {
                        obj.review_held_sync();
                    } else {
                        crate::sync::sync(true);
                    }
                }
            ));
            self.sync_banner.connect_button_clicked(glib::clone!(
                #[weak]
                obj,
                move |_| obj.review_held_sync()
            ));
            crate::application::TimeTrackerApplication::get().connect_sync_changed(glib::clone!(
                #[weak]
                obj,
                move |_| obj.update_sync_button()
            ));
            // The waiting entries may have changed since the last status.
            self.sync_popover.connect_show(glib::clone!(
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
            if crate::screenshot::enabled() {
                return self.parent_close_request();
            }
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
            Status::Off | Status::Synced(_) | Status::Pending(_) => "emblem-synchronizing-symbolic",
            Status::Syncing => "content-loading-symbolic",
            Status::Offline(_) => "network-offline-symbolic",
            Status::Error(_) => {
                button.add_css_class("warning");
                "dialog-warning-symbolic"
            }
            Status::Paused(_) => {
                button.add_css_class("warning");
                "media-playback-pause-symbolic"
            }
        };
        // Held pulls stay visible while the account checks again (syncing).
        let store = crate::application::TimeTrackerApplication::get().store();
        let held = store.accounts().unwrap_or_default().iter().find_map(crate::store::Account::held);
        let paused = held.as_ref();
        imp.sync_banner.set_revealed(paused.is_some());
        if let Some(impact) = paused {
            imp.sync_banner.set_title(&crate::sync::paused_banner(impact));
        }
        imp.sync_icon.set_icon_name(Some(icon));
        // A dot while entries wait to be sent.
        let waiting = matches!(status, Status::Pending(n) | Status::Offline(n) if n > 0);
        imp.sync_dot.set_visible(waiting);

        let (title, detail) = crate::sync::describe(&status);
        button.set_tooltip_text(Some(&title));
        imp.sync_title.set_label(&title);
        imp.sync_detail.set_label(&detail);
        imp.sync_detail.set_visible(!detail.is_empty());
        imp.sync_now_button.set_sensitive(status != Status::Syncing || paused.is_some());
        imp.sync_now_button.set_label(&if paused.is_some() { gettext("Review…") } else { gettext("Sync Now") });
        if paused.is_some() {
            imp.sync_now_button.add_css_class("suggested-action");
        } else {
            imp.sync_now_button.remove_css_class("suggested-action");
        }

        let last_sync = store.accounts().unwrap_or_default().iter().filter_map(|a| a.last_sync_at).max();
        imp.sync_last.set_label(&match last_sync {
            Some(at) => gettext("Last sync {}").replace("{}", &moment(at)),
            None => gettext("Not synced yet"),
        });
        match paused {
            Some(impact) => self.fill_paused_list(impact),
            None => self.fill_sync_list(&store.pending_entries().unwrap_or_default()),
        }
    }

    /// Opens the review of the first held sync.
    fn review_held_sync(&self) {
        let store = crate::application::TimeTrackerApplication::get().store();
        if let Some(account) = store.accounts().unwrap_or_default().into_iter().find(|a| a.held_pull.is_some()) {
            crate::widgets::sync_review::present(self, &account);
        }
    }

    /// What a held sync would remove, change and add, in short.
    fn fill_paused_list(&self, impact: &crate::store::PullImpact) {
        use crate::widgets::sync_review::signed_hm;
        let list = &*self.imp().sync_list;
        list.remove_all();
        let entries = |n: usize| ngettext("{} entry", "{} entries", n as u32).replace("{}", &n.to_string());
        let span = |dates: Vec<&str>| match (dates.iter().min(), dates.iter().max()) {
            (Some(first), Some(last)) if first != last => format!(" · {} – {}", format::short_date(first), format::short_date(last)),
            (Some(day), _) => format!(" · {}", format::short_date(day)),
            _ => String::new(),
        };
        let add = |title: String, subtitle: String, total: String| {
            let row = adw::ActionRow::builder().title(title).subtitle(subtitle).use_markup(false).build();
            row.add_suffix(&gtk::Label::builder().label(total).css_classes(["numeric"]).build());
            list.append(&row);
        };
        if !impact.removed.is_empty() {
            let secs: i64 = impact.removed.iter().map(|e| e.secs).sum();
            let dates = impact.removed.iter().map(|e| e.date.as_str()).collect();
            add(gettext("Removed in Fibery"), entries(impact.removed.len()) + &span(dates), format::hm(secs));
        }
        if !impact.changed.is_empty() {
            let secs: i64 = impact.changed.iter().map(|c| c.after.secs - c.before.secs).sum();
            let dates = impact.changed.iter().map(|c| c.before.date.as_str()).collect();
            add(gettext("Changed in Fibery"), entries(impact.changed.len()) + &span(dates), signed_hm(secs));
        }
        if !impact.added.is_empty() {
            let secs: i64 = impact.added.iter().map(|e| e.secs).sum();
            let dates = impact.added.iter().map(|e| e.date.as_str()).collect();
            add(gettext("Added in Fibery"), entries(impact.added.len()) + &span(dates), format::hm(secs));
        }
        list.set_visible(true);
    }

    /// The entries waiting to sync, newest first.
    fn fill_sync_list(&self, entries: &[crate::store::PendingEntry]) {
        const MAX_ROWS: usize = 6;
        let list = &*self.imp().sync_list;
        list.remove_all();
        let now = format::now();
        for entry in entries.iter().take(MAX_ROWS) {
            let day = moment_day(&entry.date);
            let when = match (entry.start_at, entry.end_at) {
                (Some(start), Some(end)) => format!("{day} · {} – {}", format::clock_time(start), format::clock_time(end)),
                (Some(start), None) => format!("{day} · {} – {}", format::clock_time(start), gettext("now")),
                _ => day,
            };
            let subtitle = if entry.deleted { format!("{when} · {}", gettext("Deleted")) } else { when };
            let row = adw::ActionRow::builder()
                .title(&entry.task_name)
                .subtitle(subtitle)
                .use_markup(false)
                .title_lines(1)
                .build();
            let secs = match (entry.start_at, entry.end_at) {
                (Some(start), Some(end)) => end - start,
                (Some(start), None) => now - start,
                _ => entry.duration_secs.unwrap_or_default(),
            };
            let duration = gtk::Label::builder().label(format::hm(secs)).css_classes(["numeric"]).build();
            row.add_suffix(&duration);
            list.append(&row);
        }
        if entries.len() > MAX_ROWS {
            let more = entries.len() - MAX_ROWS;
            let row = adw::ActionRow::builder()
                .title(gettextrs::ngettext("And {} More", "And {} More", more as u32).replace("{}", &more.to_string()))
                .css_classes(["dim-label"])
                .build();
            list.append(&row);
        }
        list.set_visible(!entries.is_empty());
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
        if let Some((width, height)) = crate::screenshot::size() {
            self.set_default_size(width, height);
            if let Some(view) = crate::screenshot::view() {
                self.imp().view_stack.set_visible_child_name(&view);
            }
            return;
        }
        let settings = &self.imp().settings;
        self.set_default_size(settings.int("window-width"), settings.int("window-height"));
        if settings.boolean("is-maximized") {
            self.maximize();
        }
    }
}

/// "Today", "Wed 18" and so on for a date.
fn moment_day(date: &str) -> String {
    if date == format::today() {
        gettext("Today")
    } else {
        format::short_date(date)
    }
}

/// "10:11", with the day when it wasn't today.
fn moment(ts: i64) -> String {
    let date = format::local_date(ts);
    if date == format::today() {
        format::clock_time(ts)
    } else {
        format!("{} {}", format::short_date(&date), format::clock_time(ts))
    }
}
