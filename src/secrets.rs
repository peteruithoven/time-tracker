//! API tokens in the system keyring, through libsecret. In Flatpak,
//! libsecret uses the Secret portal.

use std::collections::HashMap;

use libsecret::{Schema, SchemaAttributeType, SchemaFlags};

fn schema() -> Schema {
    let attributes = HashMap::from([
        ("workspace", SchemaAttributeType::String),
        ("user", SchemaAttributeType::String),
    ]);
    Schema::new("nl.peteruithoven.TimeTracker.ApiKey", SchemaFlags::NONE, attributes)
}

fn attributes<'a>(workspace: &'a str, user_id: &'a str) -> HashMap<&'a str, &'a str> {
    HashMap::from([("workspace", workspace), ("user", user_id)])
}

pub async fn store_token(workspace: &str, user_id: &str, token: &str) -> Result<(), String> {
    libsecret::password_store_future(
        Some(&schema()),
        attributes(workspace, user_id),
        Some(libsecret::COLLECTION_DEFAULT),
        &format!("Time Tracker: Fibery API key for {workspace}"),
        token,
    )
    .await
    .map_err(|e| e.to_string())
}

pub async fn load_token(workspace: &str, user_id: &str) -> Result<Option<String>, String> {
    libsecret::password_lookup_future(Some(&schema()), attributes(workspace, user_id))
        .await
        .map(|token| token.map(|t| t.to_string()))
        .map_err(|e| e.to_string())
}

pub async fn delete_token(workspace: &str, user_id: &str) -> Result<(), String> {
    libsecret::password_clear_future(Some(&schema()), attributes(workspace, user_id))
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn invoice_schema() -> Schema {
    let attributes = HashMap::from([
        ("script", SchemaAttributeType::String),
        ("client", SchemaAttributeType::String),
    ]);
    Schema::new("nl.peteruithoven.TimeTracker.InvoiceToken", SchemaFlags::NONE, attributes)
}

fn invoice_attributes<'a>(script: &'a str, client: &'a str) -> HashMap<&'a str, &'a str> {
    HashMap::from([("script", script), ("client", client)])
}

/// The API token of an invoice script for a client.
pub async fn store_invoice_token(script: &str, client_id: i64, label: &str, token: &str) -> Result<(), String> {
    let client = client_id.to_string();
    libsecret::password_store_future(
        Some(&invoice_schema()),
        invoice_attributes(script, &client),
        Some(libsecret::COLLECTION_DEFAULT),
        &format!("Time Tracker: {label}"),
        token,
    )
    .await
    .map_err(|e| e.to_string())
}

pub async fn load_invoice_token(script: &str, client_id: i64) -> Result<Option<String>, String> {
    let client = client_id.to_string();
    libsecret::password_lookup_future(Some(&invoice_schema()), invoice_attributes(script, &client))
        .await
        .map(|token| token.map(|t| t.to_string()))
        .map_err(|e| e.to_string())
}

pub async fn delete_invoice_token(script: &str, client_id: i64) -> Result<(), String> {
    let client = client_id.to_string();
    libsecret::password_clear_future(Some(&invoice_schema()), invoice_attributes(script, &client))
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}
