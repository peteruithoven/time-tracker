//! Searchable popover listing projects grouped per client.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::glib::clone;

use crate::application::TimeTrackerApplication;
use crate::fuzzy;
use crate::widgets::{NewProjectDialog, project_color_dot};

/// One selectable row: a project, or a client's "No Project" option.
#[derive(Debug, Clone)]
pub struct Choice {
    client_id: i64,
    client_name: String,
    client_is_remote: bool,
    project_id: Option<i64>,
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct ProjectPicker {
        pub search_entry: gtk::SearchEntry,
        pub list: gtk::ListBox,
        pub choices: Rc<RefCell<Vec<Choice>>>,
        pub selected: Cell<(i64, Option<i64>)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProjectPicker {
        const NAME: &'static str = "ProjectPicker";
        type Type = super::ProjectPicker;
        type ParentType = gtk::Popover;
    }

    impl ObjectImpl for ProjectPicker {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    // client id, project id (-1 for none)
                    glib::subclass::Signal::builder("selected")
                        .param_types([i64::static_type(), i64::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("new-project-requested").build(),
                    glib::subclass::Signal::builder("new-project-finished").build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }

    impl WidgetImpl for ProjectPicker {}
    impl PopoverImpl for ProjectPicker {}
}

glib::wrapper! {
    pub struct ProjectPicker(ObjectSubclass<imp::ProjectPicker>)
        @extends gtk::Widget, gtk::Popover,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget,
                    gtk::Native, gtk::ShortcutManager;
}

impl Default for ProjectPicker {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl ProjectPicker {
    pub fn set_selected(&self, client_id: i64, project_id: Option<i64>) {
        self.imp().selected.set((client_id, project_id));
    }

    pub fn connect_selected<F: Fn(&Self, i64, Option<i64>) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("selected", false, move |args| {
            let picker = args[0].get::<Self>().expect("Emitted by ProjectPicker");
            let client_id = args[1].get::<i64>().expect("Client id");
            let project_id = args[2].get::<i64>().expect("Project id");
            f(&picker, client_id, (project_id >= 0).then_some(project_id));
            None
        })
    }

    pub fn connect_new_project_requested<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("new-project-requested", false, move |args| {
            f(&args[0].get::<Self>().expect("Emitted by ProjectPicker"));
            None
        })
    }

    pub fn connect_new_project_finished<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("new-project-finished", false, move |args| {
            f(&args[0].get::<Self>().expect("Emitted by ProjectPicker"));
            None
        })
    }

    fn setup(&self) {
        let imp = self.imp();
        imp.search_entry.set_placeholder_text(Some(&gettext("Search projects")));

        imp.list.set_selection_mode(gtk::SelectionMode::None);
        imp.list.add_css_class("navigation-sidebar");
        let choices = imp.choices.clone();
        imp.list.set_header_func(move |row, before| {
            let choices = choices.borrow();
            let current = choices.get(row.index() as usize);
            let previous = before.and_then(|b| choices.get(b.index() as usize));
            match current {
                Some(choice) if previous.is_none_or(|p| p.client_id != choice.client_id) => {
                    let header = gtk::Box::builder()
                        .spacing(6)
                        .margin_start(6)
                        .margin_top(if before.is_some() { 12 } else { 4 })
                        .margin_bottom(4)
                        .build();
                    header.append(
                        &gtk::Label::builder()
                            .label(&choice.client_name)
                            .css_classes(["caption-heading", "dim-label"])
                            .build(),
                    );
                    let badge = gtk::Label::builder().css_classes(["source-badge"]).build();
                    crate::widgets::set_source_badge(&badge, choice.client_is_remote);
                    header.append(&badge);
                    row.set_header(Some(&header));
                }
                _ => row.set_header(None::<&gtk::Widget>),
            }
        });
        imp.list.connect_row_activated(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_, row| picker.choose(row.index())
        ));

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(360)
            .child(&imp.list)
            .build();

        let new_project = gtk::Button::builder()
            .child(
                &adw::ButtonContent::builder()
                    .icon_name("list-add-symbolic")
                    .label(gettext("New Project…"))
                    .halign(gtk::Align::Start)
                    .build(),
            )
            .css_classes(["flat", "accent"])
            .build();
        new_project.connect_clicked(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| picker.open_new_project_dialog()
        ));

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .width_request(300)
            .build();
        content.append(&imp.search_entry);
        content.append(&scrolled);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(&new_project);
        self.set_child(Some(&content));

        imp.search_entry.connect_search_changed(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| picker.rebuild()
        ));
        imp.search_entry.connect_activate(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| {
                if !picker.imp().choices.borrow().is_empty() {
                    picker.choose(0);
                }
            }
        ));
        self.connect_show(|picker| {
            picker.imp().search_entry.set_text("");
            picker.rebuild();
            picker.imp().search_entry.grab_focus();
        });
    }

    fn rebuild(&self) {
        let imp = self.imp();
        let store = TimeTrackerApplication::get().store();
        let query = imp.search_entry.text().trim().to_string();
        let clients = store.clients().unwrap_or_default();
        let projects = store.projects().unwrap_or_default();

        let mut choices = Vec::new();
        let mut rows = Vec::new();
        for client in &clients {
            let make = |project_id| Choice {
                client_id: client.id,
                client_name: client.name.clone(),
                client_is_remote: client.source.is_some(),
                project_id,
            };
            for project in projects.iter().filter(|p| p.client_id == client.id) {
                let text = format!("{} {}", project.name, client.name);
                if fuzzy::score(&query, &text).is_some() {
                    choices.push(make(Some(project.id)));
                    rows.push((project.name.clone(), Some(project.color.clone())));
                }
            }
            let no_project = gettext("No Project");
            if fuzzy::score(&query, &format!("{no_project} {}", client.name)).is_some() {
                choices.push(make(None));
                rows.push((no_project, None));
            }
        }

        imp.list.remove_all();
        // The header func reads `choices`, so update it before adding rows.
        imp.choices.replace(choices.clone());
        let selected = imp.selected.get();
        for (choice, (label, color)) in choices.iter().zip(rows) {
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            if color.is_some() {
                row_box.append(&project_color_dot(color.as_deref()));
            }
            let title = gtk::Label::builder()
                .label(&label)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            if choice.project_id.is_none() {
                title.add_css_class("dim-label");
            }
            row_box.append(&title);
            if (choice.client_id, choice.project_id) == selected {
                row_box.append(&gtk::Image::from_icon_name("object-select-symbolic"));
            }
            imp.list.append(&gtk::ListBoxRow::builder().child(&row_box).build());
        }
    }

    fn choose(&self, index: i32) {
        let choice = self.imp().choices.borrow().get(index as usize).cloned();
        if let Some(choice) = choice {
            self.set_selected(choice.client_id, choice.project_id);
            self.popdown();
            let project_id = choice.project_id.unwrap_or(-1);
            self.emit_by_name::<()>("selected", &[&choice.client_id, &project_id]);
        }
    }

    fn open_new_project_dialog(&self) {
        let parent = self.parent();
        let preselect = self.imp().selected.get().0;
        self.emit_by_name::<()>("new-project-requested", &[]);
        self.popdown();

        let dialog = NewProjectDialog::new(Some(preselect));
        dialog.connect_project_created(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_, client_id, project_id| {
                picker.set_selected(client_id, Some(project_id));
                picker.emit_by_name::<()>("selected", &[&client_id, &project_id]);
            }
        ));
        dialog.connect_closed(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| picker.emit_by_name::<()>("new-project-finished", &[])
        ));
        dialog.present(parent.as_ref());
    }
}
