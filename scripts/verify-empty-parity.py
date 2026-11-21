import json, os, urllib.request, urllib.error

BASE = os.environ.get("RUST_BASE", "http://127.0.0.1:5080")

def call(method, path, body=None, token=None):
    h = {}
    if body is not None:
        h["Content-Type"] = "application/json"
    if token:
        h["Authorization"] = "Bearer " + token
    data = json.dumps(body, separators=(",", ":")).encode() if body is not None else None
    r = urllib.request.Request(BASE + path, data=data, headers=h, method=method)
    try:
        with urllib.request.urlopen(r, timeout=8) as resp:
            return resp.status, json.load(resp)
    except urllib.error.HTTPError as e:
        return e.code, json.load(e)

tok = call("POST", "/api/auth/login", {"key": "ci-oracle-key-1234567890"})[1]["data"]["token"]
cases = [
    ("POST", "/api/subscriptions", {"remarks": "", "url": ""}, 400, "subscription_name_required"),
    ("DELETE", "/api/subscriptions/__missing__", None, 404, "subscription_not_found"),
    ("DELETE", "/api/profiles", {"profileIds": []}, 400, "profile_selection_required"),
    ("POST", "/api/profiles/0000000000000000000/select", None, 409, "profile_not_found"),
    ("POST", "/api/profiles/import", {"content": ""}, 400, "profile_content_required"),
    ("PUT", "/api/settings/routing", {"domainStrategy": "__invalid__", "domainStrategy4Singbox": None}, 400, "routing_strategy_invalid"),
    ("PUT", "/api/settings/application", {"geoAutoUpdateInterval": -1}, 400, "app_setting_invalid"),
    ("PUT", "/api/settings/inbound", {"localPort": 0}, 400, "inbound_port_invalid"),
    ("PUT", "/api/settings/speedtest", {"speedTestTimeout": 0}, 400, "speedtest_settings_invalid"),
    ("POST", "/api/core/start", None, 409, "profile_not_selected"),
    ("POST", "/api/core/restart", None, 409, "core_not_running"),
]
p = f = 0
for method, path, body, exp_status, exp_code in cases:
    st, resp = call(method, path, body, tok)
    got = resp.get("code")
    if st == exp_status and got == exp_code:
        p += 1
    else:
        f += 1
        print("MISMATCH", method, path, "status", st, "exp", exp_status, "code", got, "exp", exp_code)
print(f"write-case parity: {p} pass, {f} fail")
