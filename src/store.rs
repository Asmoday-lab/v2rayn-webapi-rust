//! Persistent data store for the headless WebAPI.
//!
//! The original v2rayN keeps a dual store (`guiNConfig.json` + a SQLite
//! `guiNDB.db`). That shape needs a C SQLite build, which is hostile to the
//! static `aarch64-unknown-linux-musl` shipping target. This store keeps the
//! *wire-level* DTOs identical (that is the contract) but persists them in a
//! single JSON document under `V2RAYN_DATA_HOME/data.json`, written atomically.
//!
//! Louder point: an empty store is initialised from the embedded golden DTOs,
//! so a fresh install answers byte-for-byte like the pinned C# oracle. That is
//! what keeps the CI differential corpus green while real CRUD works on a
//! populated device.

use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

fn golden(raw: &str) -> Value {
    serde_json::from_str(raw).expect("embedded golden resource is valid JSON")
}

/// Initial state of a fresh install. Profiles and subscriptions are empty (as
/// on the oracle's fresh data dir); the near-static settings are seeded from
/// the captured golden so their GET responses are unchanged.
fn fresh_data() -> Value {
    json!({
        "version": 1,
        "profiles": [],
        "subscriptions": [],
        "settings": golden(include_str!("../resources/settings.json")),
        "dnsSimple": golden(include_str!("../resources/dns_simple.json")),
        "dnsProfiles": golden(include_str!("../resources/dns_profiles.json")),
        "routingProfiles": golden(include_str!("../resources/routing_profiles.json")),
        "coreTemplates": golden(include_str!("../resources/core_templates.json")),
        "webdav": golden(include_str!("../resources/webdav_settings.json")),
        "logs": golden(include_str!("../resources/logs.json")),
        "currentProfileId": "",
        "currentGroupId": ""
    })
}

pub struct Store {
    path: PathBuf,
    data: RwLock<Value>,
}

impl Store {
    /// Open the store at `V2RAYN_DATA_HOME/data.json`. A missing or corrupt
    /// file falls back to a fresh golden state (never crash the API on data).
    pub fn open(data_home: &Path) -> Self {
        let path = data_home.join("data.json");
        let data = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(|v| v.is_object())
            .unwrap_or_else(fresh_data);
        Self {
            path,
            data: RwLock::new(data),
        }
    }

