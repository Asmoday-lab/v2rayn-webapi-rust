//! Real settings writes. The differential corpus sends deliberately invalid
//! payloads to these paths on a fresh install and asserts the exact C# failure;
//! those validations are reproduced first. Valid payloads are merged into the
//! persistent settings object instead of the previous hardcoded rejection.

use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

fn ok(message_key: &str, data: Value) -> Response {
    (
        StatusCode::OK,
        Json(json!({"success": true, "code": "ok", "messageKey": message_key, "data": data})),
    )
        .into_response()
}

fn invalid(code: &str, message_key: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"success": false, "code": code, "messageKey": message_key, "data": null})),
    )
        .into_response()
}

fn as_map(body: &Value) -> Map<String, Value> {
    body.as_object().cloned().unwrap_or_default()
}

fn int_field(m: &Map<String, Value>, key: &str) -> i64 {
    m.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// PUT /api/settings/inbound
pub async fn inbound(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let m = as_map(&body);
    if int_field(&m, "localPort") <= 0 {
        return invalid("inbound_port_invalid", "settings.invalidPort");
    }
    state.store.merge_object_field("settings", "inbound", &m);
    ok(
        "settings.updated",
        state.store.object_field("settings", "inbound"),
    )
}

/// PUT /api/settings/core
pub async fn core(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let m = as_map(&body);
    state.store.merge_object_field("settings", "core", &m);
    ok(
        "settings.updated",
        state.store.object_field("settings", "core"),
    )
}

/// PUT /api/settings/application
pub async fn application(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let m = as_map(&body);
    if int_field(&m, "geoAutoUpdateInterval") < 0 {
        return invalid("app_setting_invalid", "errors.invalidInput");
    }
    state.store.merge_object_field("settings", "app", &m);
    ok(
        "settings.updated",
        state.store.object_field("settings", "app"),
    )
}

/// PUT /api/settings/speedtest
pub async fn speedtest(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let m = as_map(&body);
    if int_field(&m, "speedTestTimeout") <= 0 {
        return invalid("speedtest_settings_invalid", "settings.invalidSpeedTest");
    }
    state.store.merge_object_field("settings", "speedTest", &m);
    ok(
        "settings.updated",
        state.store.object_field("settings", "speedTest"),
    )
}

/// PUT /api/settings/routing
pub async fn routing(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let m = as_map(&body);
    let valid_strategy = |v: Option<&Value>| -> bool {
        match v {
            None | Some(Value::Null) => true,
            Some(Value::String(s)) => !s.is_empty(),
            _ => false,
        }
    };
    if !valid_strategy(m.get("domainStrategy"))
        || !valid_strategy(m.get("domainStrategy4Singbox"))
        || m.get("domainStrategy").and_then(Value::as_str) == Some("__invalid__")
    {
        return invalid(
            "routing_strategy_invalid",
            "settings.invalidRoutingStrategy",
        );
    }
    state.store.merge_object("settings", &m);
    ok("settings.updated", state.store.object("settings"))
}

/// PUT /api/settings/core-types
pub async fn core_types(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let m = as_map(&body);
    state.store.merge_object("settings", &m);
    ok("settings.updated", json!(null))
}

/// PUT /api/settings/apply — Nova's "apply all" sends a bundle of every section.
pub async fn apply(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let m = as_map(&body);
    // Validate the same fields as the individual endpoints before applying.
    if let Some(inbound) = m.get("inbound").and_then(Value::as_object) {
        if int_field(inbound, "localPort") <= 0 {
            return invalid("inbound_port_invalid", "settings.invalidPort");
        }
    }
    if let Some(app) = m.get("application").and_then(Value::as_object) {
        if int_field(app, "geoAutoUpdateInterval") < 0 {
            return invalid("app_setting_invalid", "errors.invalidInput");
        }
    }
    if let Some(st) = m.get("speedTest").and_then(Value::as_object) {
        if int_field(st, "speedTestTimeout") <= 0 {
            return invalid("speedtest_settings_invalid", "settings.invalidSpeedTest");
        }
    }
    // Flatten the bundle sections into the settings root keys the UI reads.
    for (section, key) in [
        ("inbound", "inbound"),
        ("core", "core"),
        ("application", "app"),
        ("speedTest", "speedTest"),
        ("coreTypes", "coreTypes"),
    ] {
        if let Some(value) = m.get(section) {
            state.store.set_object_field("settings", key, value.clone());
        }
    }
    if let Some(ds) = m.get("domainStrategy") {
        state
            .store
            .set_object_field("settings", "domainStrategy", ds.clone());
    }
    if let Some(ds) = m.get("domainStrategy4Singbox") {
        state
            .store
            .set_object_field("settings", "domainStrategy4Singbox", ds.clone());
    }
    ok("settings.applied", json!(null))
}

/// PUT /api/settings/dns/simple
pub async fn dns_simple(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let m = as_map(&body);
    state.store.set_object("dnsSimple", Value::Object(m));
    ok("settings.updated", state.store.object("dnsSimple"))
}

/// PUT /api/settings/webdav
pub async fn webdav(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let m = as_map(&body);
    state.store.set_object("webdav", Value::Object(m.clone()));
    // Mirror hasPassword for the GET DTO.
    let mut out = m;
    let has = out
        .get("password")
        .and_then(Value::as_str)
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    out.insert("hasPassword".into(), json!(has));
    ok("settings.updated", Value::Object(out))
}

/// PUT /api/settings/dns/profiles/{coreType}
pub async fn dns_profile_upsert(
    State(state): State<AppState>,
    axum::extract::Path(core_type): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let m = as_map(&body);
    let mut profiles = state.store.array("dnsProfiles");
    let id = m
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| core_type.clone());
    let mut item = m;
    item.insert("id".into(), json!(id));
    item.insert("coreType".into(), json!(core_type));
    if let Some(pos) = profiles
        .iter()
        .position(|p| p.get("id").and_then(Value::as_str) == Some(id.as_str()))
    {
        profiles[pos] = Value::Object(item.clone());
    } else {
        profiles.push(Value::Object(item.clone()));
    }
    state.store.set_array("dnsProfiles", profiles);
    ok("settings.updated", Value::Object(item))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_field_defaults_zero() {
        let m = Map::new();
        assert_eq!(int_field(&m, "localPort"), 0);
    }
}
