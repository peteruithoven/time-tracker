//! Clients & Projects: manage clients, their billing, projects and tasks.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::glib;
use gtk::glib::clone;

use crate::application::TimeTrackerApplication;
use crate::format;
use crate::store::{Client, Project};
use crate::widgets::{
    NewProjectDialog, PROJECT_COLORS, color_label, project_color_dot, set_source_badge,
};

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct ClientsDialog {
        pub clients_group: adw::PreferencesGroup,
        pub archived_group: adw::PreferencesGroup,
        pub rows: RefCell<Vec<adw::ActionRow>>,
        /// `data-changed` handlers to disconnect when the dialog closes.
        pub handlers: RefCell<Vec<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ClientsDialog {
        const NAME: &'static str = "ClientsDialog";
        type Type = super::ClientsDialog;
        type ParentType = adw::PreferencesDialog;
    }

    impl ObjectImpl for ClientsDialog {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }

    impl WidgetImpl for ClientsDialog {}
    impl AdwDialogImpl for ClientsDialog {
        fn closed(&self) {
            let app = TimeTrackerApplication::get();
            for handler in self.handlers.take() {
                app.disconnect(handler);
            }
            self.parent_closed();
        }
    }
    impl PreferencesDialogImpl for ClientsDialog {}
}

glib::wrapper! {
    pub struct ClientsDialog(ObjectSubclass<imp::ClientsDialog>)
        @extends gtk::Widget, adw::Dialog, adw::PreferencesDialog,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::ShortcutManager;
}

impl Default for ClientsDialog {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl ClientsDialog {
    fn setup(&self) {
        let imp = self.imp();
        self.set_title(&gettext("Clients & Projects"));
        self.set_search_enabled(false);
        self.set_content_width(480);
        // Without this the dialog takes the height of the client list, which
        // is too short for the client and project pages pushed onto it.
        // libadwaita still shrinks it to fit smaller windows.
        self.set_content_height(640);

        imp.clients_group.set_title(&gettext("Clients"));
        let add = header_button("list-add-symbolic", &gettext("Add Client"));
        add.connect_clicked(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.add_client()
        ));
        imp.clients_group.set_header_suffix(Some(&add));
        imp.archived_group.set_title(&gettext("Archived"));
        imp.archived_group
            .set_description(Some(&gettext("Hidden from the task picker. Their time still shows in Review.")));

        let page = adw::PreferencesPage::new();
        page.add(&imp.clients_group);
        page.add(&imp.archived_group);
        self.add(&page);

        self.on_data_changed(clone!(
            #[weak(rename_to = dialog)]
            self,
            move || dialog.refresh()
        ));
        self.refresh();
    }

    /// Calls `f` whenever stored data changes, until the dialog closes.
    fn on_data_changed(&self, f: impl Fn() + 'static) {
        let handler = TimeTrackerApplication::get().connect_data_changed(move |_| f());
        self.imp().handlers.borrow_mut().push(handler);
    }

    fn refresh(&self) {
        let imp = self.imp();
        for row in imp.rows.take() {
            if let Some(group) = row.ancestor(adw::PreferencesGroup::static_type()) {
                group.downcast::<adw::PreferencesGroup>().expect("is a group").remove(&row);
            }
        }

        let store = TimeTrackerApplication::get().store();
        let clients = store.all_clients().unwrap_or_default();
        let projects = store.projects().unwrap_or_default();
        let mut rows = Vec::new();
        for client in &clients {
            let count = projects.iter().filter(|p| p.client_id == client.id).count() as u32;
            let projects_text = ngettext("{} project", "{} projects", count).replace("{}", &count.to_string());
            let row = adw::ActionRow::builder()
                .title(&client.name)
                .subtitle(format!("{} · {projects_text}", billing_summary(client)))
                .use_markup(false)
                .activatable(true)
                .build();
            let avatar = adw::Avatar::new(32, Some(&client.name), true);
            row.add_prefix(&avatar);
            let badge = gtk::Label::builder().valign(gtk::Align::Center).css_classes(["source-badge"]).build();
            set_source_badge(&badge, client.source.is_some());
            row.add_suffix(&badge);
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let client_id = client.id;
            row.connect_activated(clone!(
                #[weak(rename_to = dialog)]
                self,
                move |_| dialog.show_client(client_id)
            ));
            if client.archived {
                imp.archived_group.add(&row);
            } else {
                imp.clients_group.add(&row);
            }
            rows.push(row);
        }
        imp.archived_group.set_visible(clients.iter().any(|c| c.archived));
        imp.rows.replace(rows);
    }

