#!/usr/bin/env python3
"""Differential test: compare the Rust WebAPI against the pinned C# oracle.

Both servers must already be running (started by the CI job):
  oracle (C#)  at --oracle  (default http://127.0.0.1:5081)
  rust         at --rust    (default http://127.0.0.1:5082)

Each case sends the identical request to both servers and compares:
  - HTTP status code
  - JSON body (after normalizing legitimately volatile values: session tokens,
    ticket tokens and ISO-8601 expiresAt timestamps are random/time-derived)
  - the *set* of health diagnostic header names (values are process-specific)
  - C# security header values (fixed contract constants)

Exit 0 = all cases match; exit 1 = at least one mismatch (with a diff printed).
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from typing import Callable

DIAGNOSTIC_HEADERS = {
    "x-v2rayn-webapi-instance-pid",
    "x-v2rayn-webapi-version",
    "x-v2rayn-webapi-core-state",
    "x-v2rayn-webapi-core-process-ids",
    "x-v2rayn-webapi-core-profile-id",
}
# Security headers whose values are fixed contract constants; compare exactly.
SECURITY_HEADERS = {
    "x-content-type-options": "nosniff",
    "referrer-policy": "no-referrer",
    "x-frame-options": "deny",
}
CSP_PREFIX = "default-src 'self'; base-uri 'self'; object-src 'none'"

# Volatile values normalized out of JSON bodies before comparison.
TOKEN_RE = re.compile(r"^[A-Za-z0-9_-]{43}$")
# .NET System.Text.Json emits UTC DateTimeOffset as "...Z"; our Rust side emits
# "+00:00". Accept both so the shape check is not format-sensitive.
TIMESTAMP_RE = re.compile(
    r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|\+00:00)$"
)


@dataclass
class Response:
    status: int
    body: object
    headers: dict[str, str] = field(default_factory=dict)


@dataclass
class Case:
    name: str
    method: str
    path: str
    headers: dict[str, str] = field(default_factory=dict)
    body: bytes | None = None
    # Health diagnostics expected? Depends on the local/forwarded policy.
    expect_diagnostics: bool | None = None
    compare_headers: bool = True
    # If True, a per-server session token is injected as Authorization: Bearer.
    auth: bool = False
    # Optional custom comparator; default = compare status + normalized body + headers.
    compare: Callable[["Response", "Response", "Case"], list[str]] | None = None


def request(base: str, case: Case) -> Response:
    url = base.rstrip("/") + case.path
    headers = dict(case.headers)
    req = urllib.request.Request(url, data=case.body, headers=headers, method=case.method)
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            raw = resp.read()
            status = resp.status
            hdrs = {k.lower(): v for k, v in resp.headers.items()}
    except urllib.error.HTTPError as exc:
        raw = exc.read()
        status = exc.code
        hdrs = {k.lower(): v for k, v in exc.headers.items()}

    try:
        body: object = json.loads(raw)
    except (json.JSONDecodeError, UnicodeDecodeError):
        body = raw.decode("utf-8", errors="replace")
    return Response(status=status, body=body, headers=hdrs)


def normalize_body(value: object) -> object:
    """Recursively replace random/env-specific values with stable placeholders."""
    if isinstance(value, dict):
        out = {}
        for key, val in value.items():
            if key in ("token", "ticket") and isinstance(val, str) and TOKEN_RE.match(val):
                out[key] = "<TOKEN>"
            elif key in ("expiresAt", "checkedAt") and isinstance(val, str) and TIMESTAMP_RE.match(val):
                out[key] = "<TIMESTAMP>"
            elif key == "runtime" and isinstance(val, str):
                # Environment diagnostic string: oracle reports CI paths, Rust
                # reports its own. Never part of the client-facing contract.
                out[key] = "<RUNTIME>"
            else:
                out[key] = normalize_body(val)
        return out
    if isinstance(value, list):
        return [normalize_body(v) for v in value]
    return value


def compare_default(oracle: Response, rust: Response, case: Case) -> list[str]:
    diffs: list[str] = []
    if oracle.status != rust.status:
        diffs.append(f"status: oracle={oracle.status} rust={rust.status}")

    o_body = normalize_body(oracle.body)
    r_body = normalize_body(rust.body)
    if o_body != r_body:
        diffs.append(f"body: oracle={o_body!r} rust={r_body!r}")

    if case.compare_headers:
        o_names = {k for k in oracle.headers if k in DIAGNOSTIC_HEADERS}
        r_names = {k for k in rust.headers if k in DIAGNOSTIC_HEADERS}
        if case.expect_diagnostics is True:
            if o_names != DIAGNOSTIC_HEADERS:
                diffs.append(f"oracle diagnostic headers incomplete: {sorted(o_names)}")
            if r_names != DIAGNOSTIC_HEADERS:
                diffs.append(f"rust diagnostic headers incomplete: {sorted(r_names)}")
        elif case.expect_diagnostics is False:
            if o_names or r_names:
                diffs.append(
                    f"diagnostic headers must be absent: oracle={sorted(o_names)} rust={sorted(r_names)}"
                )
        # For non-health cases expect_diagnostics is None: require parity only.
        elif o_names != r_names:
            diffs.append(
                f"diagnostic header sets differ: oracle={sorted(o_names)} rust={sorted(r_names)}"
            )

        # Security headers are fixed constants on both sides.
        for name, expected in SECURITY_HEADERS.items():
            o = (oracle.headers.get(name) or "").lower()
            r = (rust.headers.get(name) or "").lower()
            if o != expected or r != expected:
                diffs.append(
                    f"{name}: oracle={o!r} rust={r!r} expected={expected!r}"
                )
        o_csp = oracle.headers.get("content-security-policy") or ""
        r_csp = rust.headers.get("content-security-policy") or ""
        if not o_csp.startswith(CSP_PREFIX) or o_csp != r_csp:
            diffs.append(
                f"content-security-policy mismatch: oracle={o_csp!r} rust={r_csp!r}"
            )

        # Content-type media type only (C# emits `; charset=utf-8`, axum does not).
        o_ct = (oracle.headers.get("content-type") or "").split(";")[0].strip()
        r_ct = (rust.headers.get("content-type") or "").split(";")[0].strip()
        if o_ct != r_ct:
            diffs.append(f"content-type media: oracle={o_ct!r} rust={r_ct!r}")
    return diffs


def compare_health(oracle: Response, rust: Response, case: Case) -> list[str]:
    return compare_default(oracle, rust, case)


def json_body(obj: object) -> bytes:
    return json.dumps(obj, separators=(",", ":")).encode()


# Management key injected identically into both servers by differential.sh.
MANAGEMENT_KEY = "ci-oracle-key-1234567890"

CORPUS: list[Case] = [
    # ---- health (no auth, no data, no Core) ----
    Case(
        name="health/plain-loopback",
        method="GET",
        path="/api/health",
        headers={"Host": "127.0.0.1:PORT"},
        expect_diagnostics=True,
    ),
    Case(
        name="health/forwarded-header-suppresses-diagnostics",
        method="GET",
        path="/api/health",
        headers={"Host": "127.0.0.1:PORT", "X-Forwarded-For": "1.2.3.4"},
        expect_diagnostics=False,
    ),
    Case(
        name="health/foreign-origin-403",
        method="GET",
        path="/api/health",
        headers={"Host": "127.0.0.1:PORT", "Origin": "https://evil.example"},
        expect_diagnostics=False,
    ),
    # ---- setup/status (no auth) ----
    Case(
        name="setup-status/loopback",
        method="GET",
        path="/api/setup/status",
        headers={"Host": "127.0.0.1:PORT"},
    ),
    # ---- auth/login ----
    Case(
        name="auth/login/wrong-key-401",
        method="POST",
        path="/api/auth/login",
        headers={"Host": "127.0.0.1:PORT", "Content-Type": "application/json"},
        body=json_body({"key": "definitely-wrong-key"}),
    ),
    Case(
        name="auth/login/empty-key-401",
        method="POST",
        path="/api/auth/login",
        headers={"Host": "127.0.0.1:PORT", "Content-Type": "application/json"},
        body=json_body({"key": ""}),
    ),
    Case(
        name="auth/login/valid-key-200",
        method="POST",
        path="/api/auth/login",
        headers={"Host": "127.0.0.1:PORT", "Content-Type": "application/json"},
        body=json_body({"key": MANAGEMENT_KEY}),
    ),
    # ---- protected endpoint without session ----
    Case(
        name="status/no-session-401",
        method="GET",
        path="/api/status",
        headers={"Host": "127.0.0.1:PORT"},
    ),
    Case(
        name="auth/logout/no-session-401",
        method="POST",
        path="/api/auth/logout",
        headers={"Host": "127.0.0.1:PORT"},
    ),
    Case(
        name="auth/sse-ticket/no-session-401",
        method="POST",
        path="/api/auth/sse-ticket",
        headers={"Host": "127.0.0.1:PORT"},
    ),
    # ---- authenticated static/near-static GET endpoints ----
    Case(name="get/status", method="GET", path="/api/status", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/operations", method="GET", path="/api/operations", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/profile-groups", method="GET", path="/api/profile-groups", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/editor-options", method="GET", path="/api/editor-options", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/subscriptions", method="GET", path="/api/subscriptions", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/profiles", method="GET", path="/api/profiles", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/settings", method="GET", path="/api/settings", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/settings-webdav", method="GET", path="/api/settings/webdav", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/settings-dns-simple", method="GET", path="/api/settings/dns/simple", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/settings-dns-profiles", method="GET", path="/api/settings/dns/profiles", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/settings-dns-editor-options", method="GET", path="/api/settings/dns/editor-options", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/settings-dns-defaults", method="GET", path="/api/settings/dns/defaults", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/settings-routing-profiles", method="GET", path="/api/settings/routing-profiles", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/settings-core-templates", method="GET", path="/api/settings/core-templates", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/core-updates", method="GET", path="/api/core-updates", headers={"Host": "127.0.0.1:PORT"}, auth=True),
    Case(name="get/core-updates-progress", method="GET", path="/api/core-updates/progress", headers={"Host": "127.0.0.1:PORT"}, auth=True),
]


def with_port_headers(case: Case, base: str, token: str = "") -> Case:
    """Fill the PORT placeholder and optionally inject the server's session token."""
    port = base.rstrip("/").rsplit(":", 1)[-1]
    headers = {k: v.replace("PORT", port) for k, v in case.headers.items()}
    if case.auth:
        headers["Authorization"] = f"Bearer {token}"
    return Case(
        name=case.name,
        method=case.method,
        path=case.path,
        headers=headers,
        body=case.body,
        expect_diagnostics=case.expect_diagnostics,
        compare_headers=case.compare_headers,
        auth=case.auth,
        compare=case.compare,
    )


