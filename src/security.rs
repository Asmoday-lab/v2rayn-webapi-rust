//! Security policy primitives and the request middleware chain.
//!
//! Ported 1:1 from the pinned C# oracle:
//!   Security/WebSetupAccessPolicy.cs
//!   Security/WebCorsPolicy.cs
//!   Security/WebSecurityHeadersMiddleware.cs
//!   Security/WebAuthRequestBodyLimitMiddleware.cs
//!   Security/WebOriginGuardMiddleware.cs
//!   Security/WebSessionAuthenticationMiddleware.cs
//!   Program.cs (middleware registration order)

use crate::state::AppState;
use axum::Json;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Fields emitted as diagnostic header names by the C# health endpoint.
/// Documented contract surface; the differential harness asserts this set.
#[allow(dead_code)]
pub const DIAGNOSTIC_HEADERS: &[&str] = &[
    "x-v2rayn-webapi-instance-pid",
    "x-v2rayn-webapi-version",
    "x-v2rayn-webapi-core-state",
    "x-v2rayn-webapi-core-process-ids",
    "x-v2rayn-webapi-core-profile-id",
];

/// Exact `WebSecurityHeadersMiddleware.ContentSecurityPolicy`.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'none'; \
     form-action 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
     img-src 'self'; font-src 'self'; connect-src 'self'";

/// `WebAuthRequestBodyLimitMiddleware.MaximumRequestBodyBytes` = 64 KiB.
pub const MAX_AUTH_BODY_BYTES: u64 = 64 * 1024;

/// Paths exempt from session authentication (WebSessionAuthenticationMiddleware.cs:25-33).
const AUTH_EXEMPT: &[&str] = &[
    "/api/health",
    "/api/setup/status",
    "/api/setup",
    "/api/auth/login",
];

/// Build the standard C# `ApiEnvelope<T>` shape (JsonSerializerDefaults.Web → camelCase,
/// null `data` retained).
pub fn envelope(success: bool, code: &str, message_key: &str, data: Option<Value>) -> Value {
    json!({"success": success, "code": code, "messageKey": message_key, "data": data})
}

pub fn fail_response(status: StatusCode, code: &str, message_key: &str) -> Response {
    (status, Json(envelope(false, code, message_key, None))).into_response()
}

/// The unified middleware chain, ordered exactly like Program.cs:249-256:
/// security headers → body limit → origin guard → (CORS/rate-limit) → session auth.
/// Security headers are applied to every response, including early rejections.
pub async fn security_middleware(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let headers = request.headers().clone();
    let method = request.method().clone();

    // 1. Body size limit on POST /api/auth/login and POST /api/setup.
    if method == axum::http::Method::POST
        && (path == "/api/auth/login" || path == "/api/setup")
        && request
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|n| n > MAX_AUTH_BODY_BYTES)
    {
        let mut response = fail_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "errors.invalidInput",
        );
        add_security_headers(&mut response);
        return response;
    }

    // 2. Origin guard — before rate limiting AND before session auth.
    if path.starts_with("/api") && !origin_allowed(&state, &headers) {
        let mut response = fail_response(
            StatusCode::FORBIDDEN,
            "origin_not_allowed",
            "errors.invalidInput",
        );
        add_security_headers(&mut response);
        return response;
    }

    // 3. Rate limiting on login/setup (fixed 1-minute window, per peer partition).
    if (path == "/api/auth/login" || path == "/api/setup")
        && method == axum::http::Method::POST
        && !state.rate_limiter.allow(&partition_key(peer.ip()), &path)
    {
        let mut response = Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header("retry-after", "60")
            .body(axum::body::Body::empty())
            .unwrap();
        add_security_headers(&mut response);
        return response;
    }

    // 4. Session authentication. `/api/events` is special: it authenticates with
    // a one-shot SSE ticket from the query string, not a Bearer token
    // (WebSessionAuthenticationMiddleware.cs:10-23).
    if path == "/api/events" {
        let ticket = query_param(request.uri().query(), "sse_ticket").unwrap_or_default();
        if !state.sessions.consume_sse_ticket(ticket) {
            let mut response = fail_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "errors.unauthorized",
            );
            add_security_headers(&mut response);
            return response;
        }
    } else if path.starts_with("/api") && !AUTH_EXEMPT.contains(&path.as_str()) {
        let presented = extract_bearer(&headers);
        if !state.sessions.validate_and_renew(&presented) {
            let mut response = fail_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "errors.unauthorized",
            );
            add_security_headers(&mut response);
            return response;
        }
    }

    let mut response = next.run(request).await;
    add_security_headers(&mut response);
    response
}

