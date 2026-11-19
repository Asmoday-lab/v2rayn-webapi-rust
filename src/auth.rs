//! Auth + setup endpoints, ported from Api/WebAuthEndpoints.cs and
//! Api/WebSetupEndpoints.cs. Response shapes are kept byte-compatible.

use crate::security::{envelope, extract_bearer, is_setup_allowed};
use crate::session::format_expires_at;
use crate::state::{AppState, MAXIMUM_KEY_LENGTH, MINIMUM_KEY_LENGTH};
use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::SystemTime;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    #[serde(default)]
    key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetupRequest {
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    confirm_key: Option<String>,
}

/// POST /api/auth/login
pub async fn login(State(state): State<AppState>, Json(request): Json<LoginRequest>) -> Response {
    let invalid = || {
        (
            StatusCode::UNAUTHORIZED,
            Json(envelope(
                false,
                "management_key_invalid",
                "errors.unauthorized",
                None,
            )),
        )
            .into_response()
    };

    let Some(key) = request.key.as_deref() else {
        return invalid();
    };
    if key.is_empty() || key.len() > MAXIMUM_KEY_LENGTH || !state.auth.validate(key) {
        return invalid();
    }

    let (token, ttl) = state.sessions.create_session();
    let expires_at = format_expires_at(SystemTime::now(), ttl);
    // C# returns an anonymous object: { success, data: { token, expiresAt } }
    // (not a full ApiEnvelope — no code/messageKey).
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "data": { "token": token, "expiresAt": expires_at }
        })),
    )
        .into_response()
}

/// POST /api/auth/logout — session auth already verified by middleware.
pub async fn logout(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    let token = extract_bearer(&headers);
    state.sessions.revoke(&token);
    (StatusCode::OK, Json(json!({ "success": true }))).into_response()
}

/// POST /api/auth/sse-ticket
pub async fn sse_ticket(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    let token = extract_bearer(&headers);
    match state.sessions.create_sse_ticket(&token) {
        Some((ticket, ttl)) => {
            let expires_at = format_expires_at(SystemTime::now(), ttl);
            (
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "data": { "ticket": ticket, "expiresAt": expires_at }
                })),
            )
                .into_response()
        }
        None => (
            StatusCode::UNAUTHORIZED,
            Json(envelope(false, "unauthorized", "errors.unauthorized", None)),
        )
            .into_response(),
    }
}

/// GET /api/setup/status
pub async fn setup_status(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
) -> Response {
    let host = headers.get("host").and_then(|v| v.to_str().ok());
    let allowed = is_setup_allowed(peer.ip(), host, &headers);
    (
        StatusCode::OK,
        Json(json!({
            "setupRequired": state.auth.setup_required(),
            "environmentKeyConfigured": state.auth.environment_key_configured(),
            "setupAllowedFromThisRequest": allowed,
        })),
    )
        .into_response()
}

/// POST /api/setup — in env-key deployments setup is already configured.
pub async fn setup(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    Json(request): Json<SetupRequest>,
) -> Response {
    let allowed = is_setup_allowed(
        peer.ip(),
        headers.get("host").and_then(|v| v.to_str().ok()),
        &headers,
    );
    if !allowed {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "private_network_required" })),
        )
            .into_response();
    }
    let key = request.key.as_deref().unwrap_or("");
    if key.len() < MINIMUM_KEY_LENGTH {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "key_too_short", "minimumLength": MINIMUM_KEY_LENGTH })),
        )
            .into_response();
    }
    if key.len() > MAXIMUM_KEY_LENGTH {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "key_too_long", "maximumLength": MAXIMUM_KEY_LENGTH })),
        )
            .into_response();
    }
    if request.confirm_key.as_deref() != Some(key) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "keys_do_not_match" })),
        )
            .into_response();
    }
    if !state.auth.setup_required() {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "already_configured" })),
        )
            .into_response();
    }
    // No persistent key store in the headless build: setup is only reachable
    // when no environment key exists, which the Termux deployment never uses.
    (StatusCode::INTERNAL_SERVER_ERROR, Json(Value::Null)).into_response()
}
