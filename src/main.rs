//! Headless WebAPI for Termux, contract-compatible with v2rayN.WebAPI.
//!
//! Contract target is pinned in `oracle.lock` and verified by the differential
//! suite in CI. Every request flows through `security::security_middleware`,
//! a faithful port of the C# middleware chain (security headers → body limit →
//! origin guard → rate limit → session auth).

mod auth;
mod security;
mod session;
mod state;

use axum::extract::ConnectInfo;
use axum::http::{HeaderName, StatusCode};
use axum::response::IntoResponse;
use axum::{Json, Router, routing::get, routing::post};
use security::{is_local_diagnostic_source, security_middleware};
use serde::Serialize;
use state::AppState;
use std::net::SocketAddr;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_BIND: &str = "127.0.0.1:5080";

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let bind = resolve_bind();
    let state = AppState::from_environment();

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/setup/status", get(auth::setup_status))
        .route("/api/setup", post(auth::setup))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/sse-ticket", post(auth::sse_ticket))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security_middleware,
        ))
        .with_state(state);

    tracing::info!(%bind, version = VERSION, "v2rayN WebAPI (Rust) starting");

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .expect("failed to bind loopback listener");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("server failed");
}

/// Parse the subset of `ASPNETCORE_URLS` the C# listener accepts that matters
/// here: a single `http://HOST:PORT` loopback URL (the default `127.0.0.1:5080`
/// is used otherwise). Multiple URLs / https are out of scope for this
/// milestone and fall back to the default rather than binding something unsafe.
fn resolve_bind() -> SocketAddr {
    let Some(raw) = std::env::var("ASPNETCORE_URLS").ok() else {
        return DEFAULT_BIND.parse().expect("default bind is valid");
    };
    let url = raw.split(';').next().unwrap_or_default().trim();
    let rest = url.strip_prefix("http://").unwrap_or_default();
    match rest.parse::<SocketAddr>() {
        Ok(addr) if addr.ip().is_loopback() => addr,
        _ => DEFAULT_BIND.parse().expect("default bind is valid"),
    }
}

/// Mirrors `WebHealthEndpoint.CreateResponse`.
async fn health(
    axum::extract::State(state): axum::extract::State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let host = headers.get("host").and_then(|v| v.to_str().ok());
    let diagnostic = is_local_diagnostic_source(peer.ip(), host, &headers);
    let mut response = (StatusCode::OK, Json(HealthResponse { status: "ok" })).into_response();

    if diagnostic {
        let h = response.headers_mut();
        let _ = h.insert(
            HeaderName::from_static("x-v2rayn-webapi-instance-pid"),
            state.instance_pid.to_string().parse().unwrap(),
        );
        let _ = h.insert(
            HeaderName::from_static("x-v2rayn-webapi-version"),
            VERSION.parse().unwrap(),
        );
        let _ = h.insert(
            HeaderName::from_static("x-v2rayn-webapi-core-state"),
            "stopped".parse().unwrap(),
        );
        let _ = h.insert(
            HeaderName::from_static("x-v2rayn-webapi-core-process-ids"),
            "".parse().unwrap(),
        );
        let _ = h.insert(
            HeaderName::from_static("x-v2rayn-webapi-core-profile-id"),
            "".parse().unwrap(),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_body_shape_is_status_ok() {
        let value = serde_json::to_value(HealthResponse { status: "ok" }).unwrap();
        assert_eq!(value, serde_json::json!({"status": "ok"}));
    }

    #[test]
    fn version_is_semver() {
        let parts: Vec<_> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "version must be MAJOR.MINOR.PATCH");
    }
}
