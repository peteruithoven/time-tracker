//! Connects a client to a Fibery workspace in two steps: the account (checked
//! when pressing Next), then the detected setup with a Connect button.

use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib::clone;
use gtk::{gio, glib};

use crate::application::TimeTrackerApplication;
use crate::fibery::{self, Mapping, schema::display_name};
use crate::format;
use crate::secrets;
use crate::store::NewAccount;

/// A successful check.
#[derive(Debug, Clone)]
pub struct Checked {
    workspace: String,
    token: String,
    user_id: String,
    user_name: String,
    mapping: Mapping,
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct ConnectDialog {
        pub client_id: std::cell::Cell<i64>,
        pub navigation: adw::NavigationView,
        pub workspace_row: adw::EntryRow,
        pub token_row: adw::PasswordEntryRow,
        pub error_row: adw::ActionRow,
        pub next_button: gtk::Button,
        pub next_stack: gtk::Stack,
        pub setup_group: adw::PreferencesGroup,
        pub connect_button: gtk::Button,
        pub checked: RefCell<Option<Checked>>,
        pub setup_rows: RefCell<Vec<adw::ActionRow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ConnectDialog {
        const NAME: &'static str = "ConnectDialog";
        type Type = super::ConnectDialog;
        type ParentType = adw::Dialog;
    }

    impl ObjectImpl for ConnectDialog {}
    impl WidgetImpl for ConnectDialog {}
    impl AdwDialogImpl for ConnectDialog {}
}

glib::wrapper! {
    pub struct ConnectDialog(ObjectSubclass<imp::ConnectDialog>)
        @extends gtk::Widget, adw::Dialog,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::ShortcutManager;
}

/// A page with a header bar for the dialog's navigation view.
fn page(title: &str, start: Option<&gtk::Widget>, end: &gtk::Widget, content: &adw::PreferencesPage) -> adw::NavigationPage {
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    if let Some(start) = start {
        header.pack_start(start);
    }
    header.pack_end(end);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(content));
    adw::NavigationPage::builder().title(title).child(&toolbar).build()
}

impl ConnectDialog {
    pub fn new(client_id: i64) -> Self {
        let dialog: Self = glib::Object::new();
        dialog.imp().client_id.set(client_id);
        dialog.setup();
        dialog
    }