    fn add_client(&self) {
        let entry = gtk::Entry::builder()
            .placeholder_text(gettext("Name"))
            .activates_default(true)
            .build();
        let alert = adw::AlertDialog::new(Some(&gettext("New Client")), None);
        alert.set_extra_child(Some(&entry));
        alert.add_responses(&[("cancel", &gettext("_Cancel")), ("add", &gettext("_Add"))]);
        alert.set_response_appearance("add", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("add"));
        alert.set_close_response("cancel");
        alert.set_response_enabled("add", false);
        entry.connect_changed(clone!(
            #[weak]
            alert,
            move |entry| alert.set_response_enabled("add", !entry.text().trim().is_empty())
        ));
        alert.connect_response(
            Some("add"),
            clone!(
                #[weak(rename_to = dialog)]
                self,
                #[weak]
                entry,
                move |_, _| {
                    let app = TimeTrackerApplication::get();
                    match app.store().create_client(&entry.text(), format::now()) {
                        Ok(id) => {
                            app.data_changed();
                            dialog.show_client(id);
                        }
                        Err(err) => glib::g_critical!("time-tracker", "Could not add the client: {err}"),
                    }
                }
            ),
        );
        alert.present(Some(self));
        entry.grab_focus();
    }

    fn show_client(&self, client_id: i64) {
        let store = TimeTrackerApplication::get().store();
        let Some(client) = store.client(client_id).ok().flatten() else {
            return;
        };
        let page = adw::PreferencesPage::new();

        // Details
        let details = adw::PreferencesGroup::new();
        let name_row = name_row(&client.name);
        name_row.connect_apply(move |row| {
            update_client(client_id, |c| c.name = row.text().trim().to_string());
        });
        let billable = adw::SwitchRow::builder()
            .title(gettext("Billable"))
            .subtitle(gettext("Time for non-billable clients is left out of invoices"))
            .active(client.billable)
            .build();
        billable.connect_active_notify(move |row| update_client(client_id, |c| c.billable = row.is_active()));
        let rate = rate_row(&gettext("Hourly Rate"), &gettext("In euros, 0 when there is none"), client.hourly_rate_cents);
        rate.connect_value_notify(move |row| {
            update_client(client_id, |c| c.hourly_rate_cents = cents(row.value()));
        });
        details.add(&name_row);
        details.add(&billable);
        details.add(&rate);
        page.add(&details);

        // Projects
        let projects_group = adw::PreferencesGroup::builder().title(gettext("Projects")).build();
        let add = header_button("list-add-symbolic", &gettext("Add Project"));
        add.connect_clicked(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| NewProjectDialog::new(Some(client_id)).present(Some(&dialog))
        ));
        projects_group.set_header_suffix(Some(&add));
        page.add(&projects_group);
        let project_rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::default();
        let refresh_projects = clone!(
            #[weak(rename_to = dialog)]
            self,
            #[weak]
            projects_group,
            #[strong]
            project_rows,
            move || dialog.fill_projects(&projects_group, &project_rows, client_id)
        );
        refresh_projects();
        self.on_data_changed(refresh_projects);

