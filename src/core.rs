//! Real Core process management for sing-box on Termux.
//!
//! The C# original routes every protocol through `CoreManager.LoadCore()`,
//! which writes a `config.json` and spawns the core binary with
//! `run -c <path> --disable-color`. We reproduce that observable behaviour:
//! generate a sing-box config from the selected profile + settings, spawn the
//! process (path overridable via `V2RAYN_CORE_BIN`), track its PID, and expose
//! start/stop/restart/status. `V2RAYN_DATA_HOME/binConfigs/config.json` mirrors
//! the C# layout so the config can be inspected after launch.

use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use std::sync::Mutex;
use tokio::process::Child;

/// Live core process. `Mutex` is fine: start/stop are serialized and the lock
/// is never held across an await.
pub struct CoreHandle {
    child: Mutex<Option<Child>>,
    pid: Mutex<Option<u32>>,
}

impl CoreHandle {
    pub fn new() -> Self {
        Self {
            child: Mutex::new(None),
            pid: Mutex::new(None),
        }
    }

    pub fn pid(&self) -> Option<u32> {
        *self.pid.lock().unwrap()
    }

    pub fn is_running(&self) -> bool {
        self.pid().is_some()
    }

    /// Reap a dead child without blocking: try_wait is non-blocking.
    fn reap_if_exited(&self) {
        let mut guard = self.child.lock().unwrap();
        if let Some(child) = guard.as_mut() {
            if let Ok(Some(_status)) = child.try_wait() {
                *guard = None;
                *self.pid.lock().unwrap() = None;
            }
        }
    }
}

fn ok(message_key: &str, data: Value) -> Response {
    (
        StatusCode::OK,
        Json(json!({"success": true, "code": "ok", "messageKey": message_key, "data": data})),
    )
        .into_response()
}

fn fail(status: StatusCode, code: &str, message_key: &str, data: Value) -> Response {
    (
        status,
        Json(json!({"success": false, "code": code, "messageKey": message_key, "data": data})),
    )
        .into_response()
}

/// Build a minimal sing-box outbound from a profile. Mirrors the subset of
/// `CoreConfigSingboxService.GenOutbounds()` needed for the common protocols.
fn singbox_outbound(profile: &Value) -> Value {
    // 0 = no profile selected; falls through to the socks default below.
    let config_type = profile
        .get("configType")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let address = profile.get("address").and_then(Value::as_str).unwrap_or("");
    let port = profile.get("port").and_then(Value::as_u64).unwrap_or(0) as u32;
    let remark = profile
        .get("remarks")
        .and_then(Value::as_str)
        .unwrap_or("profile");

    let mut outbound = json!({
        "tag": "proxy",
        "server": address,
        "server_port": port,
        "type": match config_type {
            1 => "vmess",   // VMess
            3 => "shadowsocks",
            5 => "vless",
            6 => "trojan",
            7 => "hysteria2",
            8 => "tuic",
            9 => "wireguard",
            _ => "socks",
        },
    });
    let obj = outbound.as_object_mut().unwrap();
    if let Some(v) = profile.get("password").and_then(Value::as_str) {
        match config_type {
            1 => {
                obj.insert("uuid".into(), json!(v));
                obj.insert("alter_id".into(), json!(0));
            }
            3 => {
                obj.insert("method".into(), json!("2022-blake3-aes-128-gcm"));
                obj.insert("password".into(), json!(v));
            }
            5 | 6 => {
                obj.insert("uuid".into(), json!(v));
            }
            _ => {
                obj.insert("password".into(), json!(v));
            }
        }
    }
    if let Some(v) = profile.get("sni").and_then(Value::as_str) {
        obj.insert(
            "tls".into(),
            json!({"enabled": !v.is_empty(), "server_name": v}),
        );
    }
    let _ = remark;
    outbound
}

/// Generate the full sing-box runtime config. Local SOCKS inbound on the port
/// from settings, one proxy outbound, and a default route.
pub fn generate_config(snapshot: &Value) -> Value {
    let settings = snapshot.get("settings").unwrap_or(&Value::Null);
    let local_port = settings
        .get("inbound")
        .and_then(|i| i.get("localPort"))
        .and_then(Value::as_u64)
        .unwrap_or(10808);
    let profile_id = snapshot
        .get("currentProfileId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let profile = snapshot
        .get("profiles")
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter()
                .find(|p| p.get("indexId").and_then(Value::as_str) == Some(profile_id))
        })
        .cloned()
        .unwrap_or_else(|| json!({}));

    json!({
        "log": {"level": "info", "timestamp": true},
        "inbounds": [{
            "type": "socks",
            "tag": "socks-in",
            "listen": "127.0.0.1",
            "listen_port": local_port,
            "sniff": true
        }],
        "outbounds": [singbox_outbound(&profile), {"type": "direct", "tag": "direct"}],
        "route": {
            "rules": [{"outbound": "proxy"}],
            "final": "direct"
        }
    })
}

