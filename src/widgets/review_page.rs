//! Tracked time per month as a client › project › task tree with week columns.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::glib::clone;

use crate::application::TimeTrackerApplication;
use crate::format;
use crate::report::{self, Report, Totals, Week};
use crate::store::Entry;
use crate::widgets::{EntryDialog, MonthPicker, project_color_dot, set_source_badge};

const WEEK_WIDTH: i32 = 60;
const TOTAL_WIDTH: i32 = 72;

/// Which rows a cell covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Client(i64),
    Project(i64, Option<i64>),
    Task(i64),
    All,
}

impl Scope {
    fn matches(&self, entry: &Entry) -> bool {
        match *self {
            Scope::Client(id) => entry.client_id == id,
            Scope::Project(client, project) => entry.client_id == client && entry.project_id == project,
            Scope::Task(id) => entry.task_id == id,
            Scope::All => true,
        }
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate, glib::Properties)]
    #[properties(wrapper_type = super::ReviewPage)]
    #[template(resource = "/nl/peteruithoven/TimeTracker/review_page.ui")]
    pub struct ReviewPage {
        #[template_child]
        pub previous_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub next_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub month_picker: TemplateChild<MonthPicker>,
        #[template_child]
        pub month_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub filter_slot: TemplateChild<gtk::Box>,
        #[template_child]
        pub compact_filter_slot: TemplateChild<gtk::Box>,
        #[template_child]
        pub add_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub add_button_content: TemplateChild<adw::ButtonContent>,
        #[template_child]
        pub table: TemplateChild<gtk::Box>,
        #[template_child]
        pub empty_page: TemplateChild<adw::StatusPage>,

        /// Narrow layout: only the total column, filter on its own row.
        #[property(get, set = Self::set_compact)]
        pub compact: Cell<bool>,

        pub client_filter: gtk::DropDown,
        /// Client id per drop-down item after "All Clients".
        pub filter_clients: RefCell<Vec<i64>>,
        pub month: Cell<(i32, u32)>,
        pub entries: RefCell<Vec<Entry>>,
        pub report: RefCell<Option<Report>>,
        /// Expanded clients and projects; projects start collapsed.
        pub expanded: RefCell<HashSet<Scope>>,
        pub collapsed: RefCell<HashSet<Scope>>,
        pub updating_filter: Cell<bool>,
    }

    impl ReviewPage {
        fn set_compact(&self, compact: bool) {
            self.compact.set(compact);
            self.obj().apply_compact();
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ReviewPage {
        const NAME: &'static str = "ReviewPage";
        type Type = super::ReviewPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            MonthPicker::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for ReviewPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }

    impl WidgetImpl for ReviewPage {}
    impl BinImpl for ReviewPage {}
}

glib::wrapper! {
    pub struct ReviewPage(ObjectSubclass<imp::ReviewPage>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ReviewPage {
    fn setup(&self) {
        let imp = self.imp();
        if let Some((y, m, _)) = format::date_parts(&format::today()) {
            imp.month.set((y, m));
        }

        imp.client_filter.set_tooltip_text(Some(&gettext("Filter by Client")));
        imp.filter_slot.append(&imp.client_filter);
        imp.client_filter.connect_selected_notify(clone!(
            #[weak(rename_to = page)]
            self,
            move |_| {
                if !page.imp().updating_filter.get() {
                    page.rebuild_table();
                }
            }
        ));

        imp.previous_button.connect_clicked(clone!(
            #[weak(rename_to = page)]
            self,
            move |_| page.step_month(-1)
        ));
        imp.next_button.connect_clicked(clone!(
            #[weak(rename_to = page)]
            self,
            move |_| page.step_month(1)
        ));
        imp.month_picker.connect_month_chosen(clone!(
            #[weak(rename_to = page)]
            self,
            move |_, year, month| page.set_month(year, month)
        ));
        imp.add_button.connect_clicked(clone!(
            #[weak(rename_to = page)]
            self,
            move |_| page.add_time(None, &page.default_date(None))
        ));
        TimeTrackerApplication::get().connect_data_changed(clone!(
            #[weak(rename_to = page)]
            self,
            move |_| page.reload()
        ));
        self.reload();
    }

    fn apply_compact(&self) {
        let imp = self.imp();
        let compact = imp.compact.get();
        let filter = &imp.client_filter;
        let target = if compact { &*imp.compact_filter_slot } else { &*imp.filter_slot };
        if filter.parent().as_ref() != Some(target.upcast_ref()) {
            if let Some(parent) = filter.parent().and_downcast::<gtk::Box>() {
                parent.remove(filter);
            }
            target.append(filter);
        }
        imp.compact_filter_slot.set_visible(compact);
        imp.add_button_content.set_label(&if compact { String::new() } else { gettext("Add Time") });
        self.rebuild_table();
    }

    fn step_month(&self, delta: i32) {
        let (year, month) = self.imp().month.get();
        let index = year * 12 + month as i32 - 1 + delta;
        self.set_month(index.div_euclid(12), index.rem_euclid(12) as u32 + 1);
    }

    fn set_month(&self, year: i32, month: u32) {
        self.imp().month.set((year, month));
        self.reload();
    }

    /// Reloads the month's entries from the store.
    pub fn reload(&self) {
        let imp = self.imp();
        let (year, month) = imp.month.get();
        imp.month_label.set_label(&format::month_label(year, month));
        imp.month_picker.set_selected(year, month);
        let (this_year, this_month, _) = format::date_parts(&format::today()).unwrap_or((year, month, 1));
        imp.next_button.set_sensitive((year, month) < (this_year, this_month));

        let first = format::date_string(year, month, 1);
        let last = format::date_string(year, month, report::days_in_month(year, month));
        let entries = TimeTrackerApplication::get()
            .store()
            .entries_between(&first, &last)
            .unwrap_or_else(|err| {
                glib::g_critical!("time-tracker", "Could not load entries: {err}");
                Vec::new()
            });
        imp.entries.replace(entries);
        self.rebuild_filter();
        self.rebuild_table();
    }

    fn rebuild_filter(&self) {
        let imp = self.imp();
        let selected_client = self.filtered_client();

        let mut clients: Vec<(i64, String)> = Vec::new();
        for entry in imp.entries.borrow().iter() {
            if !clients.iter().any(|(id, _)| *id == entry.client_id) {
                clients.push((entry.client_id, entry.client_name.clone()));
            }
        }
        clients.sort_by_key(|(_, name)| name.to_lowercase());

        let names = gtk::StringList::new(&[&gettext("All Clients")]);
        for (_, name) in &clients {
            names.append(name);
        }
        let position = selected_client
            .and_then(|id| clients.iter().position(|(c, _)| *c == id))
            .map_or(0, |i| i as u32 + 1);

        imp.updating_filter.set(true);
        imp.client_filter.set_model(Some(&names));
        imp.client_filter.set_selected(position);
        imp.updating_filter.set(false);
        imp.filter_clients.replace(clients.into_iter().map(|(id, _)| id).collect());
    }

    fn filtered_client(&self) -> Option<i64> {
        let imp = self.imp();
        let selected = imp.client_filter.selected();
        if selected == 0 || selected == gtk::INVALID_LIST_POSITION {
            return None;
        }
        imp.filter_clients.borrow().get(selected as usize - 1).copied()
    }

    fn rebuild_table(&self) {
        let imp = self.imp();
        let (year, month) = imp.month.get();
        let weeks = report::weeks_of_month(year, month);
        let filter = self.filtered_client();
        let entries: Vec<Entry> = imp
            .entries
            .borrow()
            .iter()
            .filter(|e| filter.is_none_or(|id| e.client_id == id))
            .cloned()
            .collect();
        let report = report::build(&entries, &weeks, format::now());

        let table = &*imp.table;
        while let Some(child) = table.first_child() {
            table.remove(&child);
        }
        let empty = report.clients.is_empty();
        table.set_visible(!empty);
        imp.empty_page.set_visible(empty);
        if !empty {
            table.append(&self.header_row(&weeks));
            for client in &report.clients {
                let scope = Scope::Client(client.client_id);
                let open = !imp.collapsed.borrow().contains(&scope);
                let name = self.name_cell(0, Some((scope, open)), &client.name, None, Some(client.is_remote));
                table.append(&self.data_row("client", name, &client.totals, scope, &weeks));
                if !open {
                    continue;
                }
                for project in &client.projects {
                    let scope = Scope::Project(client.client_id, project.project_id);
                    let open = imp.expanded.borrow().contains(&scope);
                    let label = project.name.clone().unwrap_or_else(|| gettext("No Project"));
                    let color = project.color.as_deref().or(Some("slate"));
                    let name = self.name_cell(1, Some((scope, open)), &label, Some(color), None);
                    if project.project_id.is_none() {
                        name.add_css_class("no-project");
                    }
                    table.append(&self.data_row("project", name, &project.totals, scope, &weeks));
                    if !open {
                        continue;
                    }
                    for task in &project.tasks {
                        let scope = Scope::Task(task.task_id);
                        let name = self.name_cell(2, None, &task.name, None, None);
                        table.append(&self.data_row("task", name, &task.totals, scope, &weeks));
                    }
                }
            }
            let total = gtk::Label::builder()
                .label(gettext("Total"))
                .xalign(0.0)
                .hexpand(true)
                .build();
            let name = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            name.append(&total);
            table.append(&self.data_row("total", name, &report.totals, Scope::All, &weeks));
        }
        imp.report.replace(Some(report));
    }

    fn header_row(&self, weeks: &[Week]) -> gtk::Box {
        let row = gtk::Box::builder().css_classes(["review-row", "header"]).build();
        let name = gtk::Label::builder()
            .label(gettext("Client · Project · Task"))
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        row.append(&name);
        if !self.imp().compact.get() {
            for week in weeks {
                row.append(&header_cell(&week.label(), WEEK_WIDTH));
            }
        }
        row.append(&header_cell(&gettext("Total"), TOTAL_WIDTH));
        row
    }

    /// The first cell of a row: indent, optional expander, dot, label, badge.
    fn name_cell(
        &self,
        level: i32,
        expander: Option<(Scope, bool)>,
        label: &str,
        color: Option<Option<&str>>,
        remote: Option<bool>,
    ) -> gtk::Box {
        let cell = gtk::Box::builder()
            .spacing(6)
            .hexpand(true)
            .margin_start(level * 18 + if expander.is_none() { 30 } else { 0 })
            .build();
        if let Some((scope, open)) = expander {
            let button = gtk::Button::builder()
                .icon_name(if open { "pan-down-symbolic" } else { "pan-end-symbolic" })
                .tooltip_text(if open { gettext("Collapse") } else { gettext("Expand") })
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular", "expander-button"])
                .build();
            button.connect_clicked(clone!(
                #[weak(rename_to = page)]
                self,
                move |_| page.toggle(scope)
            ));
            cell.append(&button);
        }
        if let Some(color) = color {
            cell.append(&project_color_dot(color));
        }
        cell.append(
            &gtk::Label::builder()
                .label(label)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        if let Some(is_remote) = remote {
            let badge = gtk::Label::builder().css_classes(["source-badge"]).valign(gtk::Align::Center).build();
            set_source_badge(&badge, is_remote);
            cell.append(&badge);
        }
        cell
    }

    fn data_row(&self, kind: &str, name: gtk::Box, totals: &Totals, scope: Scope, weeks: &[Week]) -> gtk::Box {
        let row = gtk::Box::builder().css_classes(["review-row", kind]).build();
        row.append(&name);
        if !self.imp().compact.get() {
            for (index, secs) in totals.weeks.iter().enumerate() {
                let week = weeks.get(index).copied();
                row.append(&self.value_cell(*secs, WEEK_WIDTH, scope, week));
            }
        }
        row.append(&self.value_cell(totals.total, TOTAL_WIDTH, scope, None));
        row
    }

    /// A clickable duration; opens the entries behind it.
    fn value_cell(&self, secs: i64, width: i32, scope: Scope, week: Option<Week>) -> gtk::Button {
        let label = gtk::Label::builder()
            .label(if secs > 0 { format::hm(secs) } else { "–".to_string() })
            .xalign(1.0)
            .css_classes(["numeric"])
            .build();
        let button = gtk::Button::builder()
            .child(&label)
            .width_request(width)
            .css_classes(["flat", "review-cell"])
            .build();
        button.connect_clicked(clone!(
            #[weak(rename_to = page)]
            self,
            move |cell| page.show_entries(cell, scope, week)
        ));
        button
    }

    fn toggle(&self, scope: Scope) {
        let imp = self.imp();
        // Clients start expanded, projects collapsed.
        let set = if matches!(scope, Scope::Client(_)) { &imp.collapsed } else { &imp.expanded };
        let mut set = set.borrow_mut();
        if !set.remove(&scope) {
            set.insert(scope);
        }
        drop(set);
        self.rebuild_table();
    }

    /// A sensible date for new time: today if it's in the range, else its first day.
    fn default_date(&self, week: Option<Week>) -> String {
        let (year, month) = self.imp().month.get();
        let today = format::today();
        let (first, last) = match week {
            Some(week) => (week.first, week.last),
            None => (1, report::days_in_month(year, month)),
        };
        let first = format::date_string(year, month, first);
        let last = format::date_string(year, month, last);
        if (first.as_str()..=last.as_str()).contains(&today.as_str()) {
            today
        } else {
            first
        }
    }

    fn add_time(&self, task_id: Option<i64>, date: &str) {
        EntryDialog::new_add(task_id, date).present(Some(self));
    }

    fn show_entries(&self, cell: &gtk::Button, scope: Scope, week: Option<Week>) {
        let imp = self.imp();
        let (year, month) = imp.month.get();
        let filter = self.filtered_client();
        let entries: Vec<Entry> = imp
            .entries
            .borrow()
            .iter()
            .filter(|e| filter.is_none_or(|id| e.client_id == id))
            .filter(|e| scope.matches(e))
            .filter(|e| {
                let day = e.date.get(8..10).and_then(|d| d.parse().ok()).unwrap_or(0);
                week.is_none_or(|w| w.contains(day))
            })
            .cloned()
            .collect();

        let title = match scope {
            Scope::Task(_) | Scope::Project(..) | Scope::Client(_) => entries
                .first()
                .map(|e| match scope {
                    Scope::Task(_) => e.task_name.clone(),
                    Scope::Project(..) => e.project_name.clone().unwrap_or_else(|| gettext("No Project")),
                    _ => e.client_name.clone(),
                })
                .unwrap_or_default(),
            Scope::All => gettext("All Entries"),
        };
        let period = match week {
            Some(w) => format!("{} {}", w.label(), format::short_month(year, month)),
            None => format::month_label(year, month),
        };
        let task_id = match scope {
            Scope::Task(id) => Some(id),
            _ => None,
        };
        let popover = entries_popover(self, &title, &period, &entries, matches!(scope, Scope::Task(_)));
        let add = gtk::Button::builder()
            .child(
                &adw::ButtonContent::builder()
                    .icon_name("list-add-symbolic")
                    .label(gettext("Add Entry"))
                    .build(),
            )
            .halign(gtk::Align::End)
            .build();
        let date = self.default_date(week);
        add.connect_clicked(clone!(
            #[weak(rename_to = page)]
            self,
            #[weak]
            popover,
            move |_| {
                popover.popdown();
                page.add_time(task_id, &date);
            }
        ));
        if let Some(content) = popover.child().and_downcast::<gtk::Box>() {
            content.append(&add);
        }

        popover.set_parent(cell);
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
    }
}

/// Column title, padded like the value buttons below it so they line up.
fn header_cell(label: &str, width: i32) -> gtk::Label {
    gtk::Label::builder()
        .label(label)
        .xalign(1.0)
        .width_request(width)
        .css_classes(["numeric", "review-header-cell"])
        .build()
}

/// Popover listing entries grouped per day; clicking one opens the editor.
fn entries_popover(
    page: &ReviewPage,
    title: &str,
    period: &str,
    entries: &[Entry],
    is_task: bool,
) -> gtk::Popover {
    let now = format::now();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .width_request(320)
        .css_classes(["entries-popover"])
        .build();

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let titles = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).build();
    titles.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["heading"])
            .build(),
    );
    titles.append(
        &gtk::Label::builder()
            .label(period)
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    header.append(&titles);
    let total: i64 = entries.iter().map(|e| e.duration(now)).sum();
    header.append(
        &gtk::Label::builder()
            .label(format::hm(total))
            .valign(gtk::Align::Start)
            .css_classes(["heading", "numeric"])
            .build(),
    );
    content.append(&header);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let mut day = String::new();
    for entry in entries {
        if entry.date != day {
            day = entry.date.clone();
            let day_total: i64 = entries.iter().filter(|e| e.date == day).map(|e| e.duration(now)).sum();
            let day_row = gtk::Box::builder().margin_top(10).margin_bottom(2).build();
            day_row.append(
                &gtk::Label::builder()
                    .label(format::long_date(&day))
                    .xalign(0.0)
                    .hexpand(true)
                    .css_classes(["caption-heading", "dim-label"])
                    .build(),
            );
            day_row.append(
                &gtk::Label::builder()
                    .label(format::hm(day_total))
                    .css_classes(["caption-heading", "dim-label", "numeric"])
                    .build(),
            );
            list.append(&day_row);
        }
        list.append(&entry_button(page, entry, is_task, now));
    }
    if entries.is_empty() {
        list.append(
            &gtk::Label::builder()
                .label(gettext("No entries"))
                .margin_top(12)
                .margin_bottom(12)
                .css_classes(["dim-label"])
                .build(),
        );
    }
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(360)
        .child(&list)
        .build();
    content.append(&scrolled);

    gtk::Popover::builder().child(&content).build()
}

fn entry_button(page: &ReviewPage, entry: &Entry, is_task: bool, now: i64) -> gtk::Button {
    let times = match (entry.start_at, entry.end_at) {
        (Some(start), Some(end)) => format!("{} – {}", format::clock_time(start), format::clock_time(end)),
        (Some(start), None) => format!("{} – {}", format::clock_time(start), gettext("now")),
        _ => gettext("Manual"),
    };
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let time_label = gtk::Label::builder()
        .label(&times)
        .xalign(0.0)
        .width_chars(13)
        .css_classes(["numeric"])
        .build();
    if entry.start_at.is_none() {
        time_label.add_css_class("dim-label");
    }
    row.append(&time_label);

    // For project/client cells, say which task; otherwise show the note.
    let detail = if is_task {
        entry.note.clone()
    } else if entry.note.is_empty() {
        entry.task_name.clone()
    } else {
        format!("{} · {}", entry.task_name, entry.note)
    };
    let detail_label = gtk::Label::builder()
        .label(if detail.is_empty() { gettext("No note") } else { detail.clone() })
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    if detail.is_empty() {
        detail_label.add_css_class("dim-label");
    }
    row.append(&detail_label);

    let duration = gtk::Label::builder()
        .label(format::hm(entry.duration(now)))
        .css_classes(["numeric"])
        .build();
    if entry.is_running() {
        duration.add_css_class("running-duration");
    }
    row.append(&duration);
    row.append(&gtk::Image::from_icon_name("go-next-symbolic"));

    let button = gtk::Button::builder()
        .child(&row)
        .tooltip_text(gettext("Edit Entry"))
        .css_classes(["flat", "entry-button"])
        .build();
    let entry = entry.clone();
    button.connect_clicked(clone!(
        #[weak]
        page,
        move |button| {
            if let Some(popover) = button.ancestor(gtk::Popover::static_type()).and_downcast::<gtk::Popover>() {
                popover.popdown();
            }
            EntryDialog::new_edit(entry.clone()).present(Some(&page));
        }
    ));
    button
}
