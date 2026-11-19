//! Fibery settings of a connected client: which databases and fields hold
//! time logs (design J2), and which tasks the picker offers (J3).
//!
//! Both pages load the workspace schema (and workflow states) when they
//! open. Changes are saved right away and synced when the page closes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::{gio, glib};

use crate::application::TimeTrackerApplication;
use crate::fibery::schema::{Kind, Schema, display_name};
use crate::fibery::{Client, Mapping, State};
use crate::store::Account;

/// The mapping being edited, saved on every change.
struct Editor {
    account_id: i64,
    mapping: RefCell<Mapping>,
    changed: Cell<bool>,
}

impl Editor {
    fn new(account: &Account) -> Option<Rc<Self>> {
        let mapping = serde_json::from_str(&account.mapping).ok()?;
        Some(Rc::new(Self { account_id: account.id, mapping: RefCell::new(mapping), changed: Cell::new(false) }))
    }

    fn update(&self, change: impl FnOnce(&mut Mapping)) {
        change(&mut self.mapping.borrow_mut());
        let json = serde_json::to_string(&*self.mapping.borrow()).unwrap_or_default();
        let app = TimeTrackerApplication::get();
        if let Err(err) = app.store().set_account_mapping(self.account_id, &json) {
            glib::g_critical!("time-tracker", "Could not save the Fibery settings: {err}");
        }
        self.changed.set(true);
        app.data_changed();
    }
}

/// A subpage that shows a spinner while `load` runs against Fibery, then
/// what `fill` puts on the preferences page (or the error).
fn loading_page<T: Send + 'static>(
    dialog: &adw::PreferencesDialog,
    title: &str,
    account: &Account,
    editor: Rc<Editor>,
    load: impl FnOnce(&Client) -> crate::fibery::Result<T> + Send + 'static,
    fill: impl FnOnce(&adw::PreferencesPage, T) + 'static,
) {
    let stack = gtk::Stack::new();
    let spinner = adw::Spinner::builder().width_request(32).height_request(32).build();
    stack.add_named(&spinner, Some("loading"));
    let page = adw::PreferencesPage::new();
    stack.add_named(&page, Some("page"));
    let error = adw::StatusPage::builder()
        .icon_name("dialog-warning-symbolic")
        .title(gettext("Could Not Load the Fibery Setup"))
        .build();
    stack.add_named(&error, Some("error"));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&stack));
    let nav = adw::NavigationPage::builder().title(title).child(&toolbar).build();
    // Apply the changes with a sync once you're done here.
    nav.connect_hidden(move |_| {
        if editor.changed.replace(false) {
            crate::sync::sync(true);
        }
    });
    dialog.push_subpage(&nav);

    let account = account.clone();
    glib::spawn_future_local(async move {
        let Some(token) = crate::sync::token_for(&account).await else {
            error.set_description(Some(&gettext("The Fibery API key is missing. Enter it under New API Key on the client page.")));
            stack.set_visible_child_name("error");
            return;
        };
        let client = Client::new(&account.workspace, &token);
        match gio::spawn_blocking(move || load(&client)).await {
            Ok(Ok(data)) => {
                fill(&page, data);
                stack.set_visible_child_name("page");
            }
            Ok(Err(err)) => {
                error.set_description(Some(&err.to_string()));
                stack.set_visible_child_name("error");
            }
            Err(_) => stack.set_visible_child_name("error"),
        }
    });
}

/// A combo row to pick one of `values` (shown by their short names); with
/// `optional`, "None" comes first.
fn choice_row(
    title: &str,
    subtitle: Option<&str>,
    values: Vec<String>,
    selected: Option<&str>,
    optional: bool,
    on_change: impl Fn(Option<String>) + 'static,
) -> adw::ComboRow {
    let mut options: Vec<Option<String>> = Vec::new();
    if optional {
        options.push(None);
    }
    options.extend(values.into_iter().map(Some));
    // Keep a current value that's no longer offered, so it isn't lost.
    if let Some(selected) = selected
        && !options.iter().any(|o| o.as_deref() == Some(selected))
    {
        options.push(Some(selected.to_string()));
    }
    let names = gtk::StringList::new(&[]);
    for option in &options {
        names.append(option.as_deref().map_or(&gettext("None"), display_name));
    }
    let index = options.iter().position(|o| o.as_deref() == selected).unwrap_or(0);
    let row = adw::ComboRow::builder().title(title).model(&names).selected(index as u32).build();
    if let Some(subtitle) = subtitle {
        row.set_subtitle(subtitle);
    }
    row.connect_selected_notify(move |row| {
        if let Some(option) = options.get(row.selected() as usize) {
            on_change(option.clone());
        }
    });
    row
}