        // Archive
        let archive = adw::PreferencesGroup::new();
        let archive_row = adw::ButtonRow::builder()
            .title(if client.archived { gettext("Restore Client") } else { gettext("Archive Client") })
            .start_icon_name(if client.archived { "edit-undo-symbolic" } else { "folder-symbolic" })
            .build();
        let archived = client.archived;
        archive_row.connect_activated(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| {
                update_client(client_id, |c| c.archived = !archived);
                dialog.pop_subpage();
            }
        ));
        archive.add(&archive_row);
        page.add(&archive);

        self.push_subpage(&navigation_page(&client.name, &page));
    }

    fn fill_projects(&self, group: &adw::PreferencesGroup, rows: &RefCell<Vec<adw::ActionRow>>, client_id: i64) {
        for row in rows.take() {
            group.remove(&row);
        }
        let store = TimeTrackerApplication::get().store();
        let mut new_rows = Vec::new();
        for project in store.client_projects(client_id).unwrap_or_default() {
            let tasks = store.project_tasks(client_id, Some(project.id)).unwrap_or_default();
            let open = tasks.iter().filter(|t| !t.done).count() as u32;
            let mut subtitle = vec![ngettext("{} open task", "{} open tasks", open).replace("{}", &open.to_string())];
            if let Some(rate) = project.hourly_rate_cents {
                subtitle.insert(0, gettext("{}/h").replace("{}", &format::euros(rate)));
            }
            if project.archived {
                subtitle.insert(0, gettext("Archived"));
            }
            let row = adw::ActionRow::builder()
                .title(&project.name)
                .subtitle(subtitle.join(" · "))
                .use_markup(false)
                .activatable(true)
                .build();
            if project.archived {
                row.add_css_class("dim-label");
            }
            row.add_prefix(&project_color_dot(Some(&project.color)));
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let project_id = project.id;
            row.connect_activated(clone!(
                #[weak(rename_to = dialog)]
                self,
                move |_| dialog.show_tasks(client_id, Some(project_id))
            ));
            group.add(&row);
            new_rows.push(row);
        }

        // Tasks directly on the client.
        let loose = store.project_tasks(client_id, None).unwrap_or_default();
        if !loose.is_empty() {
            let open = loose.iter().filter(|t| !t.done).count() as u32;
            let row = adw::ActionRow::builder()
                .title(gettext("No Project"))
                .subtitle(ngettext("{} open task", "{} open tasks", open).replace("{}", &open.to_string()))
                .activatable(true)
                .build();
            row.add_prefix(&project_color_dot(None));
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            row.connect_activated(clone!(
                #[weak(rename_to = dialog)]
                self,
                move |_| dialog.show_tasks(client_id, None)
            ));
            group.add(&row);
            new_rows.push(row);
        }
        group.set_description(new_rows.is_empty().then(|| gettext("No projects yet")).as_deref());
        rows.replace(new_rows);
    }

    /// A project's details and tasks; for `project_id` `None`, the client's
    /// tasks without a project.
    fn show_tasks(&self, client_id: i64, project_id: Option<i64>) {
        let store = TimeTrackerApplication::get().store();
        let client = store.client(client_id).ok().flatten();
        let project = project_id.and_then(|id| store.project(id).ok().flatten());
        let page = adw::PreferencesPage::new();

        if let Some(project) = &project {
            let details = adw::PreferencesGroup::new();
            let project_id = project.id;
            let name_row = name_row(&project.name);
            name_row.connect_apply(move |row| {
                update_project(project_id, |p| p.name = row.text().trim().to_string());
            });
            details.add(&name_row);

            let client_rate = client
                .as_ref()
                .and_then(|c| c.hourly_rate_cents)
                .map(|r| gettext("0 uses the client rate ({})").replace("{}", &format::euros(r)))
                .unwrap_or_else(|| gettext("In euros, 0 to use the client rate"));
            let rate = rate_row(&gettext("Hourly Rate"), &client_rate, project.hourly_rate_cents);
            rate.connect_value_notify(move |row| {
                update_project(project_id, |p| p.hourly_rate_cents = cents(row.value()));
            });
            details.add(&rate);
            page.add(&details);

            let colors = adw::PreferencesGroup::builder().title(gettext("Color")).build();
            colors.add(&color_picker(&project.color, move |color| {
                update_project(project_id, |p| p.color = color.to_string());
            }));
            page.add(&colors);
        }

        let tasks_group = adw::PreferencesGroup::builder()
            .title(gettext("Tasks"))
            .description(gettext("Done tasks are hidden from the task picker."))
            .build();
        page.add(&tasks_group);
        let task_rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::default();
        let refresh_tasks = clone!(
            #[weak]
            tasks_group,
            #[strong]
            task_rows,
            move || fill_tasks(&tasks_group, &task_rows, client_id, project_id)
        );
        refresh_tasks();
        self.on_data_changed(refresh_tasks);

        if let Some(project) = &project {
            let archive = adw::PreferencesGroup::new();
            let archived = project.archived;
            let project_id = project.id;
            let archive_row = adw::ButtonRow::builder()
                .title(if archived { gettext("Restore Project") } else { gettext("Archive Project") })
                .start_icon_name(if archived { "edit-undo-symbolic" } else { "folder-symbolic" })
                .build();
            archive_row.connect_activated(clone!(
                #[weak(rename_to = dialog)]
                self,
                move |_| {
                    update_project(project_id, |p| p.archived = !archived);
                    dialog.pop_subpage();
                }
            ));
            archive.add(&archive_row);
            page.add(&archive);
        }

        let title = project.map(|p| p.name).unwrap_or_else(|| gettext("No Project"));
        self.push_subpage(&navigation_page(&title, &page));
    }
}