fn add_security_headers(response: &mut Response) {
    let headers = response.headers_mut();
    let _ = headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    let _ = headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    let _ = headers.insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    let _ = headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
}

/// `WebSessionService.ExtractPresentedToken`: only a `Bearer ` prefix is accepted.
pub fn extract_bearer(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            if v.len() >= 7 && v[..7].eq_ignore_ascii_case("Bearer ") {
                Some(v[7..].trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_default()
}

/// Extract a single query parameter value (first occurrence) from a raw query.
fn query_param<'a>(query: Option<&'a str>, name: &str) -> Option<&'a str> {
    let query = query?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        if key == name { Some(value) } else { None }
    })
}

/// Mirror `WebOriginGuardMiddleware`: allow when the request is same-origin,
/// a browser same-origin request, or the single Origin is on the allowlist.
pub fn origin_allowed(state: &AppState, headers: &HeaderMap) -> bool {
    if is_same_origin(headers) || is_browser_same_origin(headers) {
        return true;
    }
    let origins: Vec<_> = headers.get_all("origin").iter().collect();
    if origins.len() != 1 {
        return false;
    }
    match origins[0].to_str() {
        Ok(origin) => state.allowed_origins.iter().any(|o| o == origin),
        Err(_) => false,
    }
}

/// `WebCorsPolicy.IsSameOriginRequest` for an HTTP listener.
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

/// `WebCorsPolicy.IsBrowserSameOriginRequest`: JS cannot forge Sec-Fetch-Site.
fn is_browser_same_origin(headers: &HeaderMap) -> bool {
    let sec_fetch: Vec<_> = headers.get_all("sec-fetch-site").iter().collect();
    let origins: Vec<_> = headers.get_all("origin").iter().collect();
    if sec_fetch.len() != 1
        || sec_fetch[0].to_str().ok() != Some("same-origin")
        || origins.len() != 1
    {
        return false;
    }
    origins[0]
        .to_str()
        .ok()
        .is_some_and(|o| normalize_origin(o).as_deref() == Some(o))
}

