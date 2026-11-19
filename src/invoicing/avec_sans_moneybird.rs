//! A draft sales invoice in Moneybird with a line per project, billed in
//! days.
//!
//! An earlier invoice serves as template: the contact, contact person, VAT
//! rate, ledger account and layout are copied from it. Its address (which
//! holds the administration and invoice IDs) is a script setting. The day
//! rate is the project's or client's hourly rate × 8.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use super::{ProjectHours, Request, Script, SettingField, Settings};

const HOURS_PER_DAY: f64 = 8.0;
const TEMPLATE_URL: &str = "template_url";
/// Description for time on tasks without a project.
const NO_PROJECT: &str = "Overige werkzaamheden";
/// Invoice fields copied from the template when present.
const COPIED_FIELDS: &[&str] = &[
    "contact_id",
    "contact_person_id",
    "workflow_id",
    "document_style_id",
    "prices_are_incl_tax",
    "currency",
    "language",
];

pub const SCRIPT: Script = Script {
    id: "avec-sans-moneybird",
    name: "Avec Sans Moneybird invoice",
    help: "Creates a draft in Moneybird with a line per project in days (hourly rate × 8), copying the contact and VAT from an earlier invoice. \
           Create a personal API token at moneybird.com/user/applications/new with access to sales invoices.",
    settings: &[SettingField { key: TEMPLATE_URL, title: "Template Invoice Address" }],
    describe_line,
    check,
    create,
};

/// Administration and invoice ID from an invoice's address, e.g.
/// `https://moneybird.com/123/sales_invoices/456`.
fn parse_template(url: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = url.trim().trim_end_matches('/').split('/').collect();
    let position = parts.iter().position(|p| *p == "sales_invoices");
    match position {
        Some(i) if i >= 1 && i + 1 < parts.len() => {
            let (administration, invoice) = (parts[i - 1], parts[i + 1]);
            let numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
            if numeric(administration) && numeric(invoice) {
                return Ok((administration.to_string(), invoice.to_string()));
            }
            Err("The template address should end in /sales_invoices/ and a number.".into())
        }
        _ => Err("Paste the address of a Moneybird invoice, e.g. https://moneybird.com/…/sales_invoices/….".into()),
    }
}

fn template_ids(settings: &Settings) -> Result<(String, String), String> {
    parse_template(settings.get(TEMPLATE_URL).map(String::as_str).unwrap_or_default())
}

/// Days for `hours`, rounded to two decimals.
fn days(hours: f64) -> f64 {
    (hours / HOURS_PER_DAY * 100.0).round() / 100.0
}