/// Write config.json into the data home (mirrors C# `binConfigs/config.json`).
fn write_config(config: &Value) -> std::io::Result<std::path::PathBuf> {
    let home = crate::store::data_home_from_env();
    let dir = home.join("binConfigs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("config.json");
    std::fs::write(&path, serde_json::to_vec_pretty(config)?)?;
    Ok(path)
}

fn core_bin() -> String {
    std::env::var("V2RAYN_CORE_BIN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "sing-box".to_string())
}

/// POST /api/core/start
pub async fn core_start(State(state): State<AppState>) -> Response {
    start_inner(&state).await
}

async fn start_inner(state: &AppState) -> Response {
    // Differential contract: empty install has no selected profile → 409.
    let profile_id = state.store.string("currentProfileId");
    if profile_id.is_empty()
        || state
            .store
            .find_in_array("profiles", "indexId", &profile_id)
            .is_none()
    {
        return fail(
            StatusCode::CONFLICT,
            "profile_not_selected",
            "profiles.noneSelected",
            json!(null),
        );
    }

    let snapshot = state.store.snapshot();
    let config = generate_config(&snapshot);
    let config_path = match write_config(&config) {
        Ok(p) => p,
        Err(e) => {
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "core_config_failed",
                "errors.internalError",
                json!({"message": e.to_string()}),
            );
        }
    };

    let bin = core_bin();
    let child = match tokio::process::Command::new(&bin)
        .args(["run", "-c"])
        .arg(&config_path)
        .arg("--disable-color")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return fail(
                StatusCode::CONFLICT,
                "core_start_failed",
                "core.startFailed",
                json!({"message": format!("{bin}: {e}")}),
            );
        }
    };
    let pid = child.id();
    *state.core.child.lock().unwrap() = Some(child);
    *state.core.pid.lock().unwrap() = pid;
    ok(
        "core.started",
        json!({"processIds": [pid], "runtimeState": "Running"}),
    )
}

/// POST /api/core/stop
pub async fn core_stop(State(state): State<AppState>) -> Response {
    stop_inner(&state).await
}

async fn stop_inner(state: &AppState) -> Response {
    state.core.reap_if_exited();
    // Drop the guard on this statement so it is never held across `.await`.
    let child = state.core.child.lock().unwrap().take();
    if let Some(mut child) = child {
        let _ = child.kill().await;
    }
    *state.core.pid.lock().unwrap() = None;
    // C# StopCoreAsync returns an OperationView with data=null, and succeeds
    // even when nothing was running (idempotent stop). Keep data=null for
    // byte-exact differential parity; live state is available at /core/status.
    ok("core.stopped", json!(null))
}

/// POST /api/core/restart
pub async fn core_restart(State(state): State<AppState>) -> Response {
    state.core.reap_if_exited();
    if !state.core.is_running() {
        return fail(
            StatusCode::CONFLICT,
            "core_not_running",
            "core.notRunning",
            json!({"runtimeState": "Stopped", "processIds": []}),
        );
    }
    let stop = stop_inner(&state).await;
    if !stop.status().is_success() {
        return stop;
    }
    start_inner(&state).await
}

/// GET /api/core/status — not in the differential corpus; useful for Nova.
pub async fn core_status(State(state): State<AppState>) -> Response {
    state.core.reap_if_exited();
    let pid = state.core.pid();
    ok(
        "core.statusLoaded",
        json!({
            "running": pid.is_some(),
            "runtimeState": if pid.is_some() { "Running" } else { "Stopped" },
            "processIds": pid.map(|p| vec![p]).unwrap_or_default(),
            "currentProfileId": state.store.string("currentProfileId")
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_config_uses_local_port_and_profile() {
        let snap = json!({
            "settings": {"inbound": {"localPort": 12345}},
            "currentProfileId": "p1",
            "profiles": [{"indexId": "p1", "configType": 5, "address": "v.example", "port": 443, "password": "uuid"}]
        });
        let cfg = generate_config(&snap);
        assert_eq!(cfg["inbounds"][0]["listen_port"], 12345);
        assert_eq!(cfg["outbounds"][0]["type"], "vless");
        assert_eq!(cfg["outbounds"][0]["server"], "v.example");
        assert_eq!(cfg["outbounds"][0]["uuid"], "uuid");
    }

    #[test]
    fn generate_config_empty_profile_is_socks() {
        let cfg = generate_config(&json!({"settings": {"inbound": {"localPort": 10808}}}));
        assert_eq!(cfg["outbounds"][0]["type"], "socks");
    }
}
