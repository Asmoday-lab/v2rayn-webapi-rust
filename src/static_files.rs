//! Optional same-origin hosting of the v2rayN-WebUI-Nova build output.
//!
//! A Nova page served over HTTPS cannot call an http://127.0.0.1 API (mixed
//! content / Private Network Access). Serving the SPA from this process makes
//! `/api/*` same-origin and removes that whole class of browser problems.
//! Enabled only when `NOVA_DIST_DIR` points at a readable directory.

use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use std::path::{Path, PathBuf};

/// Reject absolute paths, traversal and backslashes before touching the FS.
fn resolve(root: &Path, relative: &str) -> Option<PathBuf> {
    let mut target = root.to_path_buf();
    for segment in relative.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." || segment.contains('\\') || segment.contains('\0') {
            return None;
        }
        target.push(segment);
    }
    Some(target)
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
    {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

async fn body(path: &Path) -> Option<Response> {
    let bytes = tokio::fs::read(path).await.ok()?;
    Some(
        (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, content_type(path))],
            bytes,
        )
            .into_response(),
    )
}

/// Serve a file from `root`, falling back to `index.html` for SPA routes.
pub async fn serve(root: &Path, uri: Uri) -> Response {
    let relative = uri.path().trim_start_matches('/');
    if let Some(target) = resolve(root, relative) {
        if target.is_file() {
            if let Some(response) = body(&target).await {
                return response;
            }
        }
    }
    let index = root.join("index.html");
    if index.is_file() {
        if let Some(response) = body(&index).await {
            return response;
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_is_rejected() {
        assert!(resolve(Path::new("/dist"), "a/b.js").is_some());
        assert!(resolve(Path::new("/dist"), "../secret").is_none());
        assert!(resolve(Path::new("/dist"), "a/../../secret").is_none());
        assert!(resolve(Path::new("/dist"), "a\\b").is_none());
    }

    #[test]
    fn mime_types_are_explicit() {
        assert_eq!(
            content_type(Path::new("a.js")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("a.svg")), "image/svg+xml");
        assert_eq!(
            content_type(Path::new("a.unknown")),
            "application/octet-stream"
        );
    }

    #[tokio::test]
    async fn spa_fallback_serves_index() {
        let dir = std::env::temp_dir().join(format!("nova-dist-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("index.html"), b"<html>ok</html>").unwrap();
        let resp = serve(&dir, Uri::from_static("/some/spa/route")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"<html>ok</html>");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