/// Time Logs (J2): the databases and fields used for time logs.
pub fn show_time_logs(dialog: &adw::PreferencesDialog, account: &Account) {
    let Some(editor) = Editor::new(account) else {
        return;
    };
    let dialog_ref = dialog.clone();
    let account_ref = account.clone();
    loading_page(
        dialog,
        &gettext("Time Logs"),
        account,
        editor.clone(),
        |client| client.schema().map(|s| Schema::parse(&s)),
        move |page, schema| {
            let view = TimeLogsView {
                page: page.downgrade(),
                groups: RefCell::default(),
                schema,
                editor,
                dialog: dialog_ref.downgrade(),
                account: account_ref,
            };
            fill_time_logs(&Rc::new(view));
        },
    );
}

/// State of the Time Logs page. Its rows' handlers hold it, so it only
/// refers to widgets weakly.
struct TimeLogsView {
    page: glib::WeakRef<adw::PreferencesPage>,
    groups: RefCell<Vec<glib::WeakRef<adw::PreferencesGroup>>>,
    schema: Schema,
    editor: Rc<Editor>,
    dialog: glib::WeakRef<adw::PreferencesDialog>,
    account: Account,
}

fn fill_time_logs(view: &Rc<TimeLogsView>) {
    let (Some(page), Some(dialog)) = (view.page.upgrade(), view.dialog.upgrade()) else {
        return;
    };
    let (page, dialog) = (&page, &dialog);
    let (schema, editor, account) = (&view.schema, &view.editor, &view.account);
    // Changing a database changes which fields there are: rebuild after
    // the combo row's popover closed.
    let rebuild: Rc<dyn Fn()> = {
        let view = view.clone();
        Rc::new(move || {
            let view = view.clone();
            glib::idle_add_local_once(move || fill_time_logs(&view));
        })
    };
    for group in view.groups.take() {
        if let Some(group) = group.upgrade() {
            page.remove(&group);
        }
    }
    let add = |group: &adw::PreferencesGroup| {
        page.add(group);
        view.groups.borrow_mut().push(group.downgrade());
    };

    let m = editor.mapping.borrow().clone();
    let database = adw::PreferencesGroup::new();
    let (e, r) = (editor.clone(), rebuild.clone());
    let v = view.clone();
    database.add(&choice_row(&gettext("Database"), None, schema.log_types(), Some(&m.log_type), false, move |value| {
        let Some(log_type) = value else { return };
        if let Ok(detected) = v.schema.detect_for_log(&log_type) {
            e.update(|m| {
                // Keep the task choices of Show Tasks when the tasks stay.
                let keep = detected.task_type == m.task_type;
                let previous = std::mem::replace(m, detected);
                m.show_everyone = previous.show_everyone;
                m.keep_done_days = previous.keep_done_days;
                if keep {
                    m.task_states = previous.task_states;
                    m.hidden_project_states = previous.hidden_project_states;
                }
            });
            r();
        }
    }));
    add(&database);

    let fields = adw::PreferencesGroup::builder()
        .title(gettext("Field Mapping"))
        .description(gettext("Detected from the Fibery schema. Only fields of the right type are offered."))
        .build();
    let log_type = m.log_type.clone();
    let required = |title: &str, subtitle: Option<&str>, kind: Kind, current: &str, set: fn(&mut Mapping, String)| {
        let e = editor.clone();
        choice_row(title, subtitle, schema.fields(&log_type, kind), Some(current), false, move |value| {
            if let Some(value) = value {
                e.update(|m| set(m, value));
            }
        })
    };
    let optional = |title: &str, subtitle: Option<&str>, kind: Kind, current: Option<&str>, set: fn(&mut Mapping, Option<String>)| {
        let e = editor.clone();
        choice_row(title, subtitle, schema.fields(&log_type, kind), current, true, move |value| {
            e.update(|m| set(m, value));
        })
    };

    let (e, r, v) = (editor.clone(), rebuild.clone(), view.clone());
    fields.add(&choice_row(
        &gettext("Task"),
        Some(&gettext("Links to {}").replace("{}", display_name(&m.task_type))),
        schema.fields(&log_type, Kind::Relation),
        Some(&m.log_task),
        false,
        move |value| {
            if let Some(field) = value {
                e.update(|m| v.schema.set_log_task(m, &field));
                r();
            }
        },
    ));
    fields.add(&required(&gettext("Hours"), Some(&gettext("Number, stored as hours")), Kind::Number, &m.log_hours, |m, v| m.log_hours = v));
    fields.add(&required(&gettext("Date"), None, Kind::Date, &m.log_date, |m, v| m.log_date = v));
    fields.add(&optional(&gettext("User"), None, Kind::User, m.log_user.as_deref(), |m, v| m.log_user = v));
    fields.add(&optional(&gettext("Note"), None, Kind::Text, m.log_note.as_deref(), |m, v| m.log_note = v));
    let date_time = gettext("Optional · date and time");
    fields.add(&optional(&gettext("Start Time"), Some(&date_time), Kind::DateTime, m.log_start.as_deref(), |m, v| m.log_start = v));
    fields.add(&optional(&gettext("End Time"), Some(&date_time), Kind::DateTime, m.log_end.as_deref(), |m, v| m.log_end = v));
    add(&fields);

    let projects = adw::PreferencesGroup::builder().title(gettext("Projects")).build();
    let (e, r, v) = (editor.clone(), rebuild.clone(), view.clone());
    projects.add(&choice_row(
        &gettext("Project Link"),
        Some(&gettext("The task's link to its project")),
        schema.fields(&m.task_type, Kind::Relation),
        m.task_project.as_deref(),
        true,
        move |value| {
            e.update(|m| v.schema.set_task_project(m, value.as_deref()));
            r();
        },
    ));
    if let Some(project_type) = m.project_type.clone() {
        let hidden = adw::ActionRow::builder()
            .title(gettext("Hidden Project States"))
            .subtitle(match &m.hidden_project_states {
                Some(states) if states.is_empty() => gettext("None"),
                Some(states) => states.join(", "),
                None => gettext("Finished states"),
            })
            .use_markup(false)
            .activatable(true)
            .build();
        hidden.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let state_type = schema.state_type(&project_type);
        let (e, d, a, r) = (editor.clone(), dialog.clone(), account.clone(), rebuild.clone());
        hidden.set_sensitive(state_type.is_some());
        hidden.connect_activated(move |_| {
            if let Some(state_type) = &state_type {
                show_project_states(&d, &a, &e, state_type.clone(), r.clone());
            }
        });
        projects.add(&hidden);
    }
    add(&projects);
}

