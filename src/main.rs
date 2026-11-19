//! Minimal headless WebAPI skeleton for Termux.
//!
//! Contract target: v2rayN.WebAPI (pinned in oracle.lock).
//! The health endpoint deliberately mirrors WebHealthEndpoint.cs exactly:
//! body `{"status":"ok"}` plus loopback-only diagnostic headers.

use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::{Json, Router, routing::get};
use serde::Serialize;
use std::net::SocketAddr;

const VERSION: &str = env!("CARGO_PKG_VERSION");

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

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/version", get(version));

    let bind: SocketAddr = "127.0.0.1:5080"
        .parse()
        .expect("loopback bind address must be valid");
    tracing::info!(%bind, version = VERSION, "v2rayN WebAPI (Rust) starting");

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .expect("failed to bind loopback 5080");
    axum::serve(listener, app).await.expect("server failed");
}

/// Mirrors `WebHealthEndpoint.CreateResponse`.
///
/// The C# original emits diagnostic headers only for a local, same-origin,
/// non-forwarded request. This skeleton does not yet implement that policy:
/// it emits the process/version headers unconditionally, and leaves the
/// core-state fields empty until the runtime layer exists. Both are tracked
/// as TODOs in contract.yaml and will be closed by the differential test.
async fn health(headers: HeaderMap) -> impl IntoResponse {
    let mut response = (Json(HealthResponse { status: "ok" })).into_response();
    let h = response.headers_mut();
    h.insert(
        "X-v2rayn-WebAPI-instance-pid",
        std::process::id()
            .to_string()
            .parse()
            .expect("pid is a valid header value"),
    );
    h.insert(
        "X-v2rayn-WebAPI-version",
        VERSION.parse().expect("version is a valid header value"),
    );
    h.insert("X-v2rayn-WebAPI-core-state", "stopped".parse().unwrap());
    h.insert("X-v2rayn-WebAPI-core-process-ids", "".parse().unwrap());
    h.insert("X-v2rayn-WebAPI-core-profile-id", "".parse().unwrap());
    // Keep `headers` consumed so the signature documents that policy input
    // will arrive here once WebSetupAccessPolicy is ported.
    let _ = headers;
    response
}

async fn version() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "version": VERSION }))
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
