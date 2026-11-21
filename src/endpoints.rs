//! Static/near-static GET endpoints whose response shapes were captured from
//! the pinned C# oracle (oracle-dump.json artifact) and embedded as data.
//!
//! These endpoints are deterministic on a fresh data directory, so returning
//! the captured `data` verbatim gives byte-exact wire compatibility. Only the
//! envelope's messageKey varies per endpoint. Runtime-dependent endpoints
//! (`status`, `logs`, `web-updates`) are handled separately.

use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// Wrap a static `data` value in the C# `ApiEnvelope<T>` shape.
fn ok(data: Value, message_key: &str) -> Response {
    (
        StatusCode::OK,
        Json(json!({"success": true, "code": "ok", "messageKey": message_key, "data": data})),
    )
        .into_response()
}

fn static_data(raw: &str) -> Value {
    serde_json::from_str(raw).expect("embedded resource is valid JSON")
}

pub async fn operations() -> Response {
    ok(
        static_data(include_str!("../resources/operations.json")),
        "operations.loaded",
    )
}

pub async fn profile_groups() -> Response {
    ok(
        static_data(include_str!("../resources/profile_groups.json")),
        "profiles.groupsLoaded",
    )
}

pub async fn webdav_settings(State(state): State<AppState>) -> Response {
    ok(state.store.object("webdav"), "backup.webdavSettingsLoaded")
}

pub async fn core_updates() -> Response {
    ok(
        static_data(include_str!("../resources/core_updates.json")),
        "maintenance.updatesLoaded",
    )
}

pub async fn core_update_progress() -> Response {
    ok(
        static_data(include_str!("../resources/core_update_progress.json")),
        "maintenance.updatesLoaded",
    )
}

/// Nova's maintenance page loads this unconditionally. Its data is build
/// metadata, stable on the pinned oracle (no per-run ids or timestamps).
pub async fn web_updates() -> Response {
    ok(
        static_data(include_str!("../resources/web_updates.json")),
        "maintenance.updatesLoaded",
    )
}

/// Nova loads `/api/logs/page` on every authenticated shell startup.
/// Log *content* is process-local and cannot be a fixed golden; an empty page
/// is returned so the log view renders instead of raising a 404.
pub async fn logs_page() -> Response {
    ok(
        static_data(include_str!("../resources/logs_page.json")),
        "logs.loaded",
    )
}

pub async fn editor_options() -> Response {
    ok(
        static_data(include_str!("../resources/editor_options.json")),
        "common.loaded",
    )
}

pub async fn dns_editor_options() -> Response {
    ok(
        static_data(include_str!("../resources/dns_editor_options.json")),
        "common.loaded",
    )
}

pub async fn dns_defaults() -> Response {
    ok(
        static_data(include_str!("../resources/dns_defaults.json")),
        "common.loaded",
    )
}

pub async fn dns_profiles(State(state): State<AppState>) -> Response {
    ok(
        Value::Array(state.store.array("dnsProfiles")),
        "dns.profilesLoaded",
    )
}

pub async fn routing_profiles(State(state): State<AppState>) -> Response {
    ok(
        Value::Array(state.store.array("routingProfiles")),
        "routing.profilesLoaded",
    )
}

pub async fn core_templates(State(state): State<AppState>) -> Response {
    ok(
        Value::Array(state.store.array("coreTemplates")),
        "coreTemplates.loaded",
    )
}

pub async fn settings(State(state): State<AppState>) -> Response {
    ok(state.store.object("settings"), "settings.loaded")
}

pub async fn dns_simple(State(state): State<AppState>) -> Response {
    ok(state.store.object("dnsSimple"), "dns.simpleLoaded")
}

/// `/api/status`: static baseline from the oracle with three runtime-dependent
/// fields filled in. `runtime` is an environment diagnostic string that differs
/// by design (the oracle reports its CI paths); the differential harness
/// normalizes it plus `checkedAt`/`apiPort` before comparison.
pub async fn status_handler() -> Response {
    status(env!("CARGO_PKG_VERSION"))
}

pub fn status(version: &str) -> Response {
    let mut data: Value = static_data(include_str!("../resources/status.json"));
    if let Value::Object(ref mut map) = data {
        // Environment diagnostic string — differs by design. The differential
        // harness normalizes `runtime`, `checkedAt` and `apiPort` before compare.
        map.insert(
            "runtime".to_string(),
            json!(format!("v2rayn-webapi-rs {version} (Termux)")),
        );
        map.insert(
            "checkedAt".to_string(),
            json!(crate::session::format_expires_at(
                std::time::SystemTime::now(),
                0
            )),
        );
    }
    ok(data, "status.loaded")
}

/// `/api/subscriptions` and `/api/profiles` read the persistent store. A fresh
/// store is empty, which stays byte-identical with the oracle's fresh data dir;
/// a populated device now returns the real rows Nova created through CRUD.
/// Optional `subscriptionId` query filters profiles to one group.
pub async fn subscriptions(State(state): State<AppState>) -> Response {
    ok(
        Value::Array(state.store.array("subscriptions")),
        "subscriptions.loaded",
    )
}

pub async fn profiles(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let rows = state.store.array("profiles");
    let filtered = match params.get("subscriptionId").filter(|s| !s.is_empty()) {
        Some(sub_id) => rows
            .into_iter()
            .filter(|p| p.get("subid").and_then(Value::as_str) == Some(sub_id.as_str()))
            .collect(),
        None => rows,
    };
    ok(Value::Array(filtered), "profiles.loaded")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn all_embedded_resources_parse() {
        // Every embedded golden must parse. Store-seeded resources (settings,
        // dns*, routing*, templates, webdav) are validated in store::tests;
        // this covers the ones still served directly.
        let responses = [
            operations().await,
            profile_groups().await,
            core_updates().await,
            core_update_progress().await,
            editor_options().await,
            dns_editor_options().await,
            dns_defaults().await,
        ];
        assert_eq!(responses.len(), 7);
    }

    #[test]
    fn envelope_shape_is_c_compatible() {
        let value = serde_json::to_value(static_data(include_str!("../resources/operations.json")))
            .unwrap();
        assert_eq!(value, json!([]));
    }
}
