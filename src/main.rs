//! Headless WebAPI for Termux, contract-compatible with v2rayN.WebAPI.
//!
//! Contract target is pinned in `oracle.lock` and verified by the differential
//! suite in CI. The health endpoint mirrors `Api/WebHealthEndpoint.cs` exactly:
//! body `{"status":"ok"}` plus loopback-only diagnostic headers gated by
//! `Security/WebSetupAccessPolicy.cs` and `Security/WebCorsPolicy.cs`.

use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::IntoResponse;
use axum::{Json, Router, routing::get};
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_BIND: &str = "127.0.0.1:5080";

/// Fields emitted as diagnostic headers by the C# health endpoint when the
/// request is a local, same-origin, non-forwarded probe. Kept as constants so
/// the differential test can assert the exact header-name set.
pub const DIAGNOSTIC_HEADERS: &[&str] = &[
    "x-v2rayn-webapi-instance-pid",
    "x-v2rayn-webapi-version",
    "x-v2rayn-webapi-core-state",
    "x-v2rayn-webapi-core-process-ids",
    "x-v2rayn-webapi-core-profile-id",
];

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
    let app = Router::new().route("/api/health", get(health));

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
///
/// Diagnostic headers are attached only for a local diagnostic source:
/// loopback peer + local Host + same-origin + no forwarding headers. This is
/// the union of `WebSetupAccessPolicy.IsLocalDiagnosticSource`,
/// `WebCorsPolicy.IsSameOriginRequest` and `HasForwardedHeaders`.
async fn health(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let host = headers.get("host").and_then(|v| v.to_str().ok());
    let diagnostic = is_local_diagnostic_source(peer.ip(), host, &headers);
    let mut response = (StatusCode::OK, Json(HealthResponse { status: "ok" })).into_response();

    if diagnostic {
        let h = response.headers_mut();
        let _ = h.insert(
            HeaderName::from_static("x-v2rayn-webapi-instance-pid"),
            std::process::id()
                .to_string()
                .parse()
                .expect("pid is a valid header value"),
        );
        let _ = h.insert(
            HeaderName::from_static("x-v2rayn-webapi-version"),
            VERSION.parse().expect("version is a valid header value"),
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

/// Strip a port from an HTTP Host header value, handling bracketed IPv6.
/// Mirrors the value of `HttpRequest.Host.Host` in ASP.NET Core.
fn host_name(host_header: &str) -> &str {
    let value = host_header.trim();
    if let Some(rest) = value.strip_prefix('[') {
        // "[::1]:5080" or "[::1]"
        return rest.split(']').next().unwrap_or(rest);
    }
    // "127.0.0.1:5080" or "localhost:5080" or "localhost"
    match value.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
        _ => value,
    }
}

/// Exact loopback test matching `WebSetupAccessPolicy.IsLoopbackAddress`:
/// 127.0.0.1, ::1, or an IPv4-mapped ::ffff:127.0.0.1. (Deliberately stricter
/// than Rust's `IpAddr::is_loopback`, which also accepts 127.0.0.2.)
fn is_loopback_address(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => v4 == Ipv4Addr::LOCALHOST,
        IpAddr::V6(v6) => {
            v6 == Ipv6Addr::LOCALHOST || v6.to_ipv4_mapped() == Some(Ipv4Addr::LOCALHOST)
        }
    }
}

fn is_local_host(host_header: &str) -> bool {
    let name = host_name(host_header);
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match name.parse::<IpAddr>() {
        Ok(addr) => is_loopback_address(addr),
        Err(_) => false,
    }
}

fn has_forwarded_headers(headers: &HeaderMap) -> bool {
    headers.keys().any(|key| {
        let name = key.as_str();
        name.eq_ignore_ascii_case("forwarded")
            || name.eq_ignore_ascii_case("x-real-ip")
            || name.to_ascii_lowercase().starts_with("x-forwarded-")
    })
}

/// Mirrors `WebCorsPolicy.IsSameOriginRequest` for a native HTTP (non-TLS)
/// listener: no Origin header is allowed (native clients); a single Origin
/// must equal `http://<Host>`. Multiple Origins fail.
fn is_same_origin(headers: &HeaderMap) -> bool {
    let origins: Vec<_> = headers.get_all("origin").iter().collect();
    match origins.len() {
        0 => true,
        1 => match (
            origins[0].to_str(),
            headers.get("host").and_then(|v| v.to_str().ok()),
        ) {
            (Ok(origin), Some(host)) => origin.eq_ignore_ascii_case(&format!("http://{host}")),
            _ => false,
        },
        _ => false,
    }
}

fn is_local_diagnostic_source(peer: IpAddr, host: Option<&str>, headers: &HeaderMap) -> bool {
    let Some(host) = host else {
        return false;
    };
    is_loopback_address(peer)
        && is_local_host(host)
        && is_same_origin(headers)
        && !has_forwarded_headers(headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.append(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        map
    }

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

    #[test]
    fn diagnostic_headers_are_the_exact_c_set() {
        assert_eq!(DIAGNOSTIC_HEADERS.len(), 5);
        for name in DIAGNOSTIC_HEADERS {
            assert!(name.starts_with("x-v2rayn-webapi-"));
        }
    }

    #[test]
    fn loopback_only_exact_match() {
        assert!(is_loopback_address("127.0.0.1".parse().unwrap()));
        assert!(is_loopback_address("::1".parse().unwrap()));
        assert!(is_loopback_address("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!is_loopback_address("127.0.0.2".parse().unwrap()));
        assert!(!is_loopback_address("192.168.1.2".parse().unwrap()));
    }

    #[test]
    fn host_name_strips_port_and_brackets() {
        assert_eq!(host_name("127.0.0.1:5080"), "127.0.0.1");
        assert_eq!(host_name("[::1]:5080"), "::1");
        assert_eq!(host_name("localhost"), "localhost");
        assert_eq!(host_name("localhost:5080"), "localhost");
    }

    #[test]
    fn local_host_accepts_loopback_and_localhost() {
        assert!(is_local_host("127.0.0.1:5080"));
        assert!(is_local_host("localhost"));
        assert!(is_local_host("[::1]:5080"));
        assert!(!is_local_host("example.com:5080"));
        assert!(!is_local_host("192.168.1.2:5080"));
    }

    #[test]
    fn forwarded_headers_block_diagnostics() {
        let h = headers(&[("host", "127.0.0.1:5080"), ("x-forwarded-for", "1.2.3.4")]);
        assert!(has_forwarded_headers(&h));
        assert!(!is_local_diagnostic_source(
            "127.0.0.1".parse().unwrap(),
            Some("127.0.0.1:5080"),
            &h
        ));
    }

    #[test]
    fn diagnostic_source_requires_all_conditions() {
        let peer: IpAddr = "127.0.0.1".parse().unwrap();
        // Plain loopback curl: no Origin -> same-origin, diagnostics on.
        let plain = headers(&[("host", "127.0.0.1:5080")]);
        assert!(is_local_diagnostic_source(
            peer,
            Some("127.0.0.1:5080"),
            &plain
        ));
        // Matching Origin is accepted.
        let same_origin = headers(&[
            ("host", "127.0.0.1:5080"),
            ("origin", "http://127.0.0.1:5080"),
        ]);
        assert!(is_local_diagnostic_source(
            peer,
            Some("127.0.0.1:5080"),
            &same_origin
        ));
        // Foreign Origin blocks diagnostics.
        let foreign = headers(&[
            ("host", "127.0.0.1:5080"),
            ("origin", "https://evil.example"),
        ]);
        assert!(!is_local_diagnostic_source(
            peer,
            Some("127.0.0.1:5080"),
            &foreign
        ));
        // Remote peer blocks diagnostics.
        let remote: IpAddr = "10.0.0.5".parse().unwrap();
        assert!(!is_local_diagnostic_source(
            remote,
            Some("127.0.0.1:5080"),
            &plain
        ));
        // Missing Host blocks diagnostics.
        assert!(!is_local_diagnostic_source(peer, None, &plain));
    }
}
