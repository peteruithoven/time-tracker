use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::glib::clone;

use crate::application::TimeTrackerApplication;
use crate::format;
use crate::store::{Entry, Task};
use crate::widgets::{EntryDialog, TaskPicker, project_color_dot};

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/nl/peteruithoven/TimeTracker/track_page.ui")]
    pub struct TrackPage {
        #[template_child]
        pub timer_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub elapsed_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub status_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub task_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub task_picker: TemplateChild<TaskPicker>,
        #[template_child]
        pub task_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub task_source: TemplateChild<gtk::Label>,
        #[template_child]
        pub task_meta: TemplateChild<gtk::Label>,
        #[template_child]
        pub note_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub today_total_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub today_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub today_empty: TemplateChild<gtk::Label>,

        pub running: RefCell<Option<Entry>>,
        pub current_task: RefCell<Option<Task>>,
        pub today: RefCell<Vec<Entry>>,
        /// Ids of today's entries waiting to sync.
        pub pending: RefCell<std::collections::HashSet<i64>>,
        pub today_date: RefCell<String>,
        pub running_duration_label: RefCell<Option<gtk::Label>>,
        pub tick_source: RefCell<Option<glib::SourceId>>,
        /// Set while the page itself writes the note row, to not save it back.
        pub updating_note: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackPage {
        const NAME: &'static str = "TrackPage";
        type Type = super::TrackPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            TaskPicker::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TrackPage {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            self.timer_button.connect_clicked(clone!(
                #[weak]
                obj,
                move |_| obj.toggle_timer()
            ));
            self.task_picker.connect_task_chosen(clone!(
                #[weak]
                obj,
                move |_, task_id| obj.start_task(task_id)
            ));
            self.note_row.connect_changed(clone!(
                #[weak]
                obj,
                move |row| obj.save_note(&row.text())
            ));
            TimeTrackerApplication::get().connect_data_changed(clone!(
                #[weak]
                obj,
                move |_| obj.reload()
            ));

            obj.reload();
        }

        fn dispose(&self) {
            if let Some(source) = self.tick_source.take() {
                source.remove();
            }
        }
    }

    impl WidgetImpl for TrackPage {}
    impl BinImpl for TrackPage {}
}