fn fill_tasks(group: &adw::PreferencesGroup, rows: &RefCell<Vec<adw::ActionRow>>, client_id: i64, project_id: Option<i64>) {
    for row in rows.take() {
        group.remove(&row);
    }
    let tasks = TimeTrackerApplication::get()
        .store()
        .project_tasks(client_id, project_id)
        .unwrap_or_default();
    let mut new_rows = Vec::new();
    for task in tasks {
        let check = gtk::CheckButton::builder()
            .active(task.done)
            .valign(gtk::Align::Center)
            .tooltip_text(gettext("Done"))
            .build();
        let row = adw::ActionRow::builder()
            .title(&task.name)
            .use_markup(false)
            .activatable_widget(&check)
            .build();
        if task.done {
            row.add_css_class("dim-label");
        }
        row.add_prefix(&check);
        let task_id = task.id;
        check.connect_toggled(move |check| {
            let app = TimeTrackerApplication::get();
            if let Err(err) = app.store().set_task_done(task_id, check.is_active()) {
                glib::g_critical!("time-tracker", "Could not update the task: {err}");
            }
            app.data_changed();
        });
        group.add(&row);
        new_rows.push(row);
    }
    group.set_description(Some(&if new_rows.is_empty() {
        gettext("No tasks yet. Create them from the task picker.")
    } else {
        gettext("Done tasks are hidden from the task picker.")
    }));
    rows.replace(new_rows);
}

fn navigation_page(title: &str, content: &adw::PreferencesPage) -> adw::NavigationPage {
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(content));
    adw::NavigationPage::builder().title(title).child(&toolbar).build()
}

fn header_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
}

fn name_row(name: &str) -> adw::EntryRow {
    adw::EntryRow::builder()
        .title(gettext("Name"))
        .text(name)
        .show_apply_button(true)
        .build()
}

fn rate_row(title: &str, subtitle: &str, cents: Option<i64>) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(0.0, 10_000.0, 5.0);
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.set_digits(2);
    row.set_value(cents.unwrap_or(0) as f64 / 100.0);
    row
}

/// Euros from a spin row as cents; 0 means "not set".
fn cents(euros: f64) -> Option<i64> {
    let cents = (euros * 100.0).round() as i64;
    (cents > 0).then_some(cents)
}

fn billing_summary(client: &Client) -> String {
    if !client.billable {
        gettext("Non-billable")
    } else if let Some(rate) = client.hourly_rate_cents {
        gettext("{}/h").replace("{}", &format::euros(rate))
    } else {
        gettext("No rate set")
    }
}

fn update_client(id: i64, change: impl FnOnce(&mut Client)) {
    let app = TimeTrackerApplication::get();
    let store = app.store();
    let Some(mut client) = store.client(id).ok().flatten() else {
        return;
    };
    change(&mut client);
    if client.name.is_empty() {
        return;
    }
    if let Err(err) = store.update_client(&client) {
        glib::g_critical!("time-tracker", "Could not save the client: {err}");
    }
    app.data_changed();
}

fn update_project(id: i64, change: impl FnOnce(&mut Project)) {
    let app = TimeTrackerApplication::get();
    let store = app.store();
    let Some(mut project) = store.project(id).ok().flatten() else {
        return;
    };
    change(&mut project);
    if project.name.is_empty() {
        return;
    }
    if let Err(err) = store.update_project(&project) {
        glib::g_critical!("time-tracker", "Could not save the project: {err}");
    }
    app.data_changed();
}

/// A row of color swatches; calls `on_pick` with the chosen color name.
fn color_picker(selected: &str, on_pick: impl Fn(&str) + 'static) -> gtk::Box {
    let on_pick = Rc::new(on_pick);
    let content = gtk::Box::builder().spacing(8).build();
    let mut group: Option<gtk::ToggleButton> = None;
    for &color in PROJECT_COLORS {
        let swatch = gtk::ToggleButton::builder()
            .tooltip_text(color_label(color))
            .css_classes(["color-swatch", color])
            .active(color == selected)
            .build();
        swatch.set_group(group.as_ref());
        let on_pick = on_pick.clone();
        swatch.connect_toggled(move |swatch| {
            if swatch.is_active() {
                on_pick(color);
            }
        });
        content.append(&swatch);
        group.get_or_insert(swatch);
    }
    content
}