/// Which project states are archived in the app.
fn show_project_states(
    dialog: &adw::PreferencesDialog,
    account: &Account,
    editor: &Rc<Editor>,
    state_type: String,
    on_done: Rc<dyn Fn()>,
) {
    let e = editor.clone();
    loading_page(
        dialog,
        &gettext("Hidden Project States"),
        account,
        editor.clone(),
        move |client| client.states(&state_type),
        move |page, states| {
            let group = adw::PreferencesGroup::builder()
                .description(gettext("Projects in these states are archived in the app and their tasks leave the task picker."))
                .build();
            state_checks(&group, &states, &e, |m| m.hidden_project_states.clone(), |s| s.finished, |m, v| {
                m.hidden_project_states = v;
            });
            page.add(&group);
            let on_done = on_done.clone();
            // Refresh the summary on the Time Logs page.
            page.connect_unmap(move |_| on_done());
        },
    );
}

/// A check row per state; `current` returns the chosen states (`None`:
/// the default, where `default_on` decides).
fn state_checks(
    group: &adw::PreferencesGroup,
    states: &[State],
    editor: &Rc<Editor>,
    current: fn(&Mapping) -> Option<Vec<String>>,
    default_on: fn(&State) -> bool,
    set: fn(&mut Mapping, Option<Vec<String>>),
) {
    let chosen = current(&editor.mapping.borrow());
    let checks: Rc<RefCell<Vec<(String, gtk::CheckButton)>>> = Rc::default();
    for state in states {
        let on = chosen.as_ref().map_or(default_on(state), |c| c.contains(&state.name));
        let check = gtk::CheckButton::builder().active(on).valign(gtk::Align::Center).build();
        let row = adw::ActionRow::builder()
            .title(&state.name)
            .use_markup(false)
            .activatable_widget(&check)
            .build();
        row.add_prefix(&check);
        group.add(&row);
        checks.borrow_mut().push((state.name.clone(), check.clone()));
        let (e, checks) = (editor.clone(), checks.clone());
        check.connect_toggled(move |_| {
            let names: Vec<String> = checks
                .borrow()
                .iter()
                .filter(|(_, c)| c.is_active())
                .map(|(n, _)| n.clone())
                .collect();
            e.update(|m| set(m, Some(names)));
        });
    }
}

