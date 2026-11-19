//! Dialog to add time after the fact, or edit/delete an existing entry.

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::glib::clone;

use crate::application::TimeTrackerApplication;
use crate::format;
use crate::store::{Entry, EntryInput};
use crate::widgets::TaskPicker;

const ONE_DAY: i64 = 24 * 60 * 60;

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/nl/peteruithoven/TimeTracker/entry_dialog.ui")]
    pub struct EntryDialog {
        #[template_child]
        pub cancel_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub save_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub task_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub task_picker: TemplateChild<TaskPicker>,
        #[template_child]
        pub date_popover: TemplateChild<gtk::Popover>,
        #[template_child]
        pub calendar: TemplateChild<gtk::Calendar>,
        #[template_child]
        pub date_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub mode_toggle: TemplateChild<adw::ToggleGroup>,
        #[template_child]
        pub start_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub end_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub running_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub duration_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub total_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub total_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub note_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub delete_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub delete_row: TemplateChild<adw::ButtonRow>,

        /// The entry being edited; `None` when adding.
        pub entry: RefCell<Option<Entry>>,
        pub task_id: Cell<Option<i64>>,
        pub date: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EntryDialog {
        const NAME: &'static str = "EntryDialog";
        type Type = super::EntryDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            TaskPicker::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for EntryDialog {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                // Start and end of the saved entry (0 when it's a duration).
                vec![glib::subclass::Signal::builder("saved")
                    .param_types([i64::static_type(), i64::static_type()])
                    .build()]
            })
        }
    }
    impl WidgetImpl for EntryDialog {}
    impl AdwDialogImpl for EntryDialog {}
}

glib::wrapper! {
    pub struct EntryDialog(ObjectSubclass<imp::EntryDialog>)
        @extends gtk::Widget, adw::Dialog,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::ShortcutManager;
}

impl EntryDialog {
    /// Dialog to add time, optionally for a given task, on `date`.
    pub fn new_add(task_id: Option<i64>, date: &str) -> Self {
        let dialog: Self = glib::Object::new();
        dialog.set_title(&gettext("Add Time"));
        dialog.imp().save_button.set_label(&gettext("Add"));
        dialog.setup();
        dialog.set_task(task_id);
        dialog.set_date(date);
        dialog.imp().mode_toggle.set_active_name(Some("range"));
        dialog.update();
        dialog
    }

    /// Dialog to put the time from `start` to `end` (an away period) on a
    /// task of your choice.
    pub fn new_assign(start: i64, end: i64) -> Self {
        let dialog: Self = glib::Object::new();
        dialog.set_title(&gettext("Assign Time"));
        let imp = dialog.imp();
        imp.save_button.set_label(&gettext("Assign"));
        dialog.setup();
        dialog.set_task(None);
        dialog.set_date(&format::local_date(start));
        imp.mode_toggle.set_active_name(Some("range"));
        imp.start_row.set_text(&format::clock_time(start));
        imp.end_row.set_text(&format::clock_time(end));
        dialog.update();
        dialog
    }

