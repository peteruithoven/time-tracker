//! Reviewing a held sync (design P and P2): what Fibery would remove,
//! change and add here, with Keep Paused and Apply Changes.

use std::rc::Rc;

use adw::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::{glib, pango};

use crate::format;
use crate::store::{Account, Change, EntryView};

/// Rows shown per section before "Show N More".
const FIRST_ROWS: usize = 3;
/// Rows added per "Show N More", so long lists don't block the app.
const MORE_ROWS: usize = 100;

pub fn present(parent: &impl IsA<gtk::Widget>, account: &Account) {
    let Some(impact) = account.held() else {
        return;
    };
    let builder = gtk::Builder::from_resource("/nl/peteruithoven/TimeTracker/sync_review.ui");
    let object = |name: &str| builder.object::<glib::Object>(name).unwrap_or_else(|| panic!("{name} in sync_review.ui"));
    let dialog: adw::Dialog = object("dialog").downcast().expect("dialog");
    let intro: gtk::Label = object("intro").downcast().expect("label");
    let callout: gtk::Box = object("callout").downcast().expect("box");
    let time_logs_list: gtk::ListBox = object("time_logs_list").downcast().expect("list");
    let time_logs_row: adw::ActionRow = object("time_logs_row").downcast().expect("row");
    let sections: gtk::Box = object("sections").downcast().expect("box");
    let keep: gtk::Button = object("keep_button").downcast().expect("button");
    let apply: gtk::Button = object("apply_button").downcast().expect("button");

    let all_missing = impact.all_missing();
    intro.set_label(
        &if all_missing {
            gettext("Fibery returned none of your time logs for {} from the last two months. Syncing is paused until you choose; nothing has changed in this app yet.")
        } else {
            gettext("Fibery returned different time logs for {} than at the last sync. Syncing is paused until you choose; nothing has changed in this app yet.")
        }
        .replace("{}", &impact.client),
    );
    callout.set_visible(all_missing);
    time_logs_list.set_visible(all_missing);
    if all_missing {
        let n = impact.removed.len();
        apply.set_label(&ngettext("Remove {} Entry", "Remove {} Entries", n as u32).replace("{}", &n.to_string()));
    }

    let removed_secs: i64 = impact.removed.iter().map(|e| e.secs).sum();
    let changed_secs: i64 = impact.changed.iter().map(|c| c.after.secs - c.before.secs).sum();
    let added_secs: i64 = impact.added.iter().map(|e| e.secs).sum();
    let removed = Rc::new(impact.removed);
    let changed = Rc::new(impact.changed);
    let added = Rc::new(impact.added);
    if !removed.is_empty() {
        let rows = removed.clone();
        sections.append(&section(
            &ngettext("Removed · {} entry", "Removed · {} entries", removed.len() as u32),
            removed.len(),
            &format::hm(removed_secs),
            move |i| entry_row(&rows[i], true),
        ));
    }
    if !changed.is_empty() {
        let rows = changed.clone();
        sections.append(&section(
            &ngettext("Changed · {} entry", "Changed · {} entries", changed.len() as u32),
            changed.len(),
            &signed_hm(changed_secs),
            move |i| change_row(&rows[i]),
        ));
    }
    if !added.is_empty() {
        let rows = added.clone();
        sections.append(&section(
            &ngettext("Added · {} entry", "Added · {} entries", added.len() as u32),
            added.len(),
            &format::hm(added_secs),
            move |i| entry_row(&rows[i], false),
        ));
    }

    keep.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));
    let account_id = account.id;
    apply.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            crate::sync::approve(account_id);
            dialog.close();
        }
    ));
    let account = account.clone();
    time_logs_row.connect_activated(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            let window = dialog.root().and_downcast::<gtk::Window>();
            dialog.close();
            let clients = crate::widgets::ClientsDialog::default();
            AdwDialogExt::present(&clients, window.as_ref());
            clients.open_client(account.client_id);
            crate::widgets::fibery_settings::show_time_logs(clients.upcast_ref(), &account);
        }
    ));
    dialog.present(Some(parent));
}

/// A heading ("Removed · 12 entries", total) and a list with the first
/// rows, then a "Show N More" row.
fn section(title: &str, count: usize, total: &str, row: impl Fn(usize) -> adw::ActionRow + 'static) -> gtk::Box {
    let row: Rc<dyn Fn(usize) -> adw::ActionRow> = Rc::new(row);
    let section = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let header = gtk::Box::builder().spacing(12).margin_top(10).margin_start(4).margin_end(4).build();
    header.append(
        &gtk::Label::builder()
            .label(title.replace("{}", &count.to_string()))
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["heading"])
            .build(),
    );
    header.append(&gtk::Label::builder().label(total).css_classes(["dim-label", "numeric", "heading"]).build());
    section.append(&header);

    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
    for i in 0..count.min(FIRST_ROWS) {
        list.append(&row(i));
    }
    if count > FIRST_ROWS {
        show_more(&list, row, FIRST_ROWS, count);
    }
    section.append(&list);
    section
}

