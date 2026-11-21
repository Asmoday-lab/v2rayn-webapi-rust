#!/usr/bin/env bash
# Termux on-device smoke test for v2rayn-webapi-rs.
#
# Verifies the properties CI cannot: that the static ELF actually execve's on
# Android/Bionic, that the auth chain works from localhost, and that Nova is
# served from the same origin. Does NOT test Core process launch (not implemented).
#
# Usage:
#   API_BIN=/sdcard/Download/v2rayn-webapi-rs \
#   NOVA_DIST_DIR=$HOME/v2rayN-WebUI-Nova/dist \
#   bash scripts/termux-smoke.sh
#
# Optional: PASSWORD='your-management-key' to avoid the interactive prompt.
# Nothing is written outside $TMPDIR; the API is killed on exit.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
api_bin="${API_BIN:-$script_dir/v2rayn-webapi-rs}"
nova_dist="${NOVA_DIST_DIR:-$script_dir/nova-dist}"
port="${SMOKE_PORT:-5090}"
base="http://127.0.0.1:${port}"
password="${PASSWORD:-}"

pass=0
fail=0
ok()   { printf '  [PASS] %s\n' "$1"; pass=$((pass+1)); }
bad()  { printf '  [FAIL] %s\n' "$1"; fail=$((fail+1)); }
info() { printf '\n== %s ==\n' "$1"; }

need() { command -v "$1" >/dev/null 2>&1 || { echo "missing required tool: $1" >&2; exit 2; }; }
need curl

[[ -x "$api_bin" ]] || { echo "API binary not executable: $api_bin" >&2; echo "set API_BIN" >&2; exit 2; }

if [[ -z "$password" ]]; then
  read -r -s -p 'Management key to use for this smoke test (>=12 chars): ' password
  echo
