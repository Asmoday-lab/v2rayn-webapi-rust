//! Differential test harness skeleton.
//!
//! Design: the oracle (pinned C# v2rayN.WebAPI) and this Rust build are started
//! together on a x64 runner, the same request set is sent to both, and JSON
//! responses are compared after normalization (volatile fields stripped).
//!
//! Skeleton status: the transport/comparison scaffolding is here, but the
//! request corpus and oracle process management are still TODO. The test is
//! `#[ignore]` so it does not block the initial green CI; it will be unignored
//! and expanded when the runtime layer lands. See contract.yaml semantic_hotspots.

use std::fs;
use std::path::Path;

/// Fields whose values differ legitimately between two processes and must be
/// normalized before comparing responses.
const VOLATILE_FIELDS: &[&str] = &[
    "timestamp",
    "elapsed",
    "duration",
    "pid",
    "processId",
    "startedAt",
    "buildDate",
];

fn read_pin() -> (String, String) {
    let raw = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("oracle.lock"))
        .expect("oracle.lock must exist");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("valid oracle.lock JSON");
    let oracle = value["oracle"]["commit"]
        .as_str()
        .expect("oracle.commit")
        .to_string();
    let client = value["client"]["commit"]
        .as_str()
        .expect("client.commit")
        .to_string();
    (oracle, client)
}

#[test]
fn oracle_pin_is_present_and_pinned() {
    let (oracle, client) = read_pin();
    assert_eq!(oracle.len(), 40, "oracle commit must be a full SHA");
    assert_eq!(client.len(), 40, "client commit must be a full SHA");
    assert!(
        !oracle.chars().all(|c| c == '0'),
        "oracle commit must not be zero"
    );
}

#[test]
#[ignore = "oracle process management + request corpus not implemented yet"]
fn health_response_matches_oracle() {
    let (_oracle, _client) = read_pin();
    // TODO:
    //   1. start pinned C# WebAPI on :5081 with V2RAYN_WEB_API_KEY set
    //   2. start this Rust build on :5080 with the same key
    //   3. GET /api/health on both
    //   4. strip VOLATILE_FIELDS, normalize header casing
    //   5. assert body + header sets equal
}

/// Normalize a JSON value by removing volatile fields recursively.
#[allow(dead_code)]
fn normalize(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.retain(|key, _| !VOLATILE_FIELDS.contains(&key.as_str()));
            for v in map.values_mut() {
                normalize(v);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                normalize(item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_volatile_fields_recursively() {
        let mut value = serde_json::json!({
            "status": "ok",
            "timestamp": 123,
            "nested": { "elapsed": 5, "keep": "yes" },
            "list": [{ "pid": 42, "id": "x" }]
        });
        normalize(&mut value);
        assert_eq!(
            value,
            serde_json::json!({
                "status": "ok",
                "nested": { "keep": "yes" },
                "list": [{ "id": "x" }]
            })
        );
    }
}