/// A "Show N More" row that adds the next rows (`from`…) when activated.
fn show_more(list: &gtk::ListBox, row: Rc<dyn Fn(usize) -> adw::ActionRow>, from: usize, count: usize) {
    let next = (count - from).min(MORE_ROWS);
    let button = adw::ButtonRow::builder()
        .title(ngettext("Show {} More", "Show {} More", next as u32).replace("{}", &next.to_string()))
        .build();
    list.append(&button);
    button.connect_activated(glib::clone!(
        #[weak]
        list,
        move |button| {
            list.remove(button);
            for i in from..from + next {
                list.append(&row(i));
            }
            if from + next < count {
                show_more(&list, row.clone(), from + next, count);
            }
        }
    ));
}

/// A removed or added entry: task, when, project and duration (struck
/// through when removed).
fn entry_row(entry: &EntryView, removed: bool) -> adw::ActionRow {
    let mut parts = vec![format::short_date(&entry.date)];
    if let (Some(start), Some(end)) = (entry.start_at, entry.end_at) {
        parts.push(format!("{} – {}", format::clock_time(start), format::clock_time(end)));
    }
    parts.extend(entry.project.clone());
    let row = adw::ActionRow::builder()
        .title(&entry.task)
        .subtitle(parts.join(" · "))
        .use_markup(false)
        .title_lines(1)
        .build();
    let duration = gtk::Label::builder().label(format::hm(entry.secs)).css_classes(["numeric"]).build();
    if removed {
        let attrs = pango::AttrList::new();
        attrs.insert(pango::AttrInt::new_strikethrough(true));
        duration.set_attributes(Some(&attrs));
        duration.add_css_class("dim-label");
    }
    row.add_suffix(&duration);
    row
}

/// A changed entry: task, and what changed from what to what.
fn change_row(change: &Change) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(&change.after.task)
        .subtitle(change_subtitle(&change.before, &change.after))
        .use_markup(false)
        .title_lines(1)
        .build();
    row.add_suffix(&gtk::Label::builder().label(format::hm(change.after.secs)).css_classes(["numeric"]).build());
    row
}

/// "Mon 16 Nov · Duration 2:15 → 0:00", one part per change.
pub fn change_subtitle(before: &EntryView, after: &EntryView) -> String {
    let mut parts = vec![format::short_date(&before.date)];
    let arrow = |label: String, from: String, to: String| format!("{label} {from} → {to}");
    if before.date != after.date {
        parts.push(arrow(gettext("Date"), format::short_date(&before.date), format::short_date(&after.date)));
    }
    if before.project_id != after.project_id {
        let name = |p: &Option<String>| p.clone().unwrap_or_else(|| gettext("No project"));
        parts.push(arrow(gettext("Project"), name(&before.project), name(&after.project)));
    } else if before.task_id != after.task_id {
        parts.push(arrow(gettext("Task"), before.task.clone(), after.task.clone()));
    }
    if before.secs != after.secs {
        parts.push(arrow(gettext("Duration"), format::hm(before.secs), format::hm(after.secs)));
    } else if (before.start_at, before.end_at) != (after.start_at, after.end_at) {
        let times = |e: &EntryView| match (e.start_at, e.end_at) {
            (Some(start), Some(end)) => format!("{} – {}", format::clock_time(start), format::clock_time(end)),
            _ => format::hm(e.secs),
        };
        parts.push(arrow(gettext("Time"), times(before), times(after)));
    }
    parts.join(" · ")
}

/// `+0:30`, `−2:15` or `0:00`.
pub fn signed_hm(secs: i64) -> String {
    match secs {
        0 => format::hm(0),
        s if s < 0 => format!("−{}", format::hm(-s)),
        s => format!("+{}", format::hm(s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> EntryView {
        EntryView {
            task_id: 1,
            task: "Wireframes".into(),
            project_id: Some(1),
            project: Some("Website relaunch".into()),
            date: "2026-11-16".into(),
            start_at: None,
            end_at: None,
            secs: 8_100,
        }
    }

    #[test]
    fn describes_changes() {
        let before = entry();
        let zeroed = EntryView { secs: 0, ..entry() };
        assert!(change_subtitle(&before, &zeroed).ends_with("Duration 2:15 → 0:00"));
        let moved = EntryView { project_id: Some(2), project: Some("Mobile app".into()), ..entry() };
        assert!(change_subtitle(&before, &moved).ends_with("Project Website relaunch → Mobile app"));
        let other_task = EntryView { task_id: 2, task: "Onboarding".into(), ..entry() };
        assert!(change_subtitle(&before, &other_task).ends_with("Task Wireframes → Onboarding"));
        assert_eq!(signed_hm(-8_100), "−2:15");
        assert_eq!(signed_hm(1_800), "+0:30");
    }
}
