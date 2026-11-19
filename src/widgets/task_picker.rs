//! Popover to search and pick a task, or create a new one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib::clone;
use gtk::{gdk, glib};

use crate::application::TimeTrackerApplication;
use crate::format;
use crate::fuzzy;
use crate::store::Task;
use crate::widgets::{ProjectPicker, set_source_badge};

const RECENT_COUNT: usize = 6;

/// Where a new task will be created.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Target {
    client_id: i64,
    project_id: Option<i64>,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/nl/peteruithoven/TimeTracker/task_picker.ui")]
    pub struct TaskPicker {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub client_filter: TemplateChild<gtk::Box>,
        #[template_child]
        pub task_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub no_results: TemplateChild<gtk::Label>,
        #[template_child]
        pub create_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub create_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub name_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub project_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub project_picker: TemplateChild<ProjectPicker>,
        #[template_child]
        pub project_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub start_check: TemplateChild<gtk::CheckButton>,
        #[template_child]
        pub cancel_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub confirm_button: TemplateChild<gtk::Button>,

        pub tasks: RefCell<Vec<Task>>,
        /// Task id and section title for each row, in list order.
        pub rows: Rc<RefCell<Vec<(i64, String)>>>,
        pub client_filter_id: Cell<Option<i64>>,
        pub target: Cell<Target>,
        /// Reopen after the New Project dialog closes.
        pub reopen: Cell<bool>,
        /// Only pick a task (e.g. in the entry dialog), never start a timer.
        pub select_only: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TaskPicker {
        const NAME: &'static str = "TaskPicker";
        type Type = super::TaskPicker;
        type ParentType = gtk::Popover;

        fn class_init(klass: &mut Self::Class) {
            ProjectPicker::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TaskPicker {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    glib::subclass::Signal::builder("task-chosen")
                        .param_types([i64::static_type()])
                        .build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }

    impl WidgetImpl for TaskPicker {}
    impl PopoverImpl for TaskPicker {}
}

glib::wrapper! {
    pub struct TaskPicker(ObjectSubclass<imp::TaskPicker>)
        @extends gtk::Widget, gtk::Popover,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget,
                    gtk::Native, gtk::ShortcutManager;
}

impl TaskPicker {
    /// In select-only mode creating a task just picks it; there is no
    /// "Start timer" option.
    pub fn set_select_only(&self, select_only: bool) {
        let imp = self.imp();
        imp.select_only.set(select_only);
        imp.start_check.set_visible(!select_only);
        self.update_confirm();
    }

    fn starts_timer(&self) -> bool {
        let imp = self.imp();
        !imp.select_only.get() && imp.start_check.is_active()
    }

    pub fn connect_task_chosen<F: Fn(&Self, i64) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("task-chosen", false, move |args| {
            let picker = args[0].get::<Self>().expect("Emitted by TaskPicker");
            let task_id = args[1].get::<i64>().expect("Task id");
            f(&picker, task_id);
            None
        })
    }

    fn setup(&self) {
        let imp = self.imp();

        let rows = imp.rows.clone();
        imp.task_list.set_header_func(move |row, before| {
            let rows = rows.borrow();
            let section = rows.get(row.index() as usize).map(|(_, s)| s.as_str());
            let previous = before.and_then(|b| rows.get(b.index() as usize)).map(|(_, s)| s.as_str());
            if section.is_some() && section != previous {
                let header = gtk::Label::builder()
                    .label(section.unwrap_or_default())
                    .xalign(0.0)
                    .margin_start(6)
                    .margin_top(if before.is_some() { 12 } else { 4 })
                    .margin_bottom(4)
                    .css_classes(["caption-heading", "dim-label"])
                    .build();
                row.set_header(Some(&header));
            } else {
                row.set_header(None::<&gtk::Widget>);
            }
        });

        self.connect_show(|picker| picker.reset());
        self.connect_closed(|picker| {
            if !picker.imp().reopen.get() {
                picker.imp().stack.set_visible_child_name("list");
            }
        });

        imp.search_entry.connect_search_changed(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| picker.rebuild_list()
        ));
        imp.search_entry.connect_activate(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| {
                if let Some(row) = picker.imp().task_list.selected_row() {
                    picker.choose_row(&row);
                }
            }
        ));

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(clone!(
            #[weak(rename_to = picker)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, modifiers| picker.handle_search_key(key, modifiers)
        ));
        imp.search_entry.add_controller(keys);

        imp.task_list.connect_row_activated(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_, row| picker.choose_row(row)
        ));
        imp.create_button.connect_clicked(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| picker.show_create_form()
        ));

        // Create form
        imp.name_row.connect_changed(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| picker.update_confirm()
        ));
        imp.name_row.connect_entry_activated(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| picker.confirm_create()
        ));
        imp.start_check.connect_toggled(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| picker.update_confirm()
        ));
        imp.cancel_button.connect_clicked(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| picker.imp().stack.set_visible_child_name("list")
        ));
        imp.confirm_button.connect_clicked(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| picker.confirm_create()
        ));
        imp.project_picker.connect_selected(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_, client_id, project_id| {
                picker.set_target(Target { client_id, project_id });
            }
        ));
        imp.project_picker.connect_new_project_requested(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| {
                // Get out of the way of the dialog; reopen when it closes.
                picker.imp().reopen.set(true);
                picker.popdown();
            }
        ));
        imp.project_picker.connect_new_project_finished(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| {
                if picker.imp().reopen.replace(false) {
                    picker.popup();
                }
            }
        ));
    }

    /// Called whenever the popover opens.
    fn reset(&self) {
        let imp = self.imp();
        if imp.stack.visible_child_name().as_deref() == Some("create") {
            imp.name_row.grab_focus();
            return;
        }
        let store = TimeTrackerApplication::get().store();
        imp.tasks.replace(store.open_tasks().unwrap_or_else(|err| {
            glib::g_critical!("time-tracker", "Could not load tasks: {err}");
            Vec::new()
        }));
        self.rebuild_client_filter();
        imp.search_entry.set_text("");
        self.rebuild_list();
        imp.search_entry.grab_focus();
    }

    fn rebuild_client_filter(&self) {
        let imp = self.imp();
        while let Some(child) = imp.client_filter.first_child() {
            imp.client_filter.remove(&child);
        }

        let mut clients: Vec<(i64, String)> = Vec::new();
        for task in imp.tasks.borrow().iter() {
            if !clients.iter().any(|(id, _)| *id == task.client_id) {
                clients.push((task.client_id, task.client_name.clone()));
            }
        }
        clients.sort_by_key(|(_, name)| name.to_lowercase());
        // Only offer a filter when there is something to filter.
        imp.client_filter.set_visible(clients.len() > 1);
        if imp
            .client_filter_id
            .get()
            .is_some_and(|id| !clients.iter().any(|(c, _)| *c == id))
        {
            imp.client_filter_id.set(None);
        }

        let all = self.filter_button(&gettext("All"), None);
        imp.client_filter.append(&all);
        for (id, name) in clients {
            let button = self.filter_button(&name, Some(id));
            button.set_group(Some(&all));
            imp.client_filter.append(&button);
        }
    }

    fn filter_button(&self, label: &str, client_id: Option<i64>) -> gtk::ToggleButton {
        let button = gtk::ToggleButton::builder()
            .label(label)
            .active(self.imp().client_filter_id.get() == client_id)
            .css_classes(["pill", "small", "filter-pill"])
            .build();
        button.connect_toggled(clone!(
            #[weak(rename_to = picker)]
            self,
            move |button| {
                if button.is_active() {
                    picker.imp().client_filter_id.set(client_id);
                    picker.rebuild_list();
                }
            }
        ));
        button
    }

    fn rebuild_list(&self) {
        let imp = self.imp();
        let query = imp.search_entry.text().trim().to_string();
        let tasks = imp.tasks.borrow();
        let filter = imp.client_filter_id.get();
        let visible = tasks
            .iter()
            .filter(|task| filter.is_none_or(|id| task.client_id == id));

        let mut items: Vec<(&Task, String)> = Vec::new();
        if query.is_empty() {
            let recent = gettext("Recent");
            let other = gettext("Other Tasks");
            let mut recent_count = 0;
            for task in visible {
                if task.last_used_at.is_some() && recent_count < RECENT_COUNT {
                    recent_count += 1;
                    items.push((task, recent.clone()));
                } else {
                    items.push((task, other.clone()));
                }
            }
            // Other tasks alphabetically by client, project and name.
            items[recent_count..].sort_by_key(|(task, _)| {
                (
                    task.client_name.to_lowercase(),
                    task.project_name.clone().unwrap_or_default().to_lowercase(),
                    task.name.to_lowercase(),
                )
            });
        } else {
            let matches = gettext("Matches");
            let mut scored: Vec<(i32, &Task)> = visible
                .filter_map(|task| {
                    let haystack = format!(
                        "{} {} {}",
                        task.name,
                        task.project_name.as_deref().unwrap_or_default(),
                        task.client_name
                    );
                    // Task-name matches outrank project/client matches.
                    let name_bonus = fuzzy::score(&query, &task.name).map_or(0, |_| 50);
                    let recent_bonus = if task.last_used_at.is_some() { 5 } else { 0 };
                    fuzzy::score(&query, &haystack).map(|s| (s + name_bonus + recent_bonus, task))
                })
                .collect();
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            items = scored.into_iter().map(|(_, t)| (t, matches.clone())).collect();
        }

        imp.task_list.remove_all();
        imp.rows.replace(items.iter().map(|(t, s)| (t.id, s.clone())).collect());
        for (task, _) in &items {
            imp.task_list.append(&task_row(task));
        }
        imp.task_list.select_row(imp.task_list.row_at_index(0).as_ref());
        imp.task_list.set_visible(!items.is_empty());
        imp.no_results.set_visible(items.is_empty());
        imp.no_results.set_label(&if tasks.is_empty() {
            gettext("No tasks yet")
        } else {
            gettext("No matching tasks")
        });

        imp.create_label.set_label(&if query.is_empty() {
            gettext("New Task…")
        } else {
            gettext("New Task “{}”…").replace("{}", &query)
        });
    }

    fn handle_search_key(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
        let imp = self.imp();
        let list = &*imp.task_list;
        match key {
            gdk::Key::Return | gdk::Key::KP_Enter
                if modifiers.contains(gdk::ModifierType::CONTROL_MASK) =>
            {
                self.show_create_form();
                glib::Propagation::Stop
            }
            gdk::Key::Down | gdk::Key::Up => {
                let current = list.selected_row().map_or(-1, |row| row.index());
                let next = if key == gdk::Key::Down { current + 1 } else { current - 1 };
                if let Some(row) = list.row_at_index(next.max(0)) {
                    list.select_row(Some(&row));
                    imp.search_entry.grab_focus();
                    imp.search_entry.set_position(-1);
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn choose_row(&self, row: &gtk::ListBoxRow) {
        let task_id = self.imp().rows.borrow().get(row.index() as usize).map(|(id, _)| *id);
        if let Some(task_id) = task_id {
            self.popdown();
            self.emit_by_name::<()>("task-chosen", &[&task_id]);
        }
    }

    // ---- Create form ---------------------------------------------------------

    fn show_create_form(&self) {
        let imp = self.imp();
        imp.name_row.set_text(imp.search_entry.text().trim());

        // Default: the filtered client, else where the most recent task lives.
        let store = TimeTrackerApplication::get().store();
        let target = match imp.client_filter_id.get() {
            Some(client_id) => Target { client_id, project_id: None },
            None => match store.last_used_task().ok().flatten() {
                Some(task) => Target { client_id: task.client_id, project_id: task.project_id },
                None => Target {
                    client_id: store.clients().ok().and_then(|c| c.first().map(|c| c.id)).unwrap_or(0),
                    project_id: None,
                },
            },
        };
        self.set_target(target);
        imp.stack.set_visible_child_name("create");
        imp.name_row.grab_focus();
    }

    fn set_target(&self, target: Target) {
        let imp = self.imp();
        imp.target.set(target);
        imp.project_picker.set_selected(target.client_id, target.project_id);

        let store = TimeTrackerApplication::get().store();
        let label = match target.project_id {
            Some(id) => store
                .projects()
                .ok()
                .and_then(|projects| projects.into_iter().find(|p| p.id == id))
                .map(|p| p.name),
            None => store
                .clients()
                .ok()
                .and_then(|clients| clients.into_iter().find(|c| c.id == target.client_id))
                .map(|c| gettext("{} · No Project").replace("{}", &c.name)),
        };
        imp.project_label.set_label(&label.unwrap_or_else(|| gettext("Choose…")));
        self.update_confirm();
    }

    fn update_confirm(&self) {
        let imp = self.imp();
        let has_name = !imp.name_row.text().trim().is_empty();
        imp.confirm_button.set_sensitive(has_name && imp.target.get().client_id != 0);
        imp.confirm_button.set_label(&if self.starts_timer() {
            gettext("Create & Start")
        } else {
            gettext("Create")
        });
    }

    fn confirm_create(&self) {
        let imp = self.imp();
        if !imp.confirm_button.is_sensitive() {
            return;
        }
        let target = imp.target.get();
        let app = TimeTrackerApplication::get();
        let task_id = match app.store().create_task(
            target.client_id,
            target.project_id,
            &imp.name_row.text(),
            format::now(),
        ) {
            Ok(id) => id,
            Err(err) => {
                glib::g_critical!("time-tracker", "Could not create the task: {err}");
                return;
            }
        };
        imp.stack.set_visible_child_name("list");
        self.popdown();
        if self.starts_timer() {
            self.emit_by_name::<()>("task-chosen", &[&task_id]);
        } else {
            app.data_changed();
            if imp.select_only.get() {
                self.emit_by_name::<()>("task-chosen", &[&task_id]);
            }
        }
    }
}

fn task_row(task: &Task) -> gtk::ListBoxRow {
    let title = gtk::Label::builder()
        .label(&task.name)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let badge = gtk::Label::builder().css_classes(["source-badge"]).build();
    set_source_badge(&badge, task.is_remote);
    let context = gtk::Label::builder()
        .label(task.context())
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["caption", "dim-label"])
        .build();

    let meta = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    meta.append(&badge);
    meta.append(&context);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .margin_top(4)
        .margin_bottom(4)
        .build();
    content.append(&title);
    content.append(&meta);

    gtk::ListBoxRow::builder().child(&content).build()
}
