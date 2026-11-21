//! Real CRUD endpoints backed by the persistent Store.
//!
//! Wire shapes follow the pinned C# oracle (`ApiEnvelope<T>` + camelCase DTOs).
//! Null/empty payload validation mirrors `V2rayRuntime.*.cs` so the CI
//! differential corpus (which sends invalid inputs on a fresh data dir) keeps
//! getting byte-identical failures. On a populated device these handlers do
//! real persistence instead of the previous hardcoded no-ops.

use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

/// C# `ApiEnvelope<T>` with the standard `ok` code.
fn ok(message_key: &str, data: Value) -> Response {
    (
        StatusCode::OK,
        Json(json!({"success": true, "code": "ok", "messageKey": message_key, "data": data})),
    )
        .into_response()
}

fn created(message_key: &str, data: Value) -> Response {
    (
        StatusCode::CREATED,
        Json(json!({"success": true, "code": "ok", "messageKey": message_key, "data": data})),
    )
        .into_response()
}

fn fail(status: StatusCode, code: &str, message_key: &str, data: Value) -> Response {
    (
        status,
        Json(json!({"success": false, "code": code, "messageKey": message_key, "data": data})),
    )
        .into_response()
}

fn invalid(code: &str, message_key: &str) -> Response {
    fail(StatusCode::BAD_REQUEST, code, message_key, json!(null))
}

/// `Utils.GetId()` style 19-digit decimal id. Uses getrandom (already a dep).
fn new_id() -> String {
    let mut buf = [0u8; 8];
    if getrandom::fill(&mut buf).is_ok() {
        let n = u64::from_le_bytes(buf) % 9_000_000_000_000_000_000 + 1_000_000_000_000_000_000;
        n.to_string()
    } else {
        // Deterministic fallback; differential normalizes ids anyway.
        format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                % 9_000_000_000_000_000_000
                + 1_000_000_000_000_000_000
        )
    }
}

fn body_map(body: &Value) -> Map<String, Value> {
    body.as_object().cloned().unwrap_or_default()
}