glib::wrapper! {
    pub struct TrackPage(ObjectSubclass<imp::TrackPage>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TrackPage {
    /// Opens the task picker.
    pub fn choose_task(&self) {
        self.imp().task_button.popup();
    }

    pub fn toggle_timer(&self) {
        let imp = self.imp();
        if imp.running.borrow().is_some() {
            let store = TimeTrackerApplication::get().store();
            if let Err(err) = store.stop_timer(format::now()) {
                glib::g_critical!("time-tracker", "Could not stop the timer: {err}");
            }
            TimeTrackerApplication::get().data_changed();
            return;
        }

        let task_id = imp.current_task.borrow().as_ref().map(|task| task.id);
        match task_id {
            Some(task_id) => self.start_task(task_id),
            None => imp.task_button.popup(),
        }
    }

    /// Starts tracking `task_id`, stopping whatever was running.
    pub fn start_task(&self, task_id: i64) {
        let now = format::now();
        let store = TimeTrackerApplication::get().store();
        if let Err(err) = store.start_timer(task_id, now, &format::local_date(now)) {
            glib::g_critical!("time-tracker", "Could not start the timer: {err}");
        }
        TimeTrackerApplication::get().data_changed();
    }

    fn save_note(&self, note: &str) {
        let imp = self.imp();
        if imp.updating_note.get() {
            return;
        }
        let Some(entry_id) = imp.running.borrow().as_ref().map(|entry| entry.id) else {
            return;
        };
        let store = TimeTrackerApplication::get().store();
        if let Err(err) = store.set_entry_note(entry_id, note, format::now()) {
            glib::g_critical!("time-tracker", "Could not save the note: {err}");
        }
    }

    /// Reloads everything from the store.
    fn reload(&self) {
        let imp = self.imp();
        let store = TimeTrackerApplication::get().store();
        let today = format::today();

        let running = store.running_entry().unwrap_or_else(|err| {
            glib::g_critical!("time-tracker", "Could not load the running entry: {err}");
            None
        });
        let task = match &running {
            Some(entry) => store.task(entry.task_id).ok().flatten(),
            None => store.last_used_task().ok().flatten(),
        };
        let entries = store.entries_on(&today).unwrap_or_default();

        imp.today_date.replace(today);
        imp.running.replace(running);
        imp.current_task.replace(task);
        imp.today.replace(entries);
        imp.pending.replace(store.pending_entry_ids().unwrap_or_default());

        self.update_timer();
        self.update_task_button();
        self.update_note();
        self.rebuild_today_list();
        self.update_ticking();
    }

    fn update_timer(&self) {
        let imp = self.imp();
        let button = &*imp.timer_button;
        match &*imp.running.borrow() {
            Some(entry) => {
                button.set_icon_name("media-playback-stop-symbolic");
                button.set_tooltip_text(Some(&gettext("Stop Timer")));
                button.remove_css_class("suggested-action");
                button.add_css_class("destructive-action");
                imp.elapsed_label.remove_css_class("dim-label");
                imp.elapsed_label
                    .set_label(&format::hms(entry.duration(format::now())));
                let started = format::clock_time(entry.start_at.unwrap_or_default());
                imp.status_label
                    .set_label(&gettext("Started {}").replace("{}", &started));
            }
            None => {
                button.set_icon_name("media-playback-start-symbolic");
                button.set_tooltip_text(Some(&gettext("Start Timer")));
                button.remove_css_class("destructive-action");
                button.add_css_class("suggested-action");
                imp.elapsed_label.add_css_class("dim-label");
                imp.elapsed_label.set_label(&format::hms(0));
                imp.status_label.set_label(&gettext("Not tracking"));
            }
        }
    }

    fn update_task_button(&self) {
        let imp = self.imp();
        match &*imp.current_task.borrow() {
            Some(task) => {
                imp.task_title.set_label(&task.name);
                imp.task_meta.set_label(&task.context());
                set_source_badge(&imp.task_source, task.is_remote);
                imp.task_source.set_visible(true);
            }
            None => {
                imp.task_title.set_label(&gettext("Choose a Task"));
                imp.task_meta
                    .set_label(&gettext("Pick or create a task to track"));
                imp.task_source.set_visible(false);
            }
        }
    }

    fn update_note(&self) {
        let imp = self.imp();
        let running = imp.running.borrow();
        imp.updating_note.set(true);
        imp.note_row
            .set_text(running.as_ref().map(|e| e.note.as_str()).unwrap_or(""));
        imp.updating_note.set(false);
        imp.note_row.set_sensitive(running.is_some());
    }

    fn rebuild_today_list(&self) {
        let imp = self.imp();
        let list = &*imp.today_list;
        list.remove_all();
        imp.running_duration_label.replace(None);

        let now = format::now();
        let entries = imp.today.borrow();
        for entry in entries.iter() {
            list.append(&self.build_entry_row(entry, now));
        }
        list.set_visible(!entries.is_empty());
        imp.today_empty.set_visible(entries.is_empty());
        self.update_total(now);
    }

    fn build_entry_row(&self, entry: &Entry, now: i64) -> adw::ActionRow {
        let times = match (entry.start_at, entry.end_at) {
            (Some(start), Some(end)) => {
                format!("{} – {}", format::clock_time(start), format::clock_time(end))
            }
            (Some(start), None) => format!("{} – {}", format::clock_time(start), gettext("now")),
            _ => gettext("Manual"),
        };
        let context = entry.project_name.as_deref().unwrap_or(&entry.client_name);

        let row = adw::ActionRow::builder()
            .title(&entry.task_name)
            .subtitle(format!("{times} · {context}"))
            .use_markup(false)
            .title_lines(1)
            .subtitle_lines(1)
            .activatable(true)
            .tooltip_text(gettext("Edit Entry"))
            .build();
        let edit_entry = entry.clone();
        row.connect_activated(clone!(
            #[weak(rename_to = page)]
            self,
            move |_| EntryDialog::new_edit(edit_entry.clone()).present(Some(&page))
        ));
        row.add_prefix(&project_color_dot(entry.color.as_deref()));

        if self.imp().pending.borrow().contains(&entry.id) {
            row.add_suffix(&crate::widgets::pending_dot());
        }
        let duration = gtk::Label::new(Some(&format::hm(entry.duration(now))));
        duration.add_css_class("numeric");
        row.add_suffix(&duration);

        if entry.is_running() {
            duration.add_css_class("running-duration");
            self.imp().running_duration_label.replace(Some(duration));
        } else {
            let resume = gtk::Button::builder()
                .icon_name("media-playback-start-symbolic")
                .tooltip_text(gettext("Continue This Task"))
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .build();
            let task_id = entry.task_id;
            resume.connect_clicked(clone!(
                #[weak(rename_to = page)]
                self,
                move |_| page.start_task(task_id)
            ));
            row.add_suffix(&resume);
        }
        row
    }

    fn update_total(&self, now: i64) {
        let imp = self.imp();
        let total: i64 = imp.today.borrow().iter().map(|e| e.duration(now)).sum();
        imp.today_total_label.set_label(&format::hm(total));
    }

    /// Runs a once-per-second tick while a timer is running.
    fn update_ticking(&self) {
        let imp = self.imp();
        let running = imp.running.borrow().is_some();
        let ticking = imp.tick_source.borrow().is_some();
        if running && !ticking {
            let source = glib::timeout_add_seconds_local(
                1,
                clone!(
                    #[weak(rename_to = page)]
                    self,
                    #[upgrade_or]
                    glib::ControlFlow::Break,
                    move || {
                        page.tick();
                        glib::ControlFlow::Continue
                    }
                ),
            );
            imp.tick_source.replace(Some(source));
        } else if !running
            && ticking
            && let Some(source) = imp.tick_source.take()
        {
            source.remove();
        }
    }

    fn tick(&self) {
        let imp = self.imp();
        // A new day started: reload so "Today" shows the right entries.
        if *imp.today_date.borrow() != format::today() {
            self.reload();
            return;
        }
        let now = format::now();
        if let Some(entry) = &*imp.running.borrow() {
            imp.elapsed_label.set_label(&format::hms(entry.duration(now)));
            if let Some(label) = &*imp.running_duration_label.borrow() {
                label.set_label(&format::hm(entry.duration(now)));
            }
        }
        self.update_total(now);
    }
}

/// Styles `label` as a "Fibery" or "Local" source badge.
pub fn set_source_badge(label: &gtk::Label, is_remote: bool) {
    label.remove_css_class("fibery");
    label.remove_css_class("local");
    if is_remote {
        label.set_label("Fibery");
        label.add_css_class("fibery");
    } else {
        label.set_label(&gettext("Local"));
        label.add_css_class("local");
    }
}
