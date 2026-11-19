#!/usr/bin/env python3
"""Dump the pinned C# oracle's responses for GET endpoints to a JSON file.

Run inside CI differential job after the oracle is healthy. The resulting
artifact is the authoritative golden sample for implementing Rust DTOs — far
more reliable than guessing ServiceLib's serialization shapes.

Volatile values (tokens, timestamps, pids) are normalized to placeholders so
the dump is stable across runs.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.error
import urllib.request

TOKEN_RE = re.compile(r"^[A-Za-z0-9_-]{43}$")
TIMESTAMP_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$")
GUID_RE = re.compile(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")

MANAGEMENT_KEY = "ci-oracle-key-1234567890"

# GET endpoints that are safe to probe on a fresh empty data directory.
# Mutating / network-dependent endpoints (check-update, download, events) are
# excluded. Path params are represented by concrete sample ids where harmless.
GET_ENDPOINTS = [
    "/api/health",
    "/api/status",
    "/api/operations",
    "/api/logs",
    "/api/logs/page",
    "/api/editor-options",
    "/api/subscriptions",
    "/api/profile-groups",
    "/api/profiles",
    "/api/settings",
    "/api/settings/webdav",
    "/api/settings/dns/simple",
    "/api/settings/dns/profiles",
    "/api/settings/dns/editor-options",
    "/api/settings/dns/defaults",
    "/api/settings/routing-profiles",
    "/api/settings/core-templates",
    "/api/core-updates",
    "/api/core-updates/progress",
    "/api/web-updates",
]

# Safe write operations on a fresh empty data directory: either no-ops or
# deterministic failures. Captured AFTER all GETs so they cannot perturb the
# read golden samples. (method, path, json-body-or-None)
MUTATIONS = [
    ("DELETE", "/api/logs", None),
    ("DELETE", "/api/statistics", None),
    ("DELETE", "/api/speedtests", None),
    ("POST", "/api/latency/stop", None),
    ("POST", "/api/core/stop", None),
    ("POST", "/api/profiles/deduplicate", None),
    ("DELETE", "/api/profiles/invalid-test-results", None),
    ("PUT", "/api/profile-groups/current", {"subscriptionId": None}),
    ("POST", "/api/profile-groups/generate/all", None),
    ("POST", "/api/settings/routing-profiles/import", None),
    ("POST", "/api/subscriptions/update", {"subscriptionId": "nonexistent", "useProxy": False}),
    ("POST", "/api/profiles/0000000000000000000/select", None),
]


def normalize(value):
    if isinstance(value, dict):
        return {k: normalize(v) for k, v in value.items()}
    if isinstance(value, list):
        return [normalize(v) for v in value]
    if isinstance(value, str):
        if TOKEN_RE.match(value):
            return "<TOKEN>"
        if TIMESTAMP_RE.match(value):
            return "<TIMESTAMP>"
        if GUID_RE.match(value):
            return "<GUID>"
    return value


def raw_request(base, method, path, headers=None, body=None):
    req = urllib.request.Request(
        base.rstrip("/") + path,
        data=body,
        headers=headers or {},
        method=method,
    )
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--oracle", default="http://127.0.0.1:5081")
    parser.add_argument("--out", default="oracle-dump.json")
    args = parser.parse_args()

    # Authenticate once to obtain a session token.
    status, raw = raw_request(
        args.oracle,
        "POST",
        "/api/auth/login",
        headers={"Content-Type": "application/json"},
        body=json.dumps({"key": MANAGEMENT_KEY}).encode(),
    )
    if status != 200:
        print(f"login failed: {status} {raw[:200]!r}", file=sys.stderr)
        return 1
    token = json.loads(raw)["data"]["token"]

    dump = {}
    for path in GET_ENDPOINTS:
        status, raw = raw_request(
            args.oracle,
            "GET",
            path,
            headers={"Authorization": f"Bearer {token}"},
        )
        try:
            body = json.loads(raw)
        except (json.JSONDecodeError, UnicodeDecodeError):
            body = raw.decode("utf-8", errors="replace")
        dump[path] = {
            "status": status,
            "body": normalize(body),
        }
        print(f"{status} {path} ({len(raw)} bytes)")

    # Capture safe write operations last (they may mutate state).
    mutations = {}
    for method, path, body in MUTATIONS:
        headers = {"Authorization": f"Bearer {token}"}
        payload = None
        if body is not None:
            headers["Content-Type"] = "application/json"
            payload = json.dumps(body).encode()
        status, raw = raw_request(args.oracle, method, path, headers=headers, body=payload)
        try:
            parsed = json.loads(raw)
        except (json.JSONDecodeError, UnicodeDecodeError):
            parsed = raw.decode("utf-8", errors="replace")
        mutations[f"{method} {path}"] = {"status": status, "body": normalize(parsed)}
        print(f"{status} {method} {path} ({len(raw)} bytes)")

    dump["__mutations__"] = mutations
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(dump, f, indent=2, ensure_ascii=False)
    print(f"\nwrote {args.out} with {len(dump) - 1} GET + {len(mutations)} write endpoints")
    return 0


if __name__ == "__main__":
    sys.exit(main())
