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
