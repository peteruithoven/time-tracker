//! Made-up demo data for screenshots. Dates are relative to today, so the
//! screenshots always show a believable current month.

use crate::format;
use crate::report;
use crate::store::{EntryInput, Result, Store};

/// Fills an empty database (no tasks yet).
pub fn seed(store: &Store) -> Result<()> {
    if !store.open_tasks()?.is_empty() {
        return Ok(());
    }
    let now = format::now();
    let today = format::today();
    let Some((year, month, today_day)) = format::date_parts(&today) else {
        return Ok(());
    };

    let studio = store.create_client("Acme Studio", now)?;
    let northwind = store.create_client("Northwind", now)?;
    let personal = store.clients()?.into_iter().find(|c| !c.billable).map(|c| c.id).unwrap_or(studio);
    for (id, rate) in [(studio, 9_500), (northwind, 8_500)] {
        if let Some(mut client) = store.client(id)? {
            client.hourly_rate_cents = Some(rate);
            store.update_client(&client)?;
        }
    }

    let website = store.create_project(studio, "Website relaunch", "blue", now)?;
    let app = store.create_project(studio, "Mobile app", "orange", now)?;
    let brand = store.create_project(northwind, "Brand refresh", "green", now)?;
    let admin = store.create_project(personal, "Admin", "slate", now)?;

    let task = |client, project, name: &str| store.create_task(client, Some(project), name, now);
    let briefing = task(studio, website, "Briefing")?;
    let wireframes = task(studio, website, "Wireframes")?;
    let design_system = task(studio, website, "Design system")?;
    let payments = task(studio, app, "Payment flow")?;
    let onboarding = task(studio, app, "Onboarding screens")?;
    let logo = task(northwind, brand, "Logo concepts")?;
    let guidelines = task(northwind, brand, "Brand guidelines")?;
    let invoicing = task(personal, admin, "Invoicing")?;
    store.demo_mark_remote(studio)?;

    let add = |task_id, date: &str, (h1, m1): (u32, u32), (h2, m2): (u32, u32), note: &str| -> Result<()> {
        let (Some(start), Some(end)) = (format::local_timestamp(date, h1, m1), format::local_timestamp(date, h2, m2)) else {
            return Ok(());
        };
        if end > now {
            return Ok(());
        }
        store.add_entry(
            &EntryInput {
                task_id,
                date: date.to_string(),
                start_at: Some(start),
                end_at: Some(end),
                duration_secs: None,
                note: note.to_string(),
            },
            now,
        )?;
        Ok(())
    };

    // A believable working pattern on weekdays.
    let rotation = [
        (wireframes, logo, "Feedback round"),
        (design_system, guidelines, ""),
        (payments, logo, "Call with client"),
        (onboarding, guidelines, ""),
        (briefing, design_system, "Workshop prep"),
    ];
    for day in 1..today_day {
        if report::weekday(year, month, day) >= 5 {
            continue;
        }
        let date = format::date_string(year, month, day);
        let (morning, afternoon, note) = rotation[day as usize % rotation.len()];
        add(morning, &date, (9, 0), (12, 15 + (day % 3) * 10), note)?;
        add(afternoon, &date, (13, 0), (15, 30 + (day % 4) * 5), "")?;
        if day % 3 == 0 {
            add(invoicing, &date, (16, 0), (16, 40), "Expenses")?;
        }
    }

    // Today: a few finished entries and a running timer.
    add(invoicing, &today, (8, 20), (8, 45), "Expenses")?;
    add(briefing, &today, (8, 50), (9, 40), "")?;
    add(logo, &today, (9, 45), (10, 30), "Sketches")?;
    store.start_timer(wireframes, now - (84 * 60 + 7), &today)?;
    Ok(())
}