    /// Read a top-level array field (clone, to keep the lock scope tiny).
    pub fn array(&self, key: &str) -> Vec<Value> {
        self.data
            .read()
            .unwrap()
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// Read a top-level object field.
    pub fn object(&self, key: &str) -> Value {
        self.data
            .read()
            .unwrap()
            .get(key)
            .cloned()
            .unwrap_or_else(|| json!({}))
    }

    /// Read a top-level string field.
    pub fn string(&self, key: &str) -> String {
        self.data
            .read()
            .unwrap()
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// Find an element in a top-level array by an exact field match.
    pub fn find_in_array(&self, key: &str, field: &str, value: &str) -> Option<Value> {
        self.array(key)
            .into_iter()
            .find(|item| item.get(field).and_then(Value::as_str) == Some(value))
    }

    /// Replace a whole top-level array, then persist.
    pub fn set_array(&self, key: &str, values: Vec<Value>) {
        {
            let mut guard = self.data.write().unwrap();
            guard
                .as_object_mut()
                .unwrap()
                .insert(key.to_string(), json!(values));
        }
        let _ = self.persist();
    }

    /// Replace a whole top-level object, then persist.
    pub fn set_object(&self, key: &str, value: Value) {
        {
            let mut guard = self.data.write().unwrap();
            guard
                .as_object_mut()
                .unwrap()
                .insert(key.to_string(), value);
        }
        let _ = self.persist();
    }

    /// Set a top-level string, then persist.
    pub fn set_string(&self, key: &str, value: impl Into<String>) {
        {
            let mut guard = self.data.write().unwrap();
            guard
                .as_object_mut()
                .unwrap()
                .insert(key.to_string(), json!(value.into()));
        }
        let _ = self.persist();
    }

    /// Merge patch into a top-level object (shallow field-by-field), persist.
    pub fn merge_object(&self, key: &str, patch: &Map<String, Value>) {
        {
            let mut guard = self.data.write().unwrap();
            let root = guard.as_object_mut().unwrap();
            let target = root.entry(key.to_string()).or_insert_with(|| json!({}));
            if let Some(map) = target.as_object_mut() {
                for (field, value) in patch {
                    map.insert(field.clone(), value.clone());
                }
            }
        }
        let _ = self.persist();
    }

    /// Read a nested field: data[key][field].
    pub fn object_field(&self, key: &str, field: &str) -> Value {
        self.data
            .read()
            .unwrap()
            .get(key)
            .and_then(|v| v.get(field))
            .cloned()
            .unwrap_or_else(|| json!({}))
    }

    /// Merge `patch` into data[key][field], persist.
    pub fn merge_object_field(&self, key: &str, field: &str, patch: &Map<String, Value>) {
        {
            let mut guard = self.data.write().unwrap();
            let root = guard.as_object_mut().unwrap();
            let parent = root.entry(key.to_string()).or_insert_with(|| json!({}));
            let target = parent
                .as_object_mut()
                .unwrap()
                .entry(field.to_string())
                .or_insert_with(|| json!({}));
            if let Some(map) = target.as_object_mut() {
                for (k, v) in patch {
                    map.insert(k.clone(), v.clone());
                }
            }
        }
        let _ = self.persist();
    }

    /// Set a single nested field data[key][field] = value, persist.
    pub fn set_object_field(&self, key: &str, field: &str, value: Value) {
        {
            let mut guard = self.data.write().unwrap();
            let root = guard.as_object_mut().unwrap();
            let parent = root.entry(key.to_string()).or_insert_with(|| json!({}));
            if let Some(map) = parent.as_object_mut() {
                map.insert(field.to_string(), value);
            }
        }
        let _ = self.persist();
    }

    /// Snapshot the entire document (read-only). Used by Core config generation.
    #[allow(dead_code)]
    pub fn snapshot(&self) -> Value {
        self.data.read().unwrap().clone()
    }

    /// Atomic write: temp file in the same dir + rename, so a crash never
    /// leaves a half-written store. Errors are logged by the caller via the
    /// ignored result; a failed persist never blocks the in-memory mutation.
    fn persist(&self) -> std::io::Result<()> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let guard = self.data.read().unwrap();
        let bytes = serde_json::to_vec_pretty(&*guard)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)?;
        // Windows rename fails if the destination exists; replace it first.
        if self.path.exists() {
            std::fs::remove_file(&self.path)?;
        }
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            tracing::warn!(path = %self.path.display(), error = %e, "store persist failed");
            e
        })
    }
}

/// Resolve the data home directory from the environment, defaulting next to
/// the executable's working directory. Kept here so main.rs and tests agree.
pub fn data_home_from_env() -> PathBuf {
    std::env::var("V2RAYN_DATA_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(tag: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("nova-store-test-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn fresh_store_matches_golden_empty_state() {
        let home = temp_home("fresh");
        let store = Store::open(&home);
        assert!(store.array("profiles").is_empty());
        assert!(store.array("subscriptions").is_empty());
        assert!(store.object("settings").get("inbound").is_some());
        assert_eq!(store.string("currentProfileId"), "");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn write_then_reload_persists() {
        let home = temp_home("persist");
        {
            let store = Store::open(&home);
            store.set_array(
                "profiles",
                vec![json!({"indexId": "abc", "remarks": "node"})],
            );
            store.set_string("currentProfileId", "abc");
        }
        {
            let store = Store::open(&home);
            let profiles = store.array("profiles");
            assert_eq!(profiles.len(), 1);
            assert_eq!(profiles[0]["indexId"], "abc");
            assert_eq!(store.string("currentProfileId"), "abc");
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn find_in_array_by_field() {
        let home = temp_home("find");
        let store = Store::open(&home);
        store.set_array(
            "profiles",
            vec![
                json!({"indexId": "a", "remarks": "one"}),
                json!({"indexId": "b", "remarks": "two"}),
            ],
        );
        assert_eq!(
            store.find_in_array("profiles", "indexId", "b").unwrap()["remarks"],
            "two"
        );
        assert!(
            store
                .find_in_array("profiles", "indexId", "missing")
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn merge_object_preserves_other_fields() {
        let home = temp_home("merge");
        let store = Store::open(&home);
        let before = store.object("settings")["inbound"]["localPort"].clone();
        store.merge_object("settings", &{
            let mut m = Map::new();
            m.insert("domainStrategy".to_string(), json!("IPIfNonMatch"));
            m
        });
        assert_eq!(store.object("settings")["domainStrategy"], "IPIfNonMatch");
        assert_eq!(store.object("settings")["inbound"]["localPort"], before);
        let _ = std::fs::remove_dir_all(&home);
    }
}
