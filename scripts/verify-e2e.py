"""End-to-end check: real CRUD persists and Core config generation works."""
import json, os, urllib.request, urllib.error

BASE = os.environ.get("RUST_BASE", "http://127.0.0.1:5080")
DATA_HOME = os.environ.get("V2RAYN_DATA_HOME", "data")

def call(method, path, body=None, token=None):
    h = {}
    if body is not None:
        h["Content-Type"] = "application/json"
    if token:
        h["Authorization"] = "Bearer " + token
    data = json.dumps(body, separators=(",", ":"), ensure_ascii=False).encode() if body is not None else None
    r = urllib.request.Request(BASE + path, data=data, headers=h, method=method)
    try:
        with urllib.request.urlopen(r, timeout=8) as resp:
            return resp.status, json.load(resp)
    except urllib.error.HTTPError as e:
        return e.code, json.load(e)

tok = call("POST", "/api/auth/login", {"key": "ci-oracle-key-1234567890"})[1]["data"]["token"]

def show(label, resp):
    print(label, json.dumps(resp, ensure_ascii=False)[:160])

# Subscriptions CRUD
st, sub = call("POST", "/api/subscriptions", {"remarks": "测试订阅", "url": "https://example.com/sub", "enabled": True}, tok)
sid = sub["data"]["id"]
show("create sub:", (st, sub["code"]))
st, subs = call("GET", "/api/subscriptions", token=tok)
assert any(s["id"] == sid for s in subs["data"]), "subscription not listed"

# Profiles CRUD
st, prof = call("POST", "/api/profiles", {"remarks": "VLESS 节点", "address": "v.example.com", "port": 443, "configType": 5, "password": "uuid-1234", "sni": "v.example.com", "network": "ws", "subid": sid}, tok)
pid = prof["data"]["indexId"]
show("create profile:", (st, prof["code"]))
st, rows = call("GET", "/api/profiles", token=tok)
assert len(rows["data"]) == 1

st, one = call("GET", "/api/profiles/" + pid, token=tok)
assert one["data"]["remarks"] == "VLESS 节点"
show("get one:", (st, one["data"]["address"]))

st, upd = call("PUT", "/api/profiles/" + pid, {"remarks": "改名节点", "port": 8443}, tok)
assert upd["data"]["port"] == 8443
show("update:", (st, upd["data"]["remarks"]))

# Select -> currentProfileId
st, sel = call("POST", "/api/profiles/" + pid + "/select", token=tok)
show("select:", (st, sel["code"]))

# Core start: no sing-box on this host, so spawn fails gracefully (409 core_start_failed),
# but the config.json must have been generated from the real profile.
st, start = call("POST", "/api/core/start", token=tok)
show("core start:", (st, start["code"]))

import os
cfg_path = os.path.join(DATA_HOME, "binConfigs", "config.json")
if os.path.exists(cfg_path):
    cfg = json.load(open(cfg_path, encoding="utf-8"))
    ob = cfg["outbounds"][0]
    print("config.json generated:", ob["type"], ob["server"], ob["server_port"], ob.get("uuid"))
    assert ob["type"] == "vless" and ob["server"] == "v.example.com" and ob["server_port"] == 8443
    print("config reflects real profile: OK")
else:
    print("ERROR: config.json not generated")
    raise SystemExit(1)

# Delete
st, dele = call("DELETE", "/api/profiles", {"profileIds": [pid]}, tok)
assert call("GET", "/api/profiles", token=tok)[1]["data"] == []
show("delete profile:", (st, dele["code"]))

print("E2E CRUD + core-config: ALL PASS")