/// `WebCorsPolicy.NormalizeOrigin`: validate + reduce to scheme://authority.
fn normalize_origin(value: &str) -> Option<String> {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return None;
    }
    let scheme_end = value.find("://")?;
    let scheme = &value[..scheme_end];
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority_start = scheme_end + 3;
    let rest = &value[authority_start..];
    let authority = rest.split('/').next().unwrap_or(rest);
    if authority.is_empty() || authority.contains(['*', '\\', '?', '#', '@']) {
        return None;
    }
    if rest.len() != authority.len() && !rest[authority.len()..].starts_with('/') {
        return None;
    }
    // Only a trailing "/" is allowed after the authority.
    if rest.len() > authority.len() && &rest[authority.len()..] != "/" {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (authority, None),
    };
    if host.is_empty() {
        return None;
    }
    Some(match port {
        Some(p) => format!("{scheme}://{host}:{p}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Strip a port from an HTTP Host header value, handling bracketed IPv6.
pub fn host_name(host_header: &str) -> &str {
    let value = host_header.trim();
    if let Some(rest) = value.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match value.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
        _ => value,
    }
}

/// Exact loopback test from WebSetupAccessPolicy.IsLoopbackAddress.
pub fn is_loopback_address(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => v4 == Ipv4Addr::LOCALHOST,
        IpAddr::V6(v6) => {
            v6 == Ipv6Addr::LOCALHOST || v6.to_ipv4_mapped() == Some(Ipv4Addr::LOCALHOST)
        }
    }
}

pub fn is_local_host(host_header: &str) -> bool {
    let name = host_name(host_header);
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match name.parse::<IpAddr>() {
        Ok(addr) => is_loopback_address(addr),
        Err(_) => false,
    }
}

pub fn has_forwarded_headers(headers: &HeaderMap) -> bool {
    headers.keys().any(|key| {
        let name = key.as_str();
        name.eq_ignore_ascii_case("forwarded")
            || name.eq_ignore_ascii_case("x-real-ip")
            || name.to_ascii_lowercase().starts_with("x-forwarded-")
    })
}

/// `WebSetupAccessPolicy.IsAllowedFromRequest`.
pub fn is_setup_allowed(peer: IpAddr, host: Option<&str>, headers: &HeaderMap) -> bool {
    let Some(host) = host else {
        return false;
    };
    is_same_origin(headers)
        && !has_forwarded_headers(headers)
        && is_loopback_address(peer)
        && is_local_host(host)
}

/// `WebHealthEndpoint.CreateResponse` gating condition.
pub fn is_local_diagnostic_source(peer: IpAddr, host: Option<&str>, headers: &HeaderMap) -> bool {
    let Some(host) = host else {
        return false;
    };
    is_loopback_address(peer)
        && is_local_host(host)
        && is_same_origin(headers)
        && !has_forwarded_headers(headers)
}

fn partition_key(ip: IpAddr) -> String {
    if is_loopback_address(ip) {
        "loopback".to_string()
    } else {
        ip.to_string()
    }
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
        let plain = headers(&[("host", "127.0.0.1:5080")]);
        assert!(is_local_diagnostic_source(
            peer,
            Some("127.0.0.1:5080"),
            &plain
        ));
        let same_origin = headers(&[
            ("host", "127.0.0.1:5080"),
            ("origin", "http://127.0.0.1:5080"),
        ]);
        assert!(is_local_diagnostic_source(
            peer,
            Some("127.0.0.1:5080"),
            &same_origin
        ));
        let foreign = headers(&[
            ("host", "127.0.0.1:5080"),
            ("origin", "https://evil.example"),
        ]);
        assert!(!is_local_diagnostic_source(
            peer,
            Some("127.0.0.1:5080"),
            &foreign
        ));
        let remote: IpAddr = "10.0.0.5".parse().unwrap();
        assert!(!is_local_diagnostic_source(
            remote,
            Some("127.0.0.1:5080"),
            &plain
        ));
        assert!(!is_local_diagnostic_source(peer, None, &plain));
    }

    #[test]
    fn origin_guard_allows_native_and_same_origin() {
        let state_allowed = AppState::for_tests(&[]);
        assert!(origin_allowed(
            &state_allowed,
            &headers(&[("host", "127.0.0.1:5080")])
        ));
        assert!(origin_allowed(
            &state_allowed,
            &headers(&[
                ("host", "127.0.0.1:5080"),
                ("origin", "http://127.0.0.1:5080"),
            ])
        ));
        assert!(!origin_allowed(
            &state_allowed,
            &headers(&[
                ("host", "127.0.0.1:5080"),
                ("origin", "https://evil.example"),
            ])
        ));
    }

    #[test]
    fn origin_guard_uses_allowlist() {
        let state = AppState::for_tests(&["https://webui.example".to_string()]);
        assert!(origin_allowed(
            &state,
            &headers(&[
                ("host", "127.0.0.1:5080"),
                ("origin", "https://webui.example"),
            ])
        ));
    }

    #[test]
    fn browser_same_origin_requires_fetch_metadata() {
        let h = headers(&[
            ("host", "internal:5080"),
            ("origin", "https://public.example"),
            ("sec-fetch-site", "same-origin"),
        ]);
        assert!(is_browser_same_origin(&h));
        let h2 = headers(&[
            ("host", "internal:5080"),
            ("origin", "https://public.example"),
        ]);
        assert!(!is_browser_same_origin(&h2));
    }

    #[test]
    fn normalize_origin_rejects_paths_and_wildcards() {
        assert_eq!(
            normalize_origin("https://example.com:8443"),
            Some("https://example.com:8443".to_string())
        );
        assert_eq!(
            normalize_origin("https://example.com/"),
            Some("https://example.com".to_string())
        );
        assert_eq!(normalize_origin("https://example.com/path"), None);
        assert_eq!(normalize_origin("https://*.example.com"), None);
        assert_eq!(normalize_origin("ftp://example.com"), None);
    }

    #[test]
    fn bearer_extraction_is_case_insensitive() {
        let h = headers(&[("authorization", "bearer abc.def")]);
        assert_eq!(extract_bearer(&h), "abc.def");
        let h2 = headers(&[("authorization", "Basic abc")]);
        assert_eq!(extract_bearer(&h2), "");
    }
}
