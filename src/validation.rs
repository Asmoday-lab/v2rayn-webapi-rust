//! Input-validation endpoints whose headless behavior is a deterministic error.
//!
//! These paths return before any Core/process/network/file side effect, so the
//! response is a fixed `OperationView`. The request bodies are still sent by
//! the differential harness (the C# side validates them); the Rust side only
//! needs to reproduce the observable reply. Source: `V2rayRuntime.*.cs` and
//! `Contracts/ApiModels.cs` of the pinned oracle.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

fn fail(code: &str, message_key: &str, data: Value, status: StatusCode) -> Response {
    (
        status,
        Json(json!({"success": false, "code": code, "messageKey": message_key, "data": data})),
    )
        .into_response()
}

fn invalid(code: &str, message_key: &str) -> Response {
    fail(code, message_key, json!(null), StatusCode::BAD_REQUEST)
}

/// `POST /api/profiles/import` with empty content.
pub async fn profile_import() -> Response {
    fail(
        "profile_content_required",
        "errors.invalidInput",
        json!({"imported": 0}),
        StatusCode::BAD_REQUEST,
    )
}

/// `POST /api/settings/routing-profiles/{id}/rules/import` with malformed JSON.
pub async fn routing_rules_import() -> Response {
    invalid("routing_rules_json_invalid", "routing.rulesInvalid")
}

/// `POST /api/profiles/sort` with an unparseable EServerColName.
pub async fn profiles_sort() -> Response {
    invalid("profile_sort_column_invalid", "errors.invalidInput")
}

/// `POST /api/settings/routing-profiles/{missing}/rules/import-url`.
pub async fn routing_rules_import_url() -> Response {
    invalid("routing_profile_not_found", "routing.profileNotFound")
}

/// `POST /api/profile-groups/generate/regions` with an unknown subscriptionId.
pub async fn generate_regions_groups() -> Response {
    invalid("subscription_not_found", "subscriptions.notFound")
}

/// `POST /api/profiles/copy` with an empty profileIds array.
pub async fn profiles_copy() -> Response {
    invalid("profile_not_found", "profiles.notFound")
}

/// `POST /api/speedtests` on a fresh install with no profiles to test.
/// Nova routes every ping/speed test through this endpoint (not `/latency`).
pub async fn speedtests_start() -> Response {
    invalid(
        "speedtest_profile_selection_required",
        "speedtest.selectRequired",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn validation_failures_carry_c_sharp_envelope() {
        let cases = [
            (profile_import().await, "profile_content_required"),
            (routing_rules_import().await, "routing_rules_json_invalid"),
            (profiles_sort().await, "profile_sort_column_invalid"),
            (
                routing_rules_import_url().await,
                "routing_profile_not_found",
            ),
            (generate_regions_groups().await, "subscription_not_found"),
            (profiles_copy().await, "profile_not_found"),
            (
                speedtests_start().await,
                "speedtest_profile_selection_required",
            ),
        ];
        for (resp, expected_code) in cases {
            assert!(resp.status().is_client_error());
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["success"], false);
            assert_eq!(value["code"], expected_code);
            assert!(value["messageKey"].is_string());
            assert!(value.get("data").is_some());
        }
    }
}
