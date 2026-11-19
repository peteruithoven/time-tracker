//! Clients & Projects: manage clients, their billing, projects and tasks.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::glib::clone;
use gtk::{gio, glib};

use crate::application::TimeTrackerApplication;
use crate::format;
use crate::store::{Client, Project};
use crate::widgets::{
    ConnectDialog, NewProjectDialog, PROJECT_COLORS, color_label, project_color_dot, set_source_badge,
};

/// Rows of a preferences group, rebuilt only when what they show changes.
///
/// Rebuilding while the user tabs through the page would destroy the row
/// GTK is moving keyboard focus to (and spew criticals), so unchanged
/// sections are left alone.
struct Section<R> {
    rows: RefCell<Vec<R>>,
    key: RefCell<Option<String>>,
}

impl<R> Default for Section<R> {
    fn default() -> Self {
        Self { rows: RefCell::default(), key: RefCell::default() }
    }
}

impl<R: IsA<gtk::Widget>> Section<R> {
    /// Clears the rows for a rebuild; `false` if `key` didn't change.
    fn begin(&self, group: &adw::PreferencesGroup, key: String) -> bool {
        if self.key.borrow().as_deref() == Some(key.as_str()) {
            return false;
        }
        self.key.replace(Some(key));
        for row in self.rows.take() {
            group.remove(&row);
        }
        true
    }

    fn set_rows(&self, rows: Vec<R>) {
        self.rows.replace(rows);
    }
}