fn non_empty(body: &Map<String, Value>, key: &str) -> bool {
    body.get(key)
        .and_then(Value::as_str)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Subscriptions
// ---------------------------------------------------------------------------

/// POST /api/subscriptions
pub async fn subscription_create(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    let m = body_map(&body);
    // C#: remarks required (SubscriptionInput validation).
    if !non_empty(&m, "remarks") {
        return invalid("subscription_name_required", "subscriptions.nameRequired");
    }
    let mut item = m.clone();
    let id = new_id();
    item.insert("id".into(), json!(id));
    // Fill C# defaults the UI expects on a freshly created row.
    for (key, value) in [
        ("url", json!("")),
        ("enabled", json!(true)),
        ("sort", json!(0)),
        ("autoUpdateInterval", json!(0)),
        ("updateTime", json!(0)),
    ] {
        item.entry(key.to_string()).or_insert(value);
    }
    let mut subs = state.store.array("subscriptions");
    subs.push(Value::Object(item.clone()));
    state.store.set_array("subscriptions", subs);
    created("subscriptions.created", Value::Object(item))
}

/// PUT /api/subscriptions/{id}
pub async fn subscription_update(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let mut subs = state.store.array("subscriptions");
    let Some(existing) = subs
        .iter_mut()
        .find(|s| s.get("id").and_then(Value::as_str) == Some(id.as_str()))
    else {
        return fail(
            StatusCode::NOT_FOUND,
            "subscription_not_found",
            "subscriptions.notFound",
            json!(null),
        );
    };
    let patch = body_map(&body);
    if let Some(obj) = existing.as_object_mut() {
        for (k, v) in patch {
            obj.insert(k, v);
        }
        obj.insert("id".into(), json!(id));
    }
    let result = existing.clone();
    state.store.set_array("subscriptions", subs);
    ok("subscriptions.updated", result)
}

/// DELETE /api/subscriptions/{id}
pub async fn subscription_delete(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let subs = state.store.array("subscriptions");
    if !subs
        .iter()
        .any(|s| s.get("id").and_then(Value::as_str) == Some(id.as_str()))
    {
        return fail(
            StatusCode::NOT_FOUND,
            "subscription_not_found",
            "subscriptions.notFound",
            json!(null),
        );
    }
    let kept: Vec<Value> = subs
        .into_iter()
        .filter(|s| s.get("id").and_then(Value::as_str) != Some(id.as_str()))
        .collect();
    state.store.set_array("subscriptions", kept);
    ok("subscriptions.deleted", json!(null))
}

// ---------------------------------------------------------------------------
// Profiles (nodes)
// ---------------------------------------------------------------------------

/// POST /api/profiles
pub async fn profile_create(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let m = body_map(&body);
    // C#: address required for a manual profile.
    if !non_empty(&m, "address") {
        return invalid("profile_address_required", "profiles.addressRequired");
    }
    let mut item = m.clone();
    let id = new_id();
    item.insert("indexId".into(), json!(id));
    for (key, value) in [
        ("configType", json!(1)),
        ("configVersion", json!(4)),
        ("remarks", json!("")),
        ("port", json!(0)),
        ("isSub", json!(false)),
    ] {
        item.entry(key.to_string()).or_insert(value);
    }
    let mut profiles = state.store.array("profiles");
    profiles.push(Value::Object(item.clone()));
    state.store.set_array("profiles", profiles);
    created("profiles.created", Value::Object(item))
}

/// PUT /api/profiles/{indexId}
pub async fn profile_update(
    State(state): State<AppState>,
    axum::extract::Path(index_id): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let mut profiles = state.store.array("profiles");
    let Some(existing) = profiles
        .iter_mut()
        .find(|p| p.get("indexId").and_then(Value::as_str) == Some(index_id.as_str()))
    else {
        return fail(
            StatusCode::CONFLICT,
            "profile_not_found",
            "profiles.notFound",
            json!(null),
        );
    };
    let patch = body_map(&body);
    if let Some(obj) = existing.as_object_mut() {
        for (k, v) in patch {
            obj.insert(k, v);
        }
        obj.insert("indexId".into(), json!(index_id));
    }
    let result = existing.clone();
    state.store.set_array("profiles", profiles);
    ok("profiles.updated", result)
}

/// GET /api/profiles/{indexId}
pub async fn profile_get(
    State(state): State<AppState>,
    axum::extract::Path(index_id): axum::extract::Path<String>,
) -> Response {
    match state.store.find_in_array("profiles", "indexId", &index_id) {
        Some(profile) => ok("profiles.loaded", profile),
        None => fail(
            StatusCode::NOT_FOUND,
            "profile_not_found",
            "profiles.notFound",
            json!(null),
        ),
    }
}

/// DELETE /api/profiles  body: { profileIds: [] }
pub async fn profile_delete(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let m = body_map(&body);
    let ids: Vec<String> = m
        .get("profileIds")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        return invalid("profile_selection_required", "profiles.selectRequired");
    }
    let profiles: Vec<Value> = state
        .store
        .array("profiles")
        .into_iter()
        .filter(|p| {
            p.get("indexId")
                .and_then(Value::as_str)
                .map(|id| !ids.iter().any(|x| x == id))
                .unwrap_or(true)
        })
        .collect();
    state.store.set_array("profiles", profiles);
    ok("profiles.deleted", json!(null))
}

/// POST /api/profiles/{id}/select
pub async fn profile_select(
    State(state): State<AppState>,
    axum::extract::Path(index_id): axum::extract::Path<String>,
) -> Response {
    if state
        .store
        .find_in_array("profiles", "indexId", &index_id)
        .is_none()
    {
        return fail(
            StatusCode::CONFLICT,
            "profile_not_found",
            "profiles.notFound",
            json!(null),
        );
    }
    state.store.set_string("currentProfileId", &index_id);
    ok("profiles.selected", json!({"indexId": index_id}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_id_is_19_digits() {
        let id = new_id();
        assert_eq!(id.len(), 19);
        assert!(id.chars().all(|c| c.is_ascii_digit()));
    }
}
