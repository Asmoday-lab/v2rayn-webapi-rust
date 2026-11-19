#!/usr/bin/env python3
"""Differential test: compare the Rust WebAPI against the pinned C# oracle.

Both servers must already be running (started by the CI job):
  oracle (C#)  at --oracle  (default http://127.0.0.1:5081)
  rust         at --rust    (default http://127.0.0.1:5082)

The request corpus is intentionally small and business-independent for this
milestone: only /api/health, which is the sole endpoint that needs no auth,
no Core, no data files. Each case sends the identical request to both servers
and compares status code, JSON body, and the *set* of health diagnostic headers
(their values are process/version-specific and normalized out).

Exit 0 = all cases match; exit 1 = at least one mismatch (with a diff printed).
"""

from __future__ import annotations

import argparse
import json
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
# Headers whose VALUES are legitimately process-specific and must not be
# compared; only their presence (handled by the diagnostic-header-name set).
VOLATILE_HEADER_VALUES = DIAGNOSTIC_HEADERS | {"date", "server"}


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
    expect_diagnostics: bool = True
    compare_headers: bool = True
    # Optional custom comparator; default compares status + body + header names.
    compare: Callable[["Response", "Response"], list[str]] | None = None


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


def compare_health(oracle: Response, rust: Response, case: Case) -> list[str]:
    diffs: list[str] = []
    if oracle.status != rust.status:
        diffs.append(f"status: oracle={oracle.status} rust={rust.status}")
    if oracle.body != rust.body:
        diffs.append(f"body: oracle={oracle.body!r} rust={rust.body!r}")

    if case.compare_headers:
        o_names = {k for k in oracle.headers if k in DIAGNOSTIC_HEADERS}
        r_names = {k for k in rust.headers if k in DIAGNOSTIC_HEADERS}
        if case.expect_diagnostics:
            if o_names != DIAGNOSTIC_HEADERS:
                diffs.append(f"oracle diagnostic headers incomplete: {sorted(o_names)}")
            if r_names != DIAGNOSTIC_HEADERS:
                diffs.append(f"rust diagnostic headers incomplete: {sorted(r_names)}")
        else:
            if o_names or r_names:
                diffs.append(
                    f"diagnostic headers must be absent: oracle={sorted(o_names)} rust={sorted(r_names)}"
                )

        # Compare media type only: C# emits `application/json; charset=utf-8`
        # while axum emits `application/json`. The charset suffix is not part
        # of the wire contract Nova depends on.
        o_ct = (oracle.headers.get("content-type") or "").split(";")[0].strip()
        r_ct = (rust.headers.get("content-type") or "").split(";")[0].strip()
        if o_ct != r_ct:
            diffs.append(f"content-type media: oracle={o_ct!r} rust={r_ct!r}")
    return diffs


CORPUS: list[Case] = [
    Case(
        name="health/plain-loopback",
        method="GET",
        path="/api/health",
        headers={"Host": "127.0.0.1:PORT"},  # PORT filled per-server below
        expect_diagnostics=True,
    ),
    Case(
        name="health/forwarded-header-suppresses-diagnostics",
        method="GET",
        path="/api/health",
        headers={"Host": "127.0.0.1:PORT", "X-Forwarded-For": "1.2.3.4"},
        expect_diagnostics=False,
    ),
    # NOTE: a foreign Origin is rejected by the C# WebOriginGuardMiddleware
    # with 403 before the health handler runs. That middleware is not ported
    # yet, so this case is deferred to the auth/CORS milestone to avoid a
    # false red. Tracked in contract.yaml (semantic_hotspots: cors-origin-guard).
]


def with_port_headers(case: Case, base: str) -> Case:
    """Fill the PORT placeholder in the Host header with the server's real port."""
    port = base.rstrip("/").rsplit(":", 1)[-1]
    headers = {k: v.replace("PORT", port) for k, v in case.headers.items()}
    return Case(
        name=case.name,
        method=case.method,
        path=case.path,
        headers=headers,
        body=case.body,
        expect_diagnostics=case.expect_diagnostics,
        compare_headers=case.compare_headers,
        compare=case.compare,
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--oracle", default="http://127.0.0.1:5081")
    parser.add_argument("--rust", default="http://127.0.0.1:5082")
    args = parser.parse_args()

    failures = 0
    for case in CORPUS:
        try:
            o = request(args.oracle, with_port_headers(case, args.oracle))
            r = request(args.rust, with_port_headers(case, args.rust))
        except Exception as exc:  # noqa: BLE001 - CI needs the full failure
            print(f"FAIL {case.name}: request error: {exc}")
            failures += 1
            continue

        diffs = compare_health(o, r, case)
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