/// Wraps `f` so it runs once after the current event, however often it's
/// called meanwhile.
fn deferred(f: impl Fn() + 'static) -> impl Fn() + 'static {
    let f = Rc::new(f);
    let scheduled = Rc::new(std::cell::Cell::new(false));
    move || {
        if scheduled.replace(true) {
            return;
        }
        let f = f.clone();
        let scheduled = scheduled.clone();
        glib::idle_add_local_once(move || {
            scheduled.set(false);
            f();
        });
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct ClientsDialog {
        pub clients_group: adw::PreferencesGroup,
        pub archived_group: adw::PreferencesGroup,
        pub rows: RefCell<Vec<adw::ActionRow>>,
        pub rows_key: RefCell<Option<String>>,
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
        let f = deferred(f);
        let handler = TimeTrackerApplication::get().connect_data_changed(move |_| f());
        self.imp().handlers.borrow_mut().push(handler);
    }

    fn refresh(&self) {
        let imp = self.imp();
        let store = TimeTrackerApplication::get().store();
        let clients = store.all_clients().unwrap_or_default();
        let projects = store.projects().unwrap_or_default();
        let key = format!("{clients:?}{:?}", projects.iter().map(|p| p.client_id).collect::<Vec<_>>());
        if imp.rows_key.borrow().as_deref() == Some(key.as_str()) {
            return;
        }
        imp.rows_key.replace(Some(key));
        for row in imp.rows.take() {
            if let Some(group) = row.ancestor(adw::PreferencesGroup::static_type()) {
                group.downcast::<adw::PreferencesGroup>().expect("is a group").remove(&row);
            }
        }

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

        // Connection
        let connection = adw::PreferencesGroup::builder().title(gettext("Connection")).build();
        page.add(&connection);
        // Disconnect gets its own group, like other destructive actions.
        let disconnect_group = adw::PreferencesGroup::new();
        let disconnect = adw::ButtonRow::builder().title(gettext("Disconnect")).build();
        disconnect.add_css_class("destructive-action");
        disconnect.connect_activated(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| {
                let store = TimeTrackerApplication::get().store();
                if let Some(account) = store.account_for_client(client_id).ok().flatten() {
                    dialog.confirm_disconnect(account);
                }
            }
        ));
        disconnect_group.add(&disconnect);
        page.add(&disconnect_group);
        let connection_rows: Rc<Section<gtk::Widget>> = Rc::default();
        let refresh_connection = clone!(
            #[weak(rename_to = dialog)]
            self,
            #[weak]
            connection,
            #[weak]
            disconnect_group,
            #[strong]
            connection_rows,
            move || dialog.fill_connection(&connection, &disconnect_group, &connection_rows, client_id)
        );
        refresh_connection();
        let on_sync = deferred(refresh_connection.clone());
        let handler = TimeTrackerApplication::get().connect_sync_changed(move |_| on_sync());
        self.imp().handlers.borrow_mut().push(handler);
        self.on_data_changed(refresh_connection);

        self.add_invoicing(&page, &client);

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
        let project_rows: Rc<Section<adw::ActionRow>> = Rc::default();
        let refresh_projects = clone!(
            #[weak(rename_to = dialog)]
            self,
            #[weak]
            projects_group,
            #[weak]
            add,
            #[strong]
            project_rows,
            move || {
                // Projects of clients connected to Fibery come from Fibery.
                let connected = TimeTrackerApplication::get()
                    .store()
                    .account_for_client(client_id)
                    .ok()
                    .flatten()
                    .is_some();
                add.set_visible(!connected);
                dialog.fill_projects(&projects_group, &project_rows, client_id)
            }
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

    fn fill_connection(
        &self,
        group: &adw::PreferencesGroup,
        disconnect_group: &adw::PreferencesGroup,
        section: &Section<gtk::Widget>,
        client_id: i64,
    ) {
        let store = TimeTrackerApplication::get().store();
        let account = store.account_for_client(client_id).ok().flatten();
        disconnect_group.set_visible(account.is_some());
        let syncing = crate::sync::status() == crate::sync::Status::Syncing;
        if !section.begin(group, format!("{account:?}{syncing}")) {
            return;
        }
        let mut new_rows: Vec<gtk::Widget> = Vec::new();
        match account {
            None => {
                let row = adw::ActionRow::builder()
                    .title(gettext("Not Connected"))
                    .subtitle(gettext("Tasks and time stay on this computer"))
                    .build();
                let connect = gtk::Button::builder()
                    .label(gettext("Connect Fibery…"))
                    .valign(gtk::Align::Center)
                    .build();
                connect.connect_clicked(clone!(
                    #[weak(rename_to = dialog)]
                    self,
                    move |_| ConnectDialog::new(client_id).present(Some(&dialog))
                ));
                row.add_suffix(&connect);
                group.add(&row);
                new_rows.push(row.upcast());
            }
            Some(account) => {
                let host = account.workspace.trim_start_matches("https://").to_string();
                let row = adw::ActionRow::builder()
                    .title("Fibery")
                    .subtitle(gettext("{user} · {workspace}").replace("{user}", &account.user_name).replace("{workspace}", &host))
                    .use_markup(false)
                    .build();
                let badge = gtk::Label::builder().valign(gtk::Align::Center).css_classes(["source-badge"]).build();
                set_source_badge(&badge, true);
                row.add_prefix(&badge);
                group.add(&row);
                new_rows.push(row.upcast());

                let status = adw::ActionRow::builder().title(gettext("Sync")).use_markup(false).build();
                let subtitle = if syncing {
                    gettext("Syncing…")
                } else if let Some(error) = &account.last_error {
                    error.clone()
                } else if let Some(at) = account.last_sync_at {
                    gettext("Last synced {date} at {time}")
                        .replace("{date}", &format::short_date(&format::local_date(at)))
                        .replace("{time}", &format::clock_time(at))
                } else {
                    gettext("Not synced yet")
                };
                status.set_subtitle(&subtitle);
                if account.last_error.is_some() && !syncing {
                    status.add_css_class("error");
                }
                let sync_now = gtk::Button::builder()
                    .label(gettext("Sync Now"))
                    .valign(gtk::Align::Center)
                    .sensitive(!syncing)
                    .build();
                sync_now.connect_clicked(|_| crate::sync::sync(true));
                status.add_suffix(&sync_now);
                group.add(&status);
                new_rows.push(status.upcast());

                let mirror = adw::SwitchRow::builder()
                    .title(gettext("Mirror Running Timer"))
                    .subtitle(gettext("Create the time log in Fibery when the timer starts, not when it stops"))
                    .active(account.mirror_timer)
                    .build();
                let account_id = account.id;
                mirror.connect_active_notify(move |row| {
                    let app = TimeTrackerApplication::get();
                    let _ = app.store().set_account_mirror(account_id, row.is_active());
                    app.data_changed();
                });
                group.add(&mirror);
                new_rows.push(mirror.upcast());

                if let Ok(mapping) = serde_json::from_str::<crate::fibery::Mapping>(&account.mapping) {
                    let subpage_row = |title: &str, subtitle: String| {
                        let row = adw::ActionRow::builder()
                            .title(title)
                            .subtitle(subtitle)
                            .use_markup(false)
                            .activatable(true)
                            .build();
                        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
                        row
                    };
                    let logs = subpage_row(
                        &gettext("Time Logs"),
                        format!(
                            "{} · {}",
                            crate::fibery::schema::display_name(&mapping.log_type),
                            crate::fibery::schema::display_name(&mapping.task_type)
                        ),
                    );
                    let tasks = subpage_row(
                        &gettext("Show Tasks"),
                        crate::widgets::fibery_settings::show_tasks_summary(&mapping),
                    );
                    for (row, time_logs) in [(&logs, true), (&tasks, false)] {
                        let account = account.clone();
                        row.connect_activated(clone!(
                            #[weak(rename_to = dialog)]
                            self,
                            move |_| {
                                let dialog = dialog.upcast_ref::<adw::PreferencesDialog>();
                                if time_logs {
                                    crate::widgets::fibery_settings::show_time_logs(dialog, &account);
                                } else {
                                    crate::widgets::fibery_settings::show_tasks(dialog, &account);
                                }
                            }
                        ));
                        group.add(row);
                        new_rows.push(row.clone().upcast());
                    }
                }

                if account.token.is_some() {
                    let key = adw::ActionRow::builder()
                        .title(gettext("API Key"))
                        .subtitle(gettext(
                            "Stored in the app's own data because the system keyring couldn't be used. Each sync tries the keyring again.",
                        ))
                        .build();
                    key.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
                    group.add(&key);
                    new_rows.push(key.upcast());
                }
            }
        }
        section.set_rows(new_rows);
    }

    /// The "Invoicing" section: pick an invoice script, then fill in its
    /// settings and API token right here. A status row checks them.
    fn add_invoicing(&self, page: &adw::PreferencesPage, client: &Client) {
        use crate::invoicing::SCRIPTS;

        let client_id = client.id;
        let group = adw::PreferencesGroup::builder().title(gettext("Invoicing")).build();
        let names = gtk::StringList::new(&[&gettext("None")]);
        for script in SCRIPTS {
            names.append(script.name);
        }
        let script_at = |index: u32| (index as usize).checked_sub(1).and_then(|i| SCRIPTS.get(i));
        let selected = client
            .invoice_script
            .as_deref()
            .and_then(|id| SCRIPTS.iter().position(|s| s.id == id))
            .map_or(0, |i| i as u32 + 1);
        let script_row = adw::ComboRow::builder()
            .title(gettext("Invoice Script"))
            .model(&names)
            .selected(selected)
            .build();
        group.add(&script_row);
        page.add(&group);

        let rows: Rc<Section<gtk::Widget>> = Rc::default();
        fill_invoicing(&group, &rows, client_id, script_at(selected));
        script_row.connect_selected_notify(clone!(
            #[weak]
            group,
            move |row| {
                let script = script_at(row.selected());
                let app = TimeTrackerApplication::get();
                let store = app.store();
                let previous = store.client(client_id).ok().flatten().and_then(|c| c.invoice_script);
                if previous.as_deref() == script.map(|s| s.id) {
                    return;
                }
                if let Some(previous) = previous {
                    glib::spawn_future_local(async move {
                        let _ = crate::secrets::delete_invoice_token(&previous, client_id).await;
                    });
                }
                let _ = store.set_client_invoice_script(client_id, script.map(|s| s.id), None);
                app.data_changed();
                fill_invoicing(&group, &rows, client_id, script);
            }
        ));
    }

    fn confirm_disconnect(&self, account: crate::store::Account) {
        let alert = adw::AlertDialog::new(
            Some(&gettext("Disconnect Fibery?")),
            Some(&gettext("Synced projects, tasks and time stay in the app, but changes no longer sync. Nothing is removed from Fibery.")),
        );
        alert.add_responses(&[("cancel", &gettext("_Cancel")), ("disconnect", &gettext("_Disconnect"))]);
        alert.set_response_appearance("disconnect", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");
        alert.connect_response(Some("disconnect"), move |_, _| {
            let account = account.clone();
            glib::spawn_future_local(async move {
                let app = TimeTrackerApplication::get();
                if let Err(err) = crate::secrets::delete_token(&account.workspace, &account.user_id).await {
                    glib::g_warning!("time-tracker", "Could not remove the token from the keyring: {err}");
                }
                let _ = app.store().remove_account(account.id);
                app.data_changed();
                crate::sync::refresh_status();
            });
        });
        alert.present(Some(self));
    }

    fn fill_projects(&self, group: &adw::PreferencesGroup, section: &Section<adw::ActionRow>, client_id: i64) {
        let store = TimeTrackerApplication::get().store();
        let open_count = |project_id| {
            let tasks = store.project_tasks(client_id, project_id).unwrap_or_default();
            (tasks.len(), tasks.iter().filter(|t| !t.done).count() as u32)
        };
        let projects: Vec<_> = store
            .client_projects(client_id)
            .unwrap_or_default()
            .into_iter()
            .map(|p| {
                let (_, open) = open_count(Some(p.id));
                (p, open)
            })
            .collect();
        let loose = open_count(None);
        if !section.begin(group, format!("{projects:?}{loose:?}")) {
            return;
        }
        let mut new_rows = Vec::new();
        for (project, open) in projects {
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
        let (loose_count, open) = loose;
        if loose_count > 0 {
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
        section.set_rows(new_rows);
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
        let task_rows: Rc<Section<adw::ActionRow>> = Rc::default();
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

/// The rows below the script choice: its settings, API token and status.
fn fill_invoicing(
    group: &adw::PreferencesGroup,
    section: &Section<gtk::Widget>,
    client_id: i64,
    script: Option<&'static crate::invoicing::Script>,
) {
    if !section.begin(group, script.map(|s| s.id).unwrap_or_default().to_string()) {
        return;
    }
    let Some(script) = script else {
        group.set_description(Some(&gettext("Creates the month's invoice from the Review view")));
        section.set_rows(Vec::new());
        return;
    };
    group.set_description(Some(script.help));
    let store = TimeTrackerApplication::get().store();
    let settings =
        crate::invoicing::parse_settings(store.client(client_id).ok().flatten().and_then(|c| c.invoice_settings).as_deref());

    let status = adw::ActionRow::builder().title(gettext("Status")).use_markup(false).build();
    let check = Rc::new(clone!(
        #[weak]
        status,
        move || check_invoicing(&status, client_id, script)
    ));
    let mut rows: Vec<gtk::Widget> = Vec::new();

    for setting in script.settings {
        let row = adw::EntryRow::builder()
            .title(setting.title)
            .text(settings.get(setting.key).map(String::as_str).unwrap_or_default())
            .show_apply_button(true)
            .build();
        let key = setting.key;
        let check = check.clone();
        row.connect_apply(move |row| {
            let app = TimeTrackerApplication::get();
            let store = app.store();
            let client = store.client(client_id).ok().flatten();
            let mut settings = crate::invoicing::parse_settings(client.and_then(|c| c.invoice_settings).as_deref());
            settings.insert(key.to_string(), row.text().trim().to_string());
            let json = serde_json::to_string(&settings).unwrap_or_default();
            let _ = store.set_client_invoice_script(client_id, Some(script.id), Some(&json));
            app.data_changed();
            check();
        });
        group.add(&row);
        rows.push(row.upcast());
    }

    // Empty until you type a new token; the status says whether one is stored.
    let token = adw::PasswordEntryRow::builder()
        .title(gettext("API Token"))
        .show_apply_button(true)
        .build();
    token.connect_apply(clone!(
        #[weak]
        status,
        #[strong]
        check,
        move |row| {
            let text = row.text().trim().to_string();
            let check = check.clone();
            glib::spawn_future_local(clone!(
                #[weak]
                status,
                async move {
                    let result = if text.is_empty() {
                        crate::secrets::delete_invoice_token(script.id, client_id).await
                    } else {
                        crate::secrets::store_invoice_token(script.id, client_id, script.name, &text).await
                    };
                    match result {
                        Ok(()) => check(),
                        Err(err) => set_invoicing_status(
                            &status,
                            &gettext("Could not store the token in the keyring: {}").replace("{}", &err),
                            true,
                        ),
                    }
                }
            ));
        }
    ));
    group.add(&token);
    rows.push(token.upcast());
    group.add(&status);
    rows.push(status.upcast());
    check();
    section.set_rows(rows);
}

fn set_invoicing_status(row: &adw::ActionRow, text: &str, error: bool) {
    row.set_subtitle(text);
    if error {
        row.add_css_class("error");
    } else {
        row.remove_css_class("error");
    }
}

/// Checks the stored settings and token with the script and shows the
/// outcome in `status`.
fn check_invoicing(status: &adw::ActionRow, client_id: i64, script: &'static crate::invoicing::Script) {
    set_invoicing_status(status, &gettext("Checking…"), false);
    glib::spawn_future_local(clone!(
        #[weak]
        status,
        async move {
            let (text, error) = invoicing_status(client_id, script).await;
            set_invoicing_status(&status, &text, error);
        }
    ));
}

/// The status text, and whether it's an error rather than a missing value.
async fn invoicing_status(client_id: i64, script: &'static crate::invoicing::Script) -> (String, bool) {
    let client = TimeTrackerApplication::get().store().client(client_id).ok().flatten();
    let settings = crate::invoicing::parse_settings(client.and_then(|c| c.invoice_settings).as_deref());
    let missing = script
        .settings
        .iter()
        .find(|s| settings.get(s.key).is_none_or(|v| v.trim().is_empty()));
    if let Some(missing) = missing {
        return (gettext("Enter the {}").replace("{}", &missing.title.to_lowercase()), false);
    }
    let token = match crate::secrets::load_invoice_token(script.id, client_id).await {
        Ok(Some(token)) => token,
        Ok(None) => return (gettext("Enter the API token"), false),
        Err(err) => return (gettext("Could not read the keyring: {}").replace("{}", &err), true),
    };
    match gio::spawn_blocking(move || (script.check)(&token, &settings)).await {
        Ok(Ok(())) => (gettext("Ready · the API token is stored in your keyring"), false),
        Ok(Err(err)) => (err, true),
        Err(_) => (gettext("Checking failed unexpectedly"), true),
    }
}

fn fill_tasks(group: &adw::PreferencesGroup, section: &Section<adw::ActionRow>, client_id: i64, project_id: Option<i64>) {
    let tasks = TimeTrackerApplication::get()
        .store()
        .project_tasks(client_id, project_id)
        .unwrap_or_default();
    if !section.begin(group, format!("{tasks:?}")) {
        return;
    }
    let mut new_rows = Vec::new();
    for task in tasks {
        // Fibery tasks are done when they're finished in Fibery.
        let check = gtk::CheckButton::builder()
            .active(task.done)
            .valign(gtk::Align::Center)
            .sensitive(!task.is_remote)
            .tooltip_text(if task.is_remote { gettext("Done when finished in Fibery") } else { gettext("Done") })
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
    section.set_rows(new_rows);
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