/// "5.43", "2" (no trailing zeros).
fn decimal(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Day price as Moneybird money, e.g. "320.00".
fn day_price(hourly_rate_cents: i64) -> String {
    let cents = (hourly_rate_cents as f64 * HOURS_PER_DAY).round() as i64;
    format!("{}.{:02}", cents / 100, cents % 100)
}

fn describe_line(line: &ProjectHours) -> String {
    let days = decimal(days(line.hours));
    match line.hourly_rate_cents {
        Some(rate) => format!("{days} days × € {}", decimal(rate as f64 * HOURS_PER_DAY / 100.0)),
        None => format!("{days} days (no hourly rate set)"),
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn read(response: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Value, String> {
    let mut response = response.map_err(|e| format!("Could not reach Moneybird: {e}"))?;
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_string().unwrap_or_default();
    match status {
        200..=299 => serde_json::from_str(&body).map_err(|e| e.to_string()),
        401 => Err("Moneybird didn't accept the API token.".into()),
        404 => Err("The template invoice wasn't found in Moneybird.".into()),
        _ => Err(format!("Moneybird answered {status}: {}", body.chars().take(400).collect::<String>())),
    }
}

fn template(agent: &ureq::Agent, token: &str, administration: &str, invoice: &str) -> Result<Value, String> {
    read(
        agent
            .get(&format!("https://moneybird.com/api/v2/{administration}/sales_invoices/{invoice}.json"))
            .header("Authorization", &format!("Bearer {}", token.trim()))
            .call(),
    )
}

fn check(token: &str, settings: &Settings) -> Result<(), String> {
    let (administration, invoice) = template_ids(settings)?;
    template(&agent(), token, &administration, &invoice).map(|_| ())
}

/// The VAT rate and ledger account used by most of the template's lines.
fn common_booking(template: &Value) -> Option<(Value, Value)> {
    let mut counts: HashMap<String, (usize, (Value, Value))> = HashMap::new();
    for detail in template["details"].as_array().into_iter().flatten() {
        let pair = (detail["tax_rate_id"].clone(), detail["ledger_account_id"].clone());
        let key = format!("{}|{}", pair.0, pair.1);
        counts.entry(key).or_insert((0, pair)).0 += 1;
    }
    counts.into_values().max_by_key(|(count, _)| *count).map(|(_, pair)| pair)
}

/// The invoice body: template fields plus a line per project.
fn invoice_body(template: &Value, request: &Request) -> Result<Value, String> {
    let mut invoice = Map::new();
    for field in COPIED_FIELDS {
        if let Some(value) = template.get(*field).filter(|v| !v.is_null()) {
            invoice.insert((*field).to_string(), value.clone());
        }
    }
    let booking = common_booking(template);
    let period = format!(
        "{0:04}{1:02}01..{0:04}{1:02}{2:02}",
        request.year, request.month, request.days_in_month
    );
    let mut details = Vec::new();
    for line in &request.projects {
        let name = line.project.as_deref().unwrap_or(NO_PROJECT);
        let rate = line
            .hourly_rate_cents
            .ok_or_else(|| format!("Set an hourly rate for the client or for “{name}”."))?;
        let mut detail = json!({
            "description": name,
            "amount": format!("{} x", decimal(days(line.hours))),
            "price": day_price(rate),
            "period": period,
        });
        if let Some((tax_rate, ledger)) = &booking {
            for (field, value) in [("tax_rate_id", tax_rate), ("ledger_account_id", ledger)] {
                if !value.is_null() {
                    detail[field] = value.clone();
                }
            }
        }
        details.push(detail);
    }
    invoice.insert("details_attributes".into(), Value::Array(details));
    Ok(json!({ "sales_invoice": invoice }))
}

fn create(token: &str, settings: &Settings, request: &Request) -> Result<String, String> {
    let (administration, invoice) = template_ids(settings)?;
    let agent = agent();
    let template = template(&agent, token, &administration, &invoice)?;
    let body = invoice_body(&template, request)?;
    let created = read(
        agent
            .post(&format!("https://moneybird.com/api/v2/{administration}/sales_invoices.json"))
            .header("Authorization", &format!("Bearer {}", token.trim()))
            .send_json(&body),
    )?;
    let id = created["id"]
        .as_str()
        .map(str::to_string)
        .or_else(|| created["id"].as_u64().map(|n| n.to_string()))
        .ok_or("Moneybird didn't return the new invoice's id.")?;
    Ok(format!("https://moneybird.com/{administration}/sales_invoices/{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_hours_and_rates() {
        assert_eq!(days(8.0), 1.0);
        assert_eq!(decimal(days(43.45)), "5.43");
        assert_eq!(decimal(days(16.0)), "2");
        assert_eq!(decimal(days(12.0)), "1.5");
        assert_eq!(day_price(4_000), "320.00");
        assert_eq!(day_price(8_550), "684.00");
        let line = ProjectHours { project: None, hours: 20.0, hourly_rate_cents: Some(4_000) };
        assert_eq!(describe_line(&line), "2.5 days × € 320");
    }

    #[test]
    fn parses_template_addresses() {
        assert_eq!(
            parse_template("https://moneybird.com/123/sales_invoices/456"),
            Ok(("123".into(), "456".into()))
        );
        assert_eq!(parse_template(" https://moneybird.com/123/sales_invoices/456/ "), Ok(("123".into(), "456".into())));
        assert!(parse_template("https://moneybird.com/123/contacts/456").is_err());
        assert!(parse_template("").is_err());
    }

    #[test]
    fn builds_invoice_from_template() {
        let template = json!({
            "contact_id": "111",
            "contact_person_id": "222",
            "workflow_id": "333",
            "document_style_id": null,
            "reference": "October",
            "details": [
                {"description": "Fixed fee", "tax_rate_id": "9", "ledger_account_id": "8"},
                {"description": "Website", "tax_rate_id": "21", "ledger_account_id": "80"},
                {"description": "App", "tax_rate_id": "21", "ledger_account_id": "80"}
            ]
        });
        let request = Request {
            year: 2026,
            month: 11,
            days_in_month: 30,
            projects: vec![
                ProjectHours { project: Some("Website".into()), hours: 43.45, hourly_rate_cents: Some(4_000) },
                ProjectHours { project: None, hours: 4.0, hourly_rate_cents: Some(4_000) },
            ],
        };
        let body = invoice_body(&template, &request).unwrap();
        let invoice = &body["sales_invoice"];
        assert_eq!(invoice["contact_id"], "111");
        assert_eq!(invoice["contact_person_id"], "222");
        assert!(invoice.get("document_style_id").is_none(), "nulls aren't copied");
        assert!(invoice.get("reference").is_none(), "only listed fields are copied");
        let details = invoice["details_attributes"].as_array().unwrap();
        assert_eq!(
            details[0],
            json!({
                "description": "Website",
                "amount": "5.43 x",
                "price": "320.00",
                "period": "20261101..20261130",
                "tax_rate_id": "21",
                "ledger_account_id": "80"
            })
        );
        assert_eq!(details[1]["description"], NO_PROJECT);
        assert_eq!(details[1]["amount"], "0.5 x");
    }

    #[test]
    fn requires_a_rate() {
        let request = Request {
            year: 2026,
            month: 11,
            days_in_month: 30,
            projects: vec![ProjectHours { project: Some("Website".into()), hours: 8.0, hourly_rate_cents: None }],
        };
        assert!(invoice_body(&json!({}), &request).unwrap_err().contains("hourly rate"));
    }
}