    fn setup(&self) {
        let imp = self.imp();
        self.set_title(&gettext("Connect Fibery"));
        self.set_content_width(420);
        self.set_content_height(480);

        // Step 1: account
        let cancel = gtk::Button::with_label(&gettext("Cancel"));
        cancel.connect_clicked(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| {
                dialog.close();
            }
        ));
        // "Next", or a spinner while checking.
        imp.next_stack.add_named(&gtk::Label::new(Some(&gettext("Next"))), Some("label"));
        imp.next_stack.add_named(&adw::Spinner::new(), Some("spinner"));
        imp.next_button.set_child(Some(&imp.next_stack));
        imp.next_button.add_css_class("suggested-action");
        imp.next_button.set_sensitive(false);
        imp.next_button.connect_clicked(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.check()
        ));

        let account = adw::PreferencesGroup::builder()
            .description(gettext(
                "Create an API key in Fibery under Settings → API Keys. It is stored in your system keyring.",
            ))
            .build();
        imp.workspace_row.set_title(&gettext("Workspace (e.g. yourteam.fibery.io)"));
        imp.token_row.set_title(&gettext("API Key"));
        account.add(&imp.workspace_row);
        account.add(&imp.token_row);
        for row in [imp.workspace_row.upcast_ref::<adw::EntryRow>(), imp.token_row.upcast_ref()] {
            row.connect_changed(clone!(
                #[weak(rename_to = dialog)]
                self,
                move |_| dialog.update_next()
            ));
            row.connect_entry_activated(clone!(
                #[weak(rename_to = dialog)]
                self,
                move |_| dialog.check()
            ));
        }
        let errors = adw::PreferencesGroup::new();
        imp.error_row.set_use_markup(false);
        imp.error_row.set_visible(false);
        imp.error_row.add_css_class("error");
        imp.error_row.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
        errors.add(&imp.error_row);

        let account_page = adw::PreferencesPage::new();
        account_page.add(&account);
        account_page.add(&errors);

        // Step 2: detected setup
        imp.connect_button.set_label(&gettext("Connect"));
        imp.connect_button.add_css_class("suggested-action");
        imp.connect_button.connect_clicked(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.connect()
        ));
        imp.setup_group.set_title(&gettext("Detected Setup"));
        imp.setup_group
            .set_description(Some(&gettext("Your tasks and time logs in these databases will sync with the app.")));
        let setup_page = adw::PreferencesPage::new();
        setup_page.add(&imp.setup_group);

        imp.navigation.add(&page(&gettext("Connect Fibery"), Some(cancel.upcast_ref()), imp.next_button.upcast_ref(), &account_page));
        let setup = page(&gettext("Detected Setup"), None, imp.connect_button.upcast_ref(), &setup_page);
        setup.set_tag(Some("setup"));
        // The first page added is shown; the second waits for push_by_tag().
        imp.navigation.add(&setup);
        self.set_child(Some(&imp.navigation));
        self.set_focus(Some(&imp.workspace_row));
    }

    fn update_next(&self) {
        let imp = self.imp();
        imp.error_row.set_visible(false);
        let ready = fibery::normalize_workspace(&imp.workspace_row.text()).is_some()
            && !imp.token_row.text().trim().is_empty();
        imp.next_button.set_sensitive(ready);
    }

    fn set_checking(&self, checking: bool) {
        let imp = self.imp();
        imp.next_stack.set_visible_child_name(if checking { "spinner" } else { "label" });
        imp.next_button.set_sensitive(!checking);
        imp.workspace_row.set_sensitive(!checking);
        imp.token_row.set_sensitive(!checking);
    }

    fn show_error(&self, title: &str, detail: &str) {
        let imp = self.imp();
        imp.error_row.set_title(title);
        imp.error_row.set_subtitle(detail);
        imp.error_row.set_visible(true);
    }

    fn check(&self) {
        let imp = self.imp();
        if !imp.next_button.is_sensitive() {
            return;
        }
        let Some(workspace) = fibery::normalize_workspace(&imp.workspace_row.text()) else {
            return;
        };
        let token = imp.token_row.text().trim().to_string();
        imp.error_row.set_visible(false);
        self.set_checking(true);

        glib::spawn_future_local(clone!(
            #[weak(rename_to = dialog)]
            self,
            async move {
                let (ws, tk) = (workspace.clone(), token.clone());
                let result = gio::spawn_blocking(move || fibery::check(&ws, &tk)).await;
                dialog.set_checking(false);
                match result {
                    Ok(Ok((user_id, user_name, mapping))) => {
                        dialog.show_setup(&user_name, &mapping);
                        dialog.imp().checked.replace(Some(Checked { workspace, token, user_id, user_name, mapping }));
                        dialog.imp().navigation.push_by_tag("setup");
                    }
                    Ok(Err(fibery::Error::Unauthorized)) => dialog.show_error(
                        &gettext("The API key was not accepted"),
                        &gettext("Check that you copied the whole key and that it belongs to this workspace."),
                    ),
                    Ok(Err(err)) => dialog.show_error(&gettext("Could not connect"), &err.to_string()),
                    Err(_) => dialog.show_error(&gettext("Could not connect"), ""),
                }
            }
        ));
    }

    fn show_setup(&self, user_name: &str, m: &Mapping) {
        let imp = self.imp();
        for row in imp.setup_rows.take() {
            imp.setup_group.remove(&row);
        }
        let name = |field: &Option<String>| field.as_deref().map(display_name).unwrap_or("–").to_string();
        let both_times = m.log_start.is_some() && m.log_end.is_some();
        let mut rows = vec![
            (gettext("Account"), gettext("Signed in as {}").replace("{}", user_name), true),
            (gettext("Tasks"), display_name(&m.task_type).to_string(), true),
            (gettext("Projects"), name(&m.project_type), m.project_type.is_some()),
            (gettext("Time Logs"), display_name(&m.log_type).to_string(), true),
            (
                gettext("Start and End Times"),
                if both_times {
                    format!("{}, {}", name(&m.log_start), name(&m.log_end))
                } else {
                    gettext("Not found; only durations are synced")
                },
                both_times,
            ),
        ];
        if m.task_timing.is_some() {
            rows.push((gettext("Fibery Timer"), gettext("A running timer also shows in Fibery"), true));
        }
        let mut new_rows = Vec::new();
        for (title, subtitle, ok) in rows {
            let row = adw::ActionRow::builder().title(&title).subtitle(&subtitle).use_markup(false).build();
            let icon = gtk::Image::from_icon_name(if ok { "object-select-symbolic" } else { "dialog-warning-symbolic" });
            icon.add_css_class(if ok { "success" } else { "warning" });
            row.add_prefix(&icon);
            imp.setup_group.add(&row);
            new_rows.push(row);
        }
        imp.setup_rows.replace(new_rows);
    }

    fn connect(&self) {
        let imp = self.imp();
        let Some(checked) = imp.checked.borrow().clone() else {
            return;
        };
        let app = TimeTrackerApplication::get();
        let mapping = serde_json::to_string(&checked.mapping).unwrap_or_default();
        let account_id = match app.store().add_account(
            &NewAccount {
                client_id: imp.client_id.get(),
                service: "fibery",
                workspace: &checked.workspace,
                user_id: &checked.user_id,
                user_name: &checked.user_name,
                mapping: &mapping,
            },
            format::now(),
        ) {
            Ok(id) => id,
            Err(err) => {
                glib::g_critical!("time-tracker", "Could not save the connection: {err}");
                return;
            }
        };
        imp.connect_button.set_sensitive(false);

        glib::spawn_future_local(clone!(
            #[weak(rename_to = dialog)]
            self,
            async move {
                let app = TimeTrackerApplication::get();
                if let Err(err) = secrets::store_token(&checked.workspace, &checked.user_id, &checked.token).await {
                    // No usable keyring: keep it in the database; syncs retry the keyring.
                    glib::g_warning!("time-tracker", "Keyring unavailable, storing the API key locally: {err}");
                    let _ = app.store().set_account_token(account_id, Some(&checked.token));
                }
                app.data_changed();
                crate::sync::sync(true);
                dialog.close();
            }
        ));
    }
}