fi
if (( ${#password} < 12 )); then
  echo "management key must be >= 12 chars (C# minimum)" >&2
  exit 2
fi

tmp="$(mktemp -d)"
api_pid=""
cleanup() {
  [[ -n "$api_pid" ]] && kill "$api_pid" 2>/dev/null || true
  rm -rf "$tmp"
}
trap cleanup EXIT HUP INT TERM

info "1. static ELF sanity"
if command -v readelf >/dev/null 2>&1 && readelf -l "$api_bin" 2>/dev/null | grep -q INTERP; then
  bad "binary has an INTERP segment (not static)"
else
  ok "no dynamic loader / INTERP (or readelf unavailable; CI already verified)"
fi
if command -v file >/dev/null 2>&1; then
  file "$api_bin" | grep -q 'statically linked' && ok "file(1): statically linked" || bad "file(1): not static"
fi

info "2. execve on Bionic + health"
V2RAYN_DATA_HOME="$tmp/data" \
V2RAYN_WEB_API_KEY="$password" \
V2RAYN_WEB_AUTOSTART=false \
ASPNETCORE_URLS="$base" \
NOVA_DIST_DIR="$nova_dist" \
  "$api_bin" >"$tmp/api.log" 2>&1 &
api_pid=$!

healthy=0
for _ in $(seq 1 40); do
  if curl -fsS --noproxy '*' "$base/api/health" 2>/dev/null | grep -q '"status":"ok"'; then healthy=1; break; fi
  kill -0 "$api_pid" 2>/dev/null || break
  sleep 0.25
done
if [[ "$healthy" == 1 ]]; then
  ok "aarch64 static ELF executed natively; /api/health = ok"
else
  bad "API did not become healthy"
  echo "--- api.log ---" >&2
  tail -n 30 "$tmp/api.log" >&2
  exit 1
fi

health_headers="$(curl -sS --noproxy '*' -D - -o /dev/null "$base/api/health" | tr -d '\r' | grep -i '^x-v2rayn-webapi-' | cut -d: -f1 | tr '[:upper:]' '[:lower:]' | sort | tr '\n' ' ')"
expected="x-v2rayn-webapi-core-process-ids x-v2rayn-webapi-core-profile-id x-v2rayn-webapi-core-state x-v2rayn-webapi-instance-pid x-v2rayn-webapi-version "
[[ "$health_headers" == "$expected" ]] && ok "5 health diagnostic headers present" || bad "health headers differ: $health_headers"

info "3. auth chain"
# /api/setup/status is a bare DTO (mirrors the C# oracle), not an envelope.
# With V2RAYN_WEB_API_KEY pre-set the expected shape is:
#   {"environmentKeyConfigured":true,"setupAllowedFromThisRequest":true,"setupRequired":false}
setup_status="$(curl -sS --noproxy '*' "$base/api/setup/status")"
if printf '%s' "$setup_status" | grep -q '"setupRequired":false' \
   && printf '%s' "$setup_status" | grep -q '"environmentKeyConfigured":true'; then
  ok "GET /api/setup/status readable pre-auth (key configured, setup not required)"
else
  bad "setup/status: $setup_status"
fi

bad_login="$(curl -sS --noproxy '*' -o /dev/null -w '%{http_code}' -X POST "$base/api/auth/login" -H 'Content-Type: application/json' --data '{"key":"wrong-wrong-wrong"}')"
[[ "$bad_login" == "401" ]] && ok "login with wrong key -> 401" || bad "login wrong key -> $bad_login (want 401)"

login="$(curl -sS --noproxy '*' -X POST "$base/api/auth/login" -H 'Content-Type: application/json' --data "{\"key\":\"$password\"}")"
token="$(printf '%s' "$login" | sed -n 's/.*"token"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
if [[ -n "$token" ]]; then
  ok "login issued a session token (${#token} chars)"
else
  bad "no token in login response: $login"
  exit 1
fi

unauth="$(curl -sS --noproxy '*' -o /dev/null -w '%{http_code}' "$base/api/status")"
[[ "$unauth" == "401" ]] && ok "protected endpoint without token -> 401" || bad "unauthenticated /api/status -> $unauth"

authed="$(curl -sS --noproxy '*' -o /dev/null -w '%{http_code}' "$base/api/status" -H "Authorization: Bearer $token")"
[[ "$authed" == "200" ]] && ok "protected endpoint with token -> 200" || bad "authenticated /api/status -> $authed"

ticket="$(curl -sS --noproxy '*' -X POST "$base/api/auth/sse-ticket" -H "Authorization: Bearer $token")"
sse_ticket="$(printf '%s' "$ticket" | sed -n 's/.*"ticket"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
[[ -n "$sse_ticket" ]] && ok "sse-ticket minted (${#sse_ticket} chars)" || bad "sse-ticket: $ticket"

info "4. same-origin Nova hosting"
if [[ -n "$nova_dist" && -f "$nova_dist/index.html" ]]; then
  index_code="$(curl -sS --noproxy '*' -o "$tmp/index.html" -w '%{http_code}' "$base/")"
  [[ "$index_code" == "200" ]] && ok "GET / serves Nova index.html" || bad "GET / -> $index_code"
  curl -sS --noproxy '*' "$base/" | grep -qi '<html' && ok "index.html looks like HTML" || bad "index.html body is not HTML"
  # curl normally collapses ../ client-side; --path-as-is sends it verbatim.
  # The SPA fallback intentionally answers unknown paths with index.html (200),
  # so status alone is not a signal -- assert no file content is disclosed.
  traversal_body="$(curl -sS --path-as-is --noproxy '*' "$base/../../etc/passwd")"
  if printf '%s' "$traversal_body" | grep -q 'root:x:0:0'; then
    bad "path traversal leaked /etc/passwd"
  else
    ok "path traversal disclosed no file content (SPA fallback)"
  fi
else
  echo "  [SKIP] NOVA_DIST_DIR unset or no index.html"
fi

info "result"
printf '  %d passed, %d failed\n' "$pass" "$fail"
if (( fail > 0 )); then
  echo "--- api.log tail ---" >&2
  tail -n 40 "$tmp/api.log" >&2
  exit 1
fi