    /// Called after the entry was saved, with its start and end when it
    /// has them.
    pub fn connect_saved<F: Fn(Option<(i64, i64)>) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("saved", false, move |args| {
            let start = args[1].get::<i64>().unwrap_or_default();
            let end = args[2].get::<i64>().unwrap_or_default();
            f((start != 0 && end != 0).then_some((start, end)));
            None
        })
    }

    /// Dialog to edit an existing entry.
    pub fn new_edit(entry: Entry) -> Self {
        let dialog: Self = glib::Object::new();
        dialog.set_title(&gettext("Edit Time"));
        let imp = dialog.imp();
        imp.save_button.set_label(&gettext("Save"));
        imp.delete_group.set_visible(true);
        dialog.setup();

        dialog.set_task(Some(entry.task_id));
        dialog.set_date(&entry.date);
        imp.note_row.set_text(&entry.note);
        match entry.start_at {
            Some(start) => {
                imp.mode_toggle.set_active_name(Some("range"));
                imp.start_row.set_text(&format::clock_time(start));
                if let Some(end) = entry.end_at {
                    imp.end_row.set_text(&format::clock_time(end));
                }
            }
            None => {
                imp.mode_toggle.set_active_name(Some("duration"));
                let secs = entry.duration_secs.unwrap_or_default();
                imp.duration_row.set_text(&format::hm(secs));
            }
        }
        if entry.is_running() {
            // The timer keeps running; only its start can change.
            imp.mode_toggle.set_sensitive(false);
            imp.delete_group.set_visible(false);
        }
        imp.entry.replace(Some(entry));
        dialog.update();
        dialog
    }

    fn setup(&self) {
        let imp = self.imp();
        imp.task_picker.set_select_only(true);
        imp.task_picker.connect_task_chosen(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_, task_id| dialog.set_task(Some(task_id))
        ));
        imp.calendar.connect_day_selected(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |calendar| {
                let imp = dialog.imp();
                let date = format::date_string(
                    calendar.year(),
                    calendar.month() as u32 + 1,
                    calendar.day() as u32,
                );
                dialog.set_date(&date);
                imp.date_popover.popdown();
            }
        ));
        imp.mode_toggle.connect_active_name_notify(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.update()
        ));
        for row in [&*imp.start_row, &*imp.end_row, &*imp.duration_row, &*imp.note_row] {
            row.connect_changed(clone!(
                #[weak(rename_to = dialog)]
                self,
                move |_| dialog.update()
            ));
            row.connect_entry_activated(clone!(
                #[weak(rename_to = dialog)]
                self,
                move |_| dialog.save()
            ));
        }
        imp.cancel_button.connect_clicked(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| {
                dialog.close();
            }
        ));
        imp.save_button.connect_clicked(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.save()
        ));
        imp.delete_row.connect_activated(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.confirm_delete()
        ));
    }

    fn set_task(&self, task_id: Option<i64>) {
        let imp = self.imp();
        imp.task_id.set(task_id);
        let task = task_id.and_then(|id| TimeTrackerApplication::get().store().task(id).ok().flatten());
        match task {
            Some(task) => {
                imp.task_row.set_title(&task.name);
                imp.task_row.set_subtitle(&task.context());
            }
            None => {
                imp.task_row.set_title(&gettext("Choose a Task"));
                imp.task_row.set_subtitle("");
            }
        }
        self.update();
    }

    fn set_date(&self, date: &str) {
        let imp = self.imp();
        imp.date.replace(date.to_string());
        imp.date_label.set_label(&format::short_date(date));
        if let Some((y, m, d)) = format::date_parts(date) {
            imp.calendar.set_year(y);
            imp.calendar.set_month(m as i32 - 1);
            imp.calendar.set_day(d as i32);
        }
        self.update();
    }

    fn is_running(&self) -> bool {
        self.imp().entry.borrow().as_ref().is_some_and(Entry::is_running)
    }

    /// The entry as described by the form, or `None` while it's incomplete.
    fn input(&self) -> Option<EntryInput> {
        let imp = self.imp();
        let date = imp.date.borrow().clone();
        let mut input = EntryInput {
            task_id: imp.task_id.get()?,
            date: date.clone(),
            start_at: None,
            end_at: None,
            duration_secs: None,
            note: imp.note_row.text().trim().to_string(),
        };
        if imp.mode_toggle.active_name().as_deref() == Some("duration") {
            input.duration_secs = Some(format::parse_duration(&imp.duration_row.text())?);
        } else {
            let (h, m) = format::parse_clock(&imp.start_row.text())?;
            let start = format::local_timestamp(&date, h, m)?;
            input.start_at = Some(start);
            if !self.is_running() {
                let (h, m) = format::parse_clock(&imp.end_row.text())?;
                let mut end = format::local_timestamp(&date, h, m)?;
                // An end before the start means the entry ran past midnight.
                if end <= start {
                    end += ONE_DAY;
                }
                input.end_at = Some(end);
            }
        }
        Some(input)
    }

    fn update(&self) {
        let imp = self.imp();
        let duration_mode = imp.mode_toggle.active_name().as_deref() == Some("duration");
        let running = self.is_running();
        imp.start_row.set_visible(!duration_mode);
        imp.end_row.set_visible(!duration_mode && !running);
        imp.running_row.set_visible(!duration_mode && running);
        imp.duration_row.set_visible(duration_mode);
        imp.total_row.set_visible(!duration_mode);

        mark_error(&imp.start_row, format::parse_clock(&imp.start_row.text()).is_none());
        mark_error(&imp.end_row, format::parse_clock(&imp.end_row.text()).is_none());
        mark_error(&imp.duration_row, format::parse_duration(&imp.duration_row.text()).is_none());

        let input = self.input();
        let total = input.as_ref().and_then(|i| match (i.start_at, i.end_at) {
            (Some(start), Some(end)) => Some(end - start),
            (Some(start), None) => Some(format::now() - start),
            _ => None,
        });
        imp.total_label.set_label(&total.map(format::hm).unwrap_or_else(|| "–".into()));
        imp.save_button.set_sensitive(input.is_some());
    }

    fn save(&self) {
        let Some(input) = self.input() else {
            return;
        };
        let app = TimeTrackerApplication::get();
        let store = app.store();
        let now = format::now();
        let result = match &*self.imp().entry.borrow() {
            Some(entry) => store.update_entry(entry.id, &input, now),
            None => store.add_entry(&input, now).map(|_| ()),
        };
        match result {
            Ok(()) => {
                let (start, end) = (input.start_at.unwrap_or_default(), input.end_at.unwrap_or_default());
                self.emit_by_name::<()>("saved", &[&start, &end]);
                app.data_changed();
                self.close();
            }
            Err(err) => glib::g_critical!("time-tracker", "Could not save the entry: {err}"),
        }
    }

    fn confirm_delete(&self) {
        let Some(entry_id) = self.imp().entry.borrow().as_ref().map(|e| e.id) else {
            return;
        };
        let alert = adw::AlertDialog::new(
            Some(&gettext("Delete Entry?")),
            Some(&gettext("The tracked time will be removed permanently.")),
        );
        alert.add_responses(&[("cancel", &gettext("_Cancel")), ("delete", &gettext("_Delete"))]);
        alert.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");
        alert.connect_response(
            None,
            clone!(
                #[weak(rename_to = dialog)]
                self,
                move |_, response| {
                    if response != "delete" {
                        return;
                    }
                    let app = TimeTrackerApplication::get();
                    match app.store().delete_entry(entry_id) {
                        Ok(()) => {
                            app.data_changed();
                            dialog.close();
                        }
                        Err(err) => {
                            glib::g_critical!("time-tracker", "Could not delete the entry: {err}")
                        }
                    }
                }
            ),
        );
        alert.present(Some(self));
    }
}

/// Highlights an entry row with invalid, non-empty content.
fn mark_error(row: &adw::EntryRow, invalid: bool) {
    if invalid && !row.text().trim().is_empty() {
        row.add_css_class("error");
    } else {
        row.remove_css_class("error");
    }
}