/// Show Tasks (J3): which Fibery tasks the picker offers.
pub fn show_tasks(dialog: &adw::PreferencesDialog, account: &Account) {
    let Some(editor) = Editor::new(account) else {
        return;
    };
    let (task_type, has_states) = {
        let m = editor.mapping.borrow();
        (m.task_type.clone(), m.task_state.is_some())
    };
    let e = editor.clone();
    loading_page(
        dialog,
        &gettext("Show Tasks"),
        account,
        editor.clone(),
        move |client| {
            if !has_states {
                return Ok(Vec::new());
            }
            let schema = Schema::parse(&client.schema()?);
            match schema.state_type(&task_type) {
                Some(state_type) => client.states(&state_type),
                None => Ok(Vec::new()),
            }
        },
        move |page, states| fill_show_tasks(page, &e, &states),
    );
}

fn fill_show_tasks(page: &adw::PreferencesPage, editor: &Rc<Editor>, states: &[State]) {
    let m = editor.mapping.borrow().clone();

    let assignee = adw::PreferencesGroup::builder().title(gettext("Assignee")).build();
    let mine = gtk::CheckButton::builder().active(!m.show_everyone).valign(gtk::Align::Center).build();
    let everyone = gtk::CheckButton::builder()
        .active(m.show_everyone)
        .group(&mine)
        .valign(gtk::Align::Center)
        .build();
    let mine_row = adw::ActionRow::builder()
        .title(gettext("Assigned to Me"))
        .activatable_widget(&mine)
        .sensitive(m.task_assignees.is_some())
        .build();
    mine_row.add_prefix(&mine);
    let everyone_row = adw::ActionRow::builder()
        .title(gettext("Everyone"))
        .subtitle(gettext("All tasks in the chosen states"))
        .activatable_widget(&everyone)
        .build();
    everyone_row.add_prefix(&everyone);
    let e = editor.clone();
    everyone.connect_toggled(move |check| {
        let on = check.is_active();
        e.update(|m| m.show_everyone = on);
    });
    assignee.add(&mine_row);
    assignee.add(&everyone_row);
    page.add(&assignee);

    if !states.is_empty() {
        let group = adw::PreferencesGroup::builder().title(gettext("Task States")).build();
        state_checks(&group, states, editor, |m| m.task_states.clone(), |s| !s.finished, |m, v| m.task_states = v);
        page.add(&group);
    }

    let done = adw::PreferencesGroup::builder()
        .title(gettext("Recently Done"))
        .description(gettext("Tasks you're tracking time on, or tracked before, are never hidden from the timer and Review."))
        .build();
    const DAYS: [u32; 6] = [0, 1, 3, 7, 14, 30];
    let names = gtk::StringList::new(&[]);
    for days in DAYS {
        names.append(&if days == 0 {
            gettext("Off")
        } else {
            ngettext("{} Day", "{} Days", days).replace("{}", &days.to_string())
        });
    }
    let keep = adw::ComboRow::builder()
        .title(gettext("Keep Done Tasks For"))
        .subtitle(gettext("Handy for logging time after finishing"))
        .model(&names)
        .selected(DAYS.iter().position(|d| *d == m.keep_done_days).unwrap_or(0) as u32)
        .build();
    let e = editor.clone();
    keep.connect_selected_notify(move |row| {
        let days = DAYS.get(row.selected() as usize).copied().unwrap_or(0);
        e.update(|m| m.keep_done_days = days);
    });
    done.add(&keep);
    page.add(&done);
}

/// One-line summary of what the picker offers, e.g. "Assigned to me ·
/// Open, In Progress".
pub fn show_tasks_summary(m: &Mapping) -> String {
    let who = if m.show_everyone || m.task_assignees.is_none() {
        gettext("Everyone")
    } else {
        gettext("Assigned to me")
    };
    let states = match &m.task_states {
        Some(states) if states.is_empty() => gettext("no states"),
        Some(states) => states.join(", "),
        None => gettext("unfinished"),
    };
    format!("{who} · {states}")
}