def login_token(base: str) -> str:
    """Authenticate against one server and return its session token."""
    case = Case(
        name="login",
        method="POST",
        path="/api/auth/login",
        headers={"Content-Type": "application/json"},
        body=json_body({"key": MANAGEMENT_KEY}),
    )
    resp = request(base, case)
    if resp.status != 200 or not isinstance(resp.body, dict):
        raise RuntimeError(f"login to {base} failed: {resp.status} {resp.body!r}")
    return resp.body["data"]["token"]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--oracle", default="http://127.0.0.1:5081")
    parser.add_argument("--rust", default="http://127.0.0.1:5082")
    args = parser.parse_args()

    # Each server gets its own session token (tokens are random and differ).
    try:
        oracle_token = login_token(args.oracle)
        rust_token = login_token(args.rust)
    except Exception as exc:  # noqa: BLE001
        print(f"FAIL login setup: {exc}")
        return 1

    failures = 0
    for case in CORPUS:
        try:
            o = request(args.oracle, with_port_headers(case, args.oracle, oracle_token))
            r = request(args.rust, with_port_headers(case, args.rust, rust_token))
        except Exception as exc:  # noqa: BLE001 - CI needs the full failure
            print(f"FAIL {case.name}: request error: {exc}")
            failures += 1
            continue

        diffs = (case.compare or compare_default)(o, r, case)
        if diffs:
            failures += 1
            print(f"FAIL {case.name}")
            for d in diffs:
                print(f"    {d}")
        else:
            print(f"PASS {case.name}")

    total = len(CORPUS)
    print(f"\n{total - failures}/{total} differential cases matched")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
