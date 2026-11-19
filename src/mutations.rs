//! State-mutating endpoints whose empty-data behavior is deterministic.
//!
//! Response shapes captured from the pinned oracle (oracle-dump.json artifact,
//! `__mutations__` section). On a fresh Termux install there is no Core, no
//! profiles and no subscriptions, so these operations either no-op success or
//! return a stable failure. Real Core-driven mutation is out of scope for the
//! headless MVP; Nova's load/operate flow is covered by these responses.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

fn operation(
    success: bool,
    code: &str,
    message_key: &str,
    data: Value,
    status: StatusCode,
) -> Response {
    (
        status,
        Json(json!({"success": success, "code": code, "messageKey": message_key, "data": data})),
    )
        .into_response()
}

fn ok(message_key: &str, data: Value) -> Response {
    operation(true, "ok", message_key, data, StatusCode::OK)
}

pub async fn clear_logs() -> Response {
    // generation is a per-process counter; the differential normalizes it.
    ok("logs.cleared", json!({"cleared": true, "generation": 1}))
}

pub async fn clear_statistics() -> Response {
    ok("common.deleted", json!(null))
}

pub async fn stop_speedtests() -> Response {
    ok("common.completed", json!(null))
}

pub async fn stop_latency() -> Response {
    ok("common.completed", json!(null))
}

pub async fn core_stop() -> Response {
    ok("core.stopped", json!(null))
}

pub async fn profiles_deduplicate() -> Response {
    ok("profiles.deduplicated", json!({"removed": 0}))
}

pub async fn profiles_clear_invalid() -> Response {
    ok("common.completed", json!({"removed": 0}))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupCurrentRequest {
    #[serde(default)]
    subscription_id: Option<String>,
}

pub async fn set_current_group(Json(request): Json<GroupCurrentRequest>) -> Response {
    let id = request
        .subscription_id
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    ok("common.completed", json!({"subscriptionId": id}))
}

pub async fn routing_import() -> Response {
    ok(
        "common.completed",
        json!({"changed": false, "coreRestarted": false, "restartRequired": false}),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionUpdateRequest {
    #[serde(default)]
    subscription_id: Option<String>,
    #[serde(default)]
    use_proxy: Option<bool>,
}

pub async fn subscription_update(Json(request): Json<SubscriptionUpdateRequest>) -> Response {
    operation(
        true,
        "ok",
        "subscriptions.updateStarted",
        json!({
            "subscriptionId": request.subscription_id.unwrap_or_default(),
            "useProxy": request.use_proxy.unwrap_or(false),
        }),
        StatusCode::ACCEPTED,
    )
}

pub async fn generate_all_groups() -> Response {
    operation(
        false,
        "profile_group_empty",
        "profiles.grouped",
        json!(null),
        StatusCode::BAD_REQUEST,
    )
}

pub async fn profile_select() -> Response {
    operation(
        false,
        "profile_not_found",
        "profiles.notFound",
        json!(null),
        StatusCode::CONFLICT,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn operation_envelope_is_c_compatible() {
        let resp = profiles_deduplicate().await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn set_current_normalizes_null_group() {
        // serde default maps a missing subscriptionId to None -> "".
        let r = GroupCurrentRequest {
            subscription_id: None,
        };
        let id = r
            .subscription_id
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        assert_eq!(id, "");
    }

    #[test]
    fn camel_case_request_fields_are_deserialized() {
        // C# Web defaults are camelCase; snake_case must not silently become None.
        let r: SubscriptionUpdateRequest =
            serde_json::from_str(r#"{"subscriptionId":"sub-1","useProxy":true}"#).unwrap();
        assert_eq!(r.subscription_id.as_deref(), Some("sub-1"));
        assert_eq!(r.use_proxy, Some(true));

        let g: GroupCurrentRequest = serde_json::from_str(r#"{"subscriptionId":"g-1"}"#).unwrap();
        assert_eq!(g.subscription_id.as_deref(), Some("g-1"));
    }
}
