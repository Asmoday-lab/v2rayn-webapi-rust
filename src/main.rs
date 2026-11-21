//! Headless WebAPI for Termux, contract-compatible with v2rayN.WebAPI.
//!
//! Contract target is pinned in `oracle.lock` and verified by the differential
//! suite in CI. Every request flows through `security::security_middleware`,
//! a faithful port of the C# middleware chain (security headers → body limit →
//! origin guard → rate limit → session auth).

mod auth;
mod core;
mod crud;
mod endpoints;
mod events;
mod mutations;
mod security;
mod session;
mod settings_crud;
mod state;
mod static_files;
mod store;
mod validation;

use axum::extract::ConnectInfo;
use axum::http::{HeaderName, StatusCode};
use axum::response::IntoResponse;
use axum::{
    Json, Router,
    routing::{delete, get, post, put},
};
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
        .route("/api/status", get(endpoints::status_handler))
        .route("/api/setup/status", get(auth::setup_status))
        .route("/api/setup", post(auth::setup))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/sse-ticket", post(auth::sse_ticket))
        // Static/near-static GET endpoints (data captured from the pinned oracle).
        .route("/api/operations", get(endpoints::operations))
        .route("/api/profile-groups", get(endpoints::profile_groups))
        .route("/api/editor-options", get(endpoints::editor_options))
        .route("/api/subscriptions", get(endpoints::subscriptions))
        .route("/api/profiles", get(endpoints::profiles))
        .route("/api/profiles/{indexId}", get(crud::profile_get))
        .route("/api/settings", get(endpoints::settings))
        .route("/api/settings/webdav", get(endpoints::webdav_settings))
        .route("/api/settings/dns/simple", get(endpoints::dns_simple))
        .route("/api/settings/dns/profiles", get(endpoints::dns_profiles))
        .route(
            "/api/settings/dns/editor-options",
            get(endpoints::dns_editor_options),
        )
        .route("/api/settings/dns/defaults", get(endpoints::dns_defaults))
        .route(
            "/api/settings/routing-profiles",
            get(endpoints::routing_profiles),
        )
        .route(
            "/api/settings/core-templates",
            get(endpoints::core_templates),
        )
        .route("/api/core-updates", get(endpoints::core_updates))
        .route(
            "/api/core-updates/progress",
            get(endpoints::core_update_progress),
        )
        .route("/api/web-updates", get(endpoints::web_updates))
        .route("/api/logs/page", get(endpoints::logs_page))
        .route("/api/events", get(events::events))
        // Deterministic write operations on a fresh/headless install.
        .route("/api/logs", delete(mutations::clear_logs))
        .route("/api/statistics", delete(mutations::clear_statistics))
        .route("/api/speedtests", delete(mutations::stop_speedtests))
        .route("/api/latency/stop", post(mutations::stop_latency))
        .route("/api/core/stop", post(core::core_stop))
        .route(
            "/api/profiles/deduplicate",
            post(mutations::profiles_deduplicate),
        )
        .route(
            "/api/profiles/invalid-test-results",
            delete(mutations::profiles_clear_invalid),
        )
        .route(
            "/api/profile-groups/current",
            put(mutations::set_current_group),
        )
        .route(
            "/api/profile-groups/generate/all",
            post(mutations::generate_all_groups),
        )
        .route(
            "/api/settings/routing-profiles/import",
            post(mutations::routing_import),
        )
        .route(
            "/api/subscriptions/update",
            post(mutations::subscription_update),
        )
        .route("/api/profiles/{id}/select", post(crud::profile_select))
        // Deterministic validation failures (no Core/IO side effects).
        .route("/api/profiles/import", post(validation::profile_import))
        .route(
            "/api/settings/routing-profiles/{id}/rules/import",
            post(validation::routing_rules_import),
        )
        .route("/api/settings/routing", put(settings_crud::routing))
        .route("/api/settings/application", put(settings_crud::application))
        .route("/api/settings/inbound", put(settings_crud::inbound))
        .route("/api/settings/speedtest", put(settings_crud::speedtest))
        .route("/api/settings/core", put(settings_crud::core))
        .route("/api/settings/core-types", put(settings_crud::core_types))
        .route("/api/settings/apply", put(settings_crud::apply))
        .route("/api/settings/dns/simple", put(settings_crud::dns_simple))
        .route("/api/settings/webdav", put(settings_crud::webdav))
        .route(
            "/api/settings/dns/profiles/{coreType}",
            put(settings_crud::dns_profile_upsert),
        )
        .route("/api/subscriptions", post(crud::subscription_create))
        .route(
            "/api/subscriptions/{id}",
            put(crud::subscription_update).delete(crud::subscription_delete),
        )
        .route("/api/profiles/sort", post(validation::profiles_sort))
        .route(
            "/api/profiles",
            post(crud::profile_create).delete(crud::profile_delete),
        )
        .route("/api/profiles/{indexId}", put(crud::profile_update))
        .route(
            "/api/settings/routing-profiles/{id}/rules/import-url",
            post(validation::routing_rules_import_url),
        )
        .route(
            "/api/profile-groups/generate/regions",
            post(validation::generate_regions_groups),
        )
        .route("/api/profiles/copy", post(validation::profiles_copy))
        .route("/api/speedtests", post(validation::speedtests_start))
        .route("/api/core/start", post(core::core_start))
        .route("/api/core/restart", post(core::core_restart))
        .route("/api/core/status", get(core::core_status))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security_middleware,
        ))
        .with_state(state);

    // Optional same-origin Nova hosting. Without it an HTTPS-served Nova build
    // cannot call this loopback HTTP API (mixed content / PNA).
    let app = match std::env::var_os("NOVA_DIST_DIR").map(std::path::PathBuf::from) {
        Some(root) if root.is_dir() => {
            tracing::info!(path = %root.display(), "serving Nova SPA from NOVA_DIST_DIR");
            app.fallback({
                let root = root.clone();
                move |uri: axum::http::Uri| {
                    let root = root.clone();
                    async move { static_files::serve(&root, uri).await }
                }
            })
        }
        _ => app,
    };

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
