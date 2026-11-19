#!/usr/bin/env bash
# CI-only orchestration: start the pinned C# oracle and this Rust build, wait
# for both health probes, run the differential corpus, then tear both down.
# Requires: dotnet (10.x), cargo, python3, curl on PATH.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
oracle_src="${1:?usage: differential.sh <oracle-src-dir>}"
oracle_port=5081
rust_port=5082

tmp="$(mktemp -d)"
oracle_pid=""
rust_pid=""
cleanup() {
  for pid in "${rust_pid:-}" "${oracle_pid:-}"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill -TERM "$pid" 2>/dev/null || true
    fi
  done
  sleep 1
  for pid in "${rust_pid:-}" "${oracle_pid:-}"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill -KILL "$pid" 2>/dev/null || true
    fi
  done
  rm -rf "$tmp"
}
trap cleanup EXIT HUP INT TERM

wait_health() {
  local url="$1" label="$2" pid="$3" log="$4" ok=0
  for _ in $(seq 1 120); do
    if curl -fsS --noproxy '*' "$url/api/health" 2>/dev/null | grep -q '"status":"ok"'; then
      ok=1
      break
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "=== $label exited before healthy ===" >&2
      cat "$log" >&2 || true
      return 1
    fi
    sleep 0.5
  done
  if [[ "$ok" != 1 ]]; then
    echo "=== $label did not become healthy ===" >&2
    cat "$log" >&2 || true
    return 1
  fi
  echo "$label healthy at $url"
}

echo "=== building Rust (x64 debug) ==="
cargo build --manifest-path "$root/Cargo.toml"

echo "=== starting C# oracle on :$oracle_port ==="
bash "$root/scripts/run-oracle.sh" "$oracle_src" "$oracle_port" \
  "$tmp/oracle.log" "$tmp/oracle.pid"
oracle_pid="$(cat "$tmp/oracle.pid")"
wait_health "http://127.0.0.1:$oracle_port" "oracle" "$oracle_pid" "$tmp/oracle.log"

echo "=== starting Rust on :$rust_port ==="
# Both servers must share the same management key so login behaves identically.
ASPNETCORE_URLS="http://127.0.0.1:$rust_port" \
  V2RAYN_WEB_API_KEY='ci-oracle-key-1234567890' \
  "$root/target/debug/v2rayn-webapi-rs" >"$tmp/rust.log" 2>&1 &
rust_pid=$!
echo "$rust_pid" >"$tmp/rust.pid"
wait_health "http://127.0.0.1:$rust_port" "rust" "$rust_pid" "$tmp/rust.log"

echo "=== running differential corpus ==="
python3 "$root/scripts/differential.py" \
  --oracle "http://127.0.0.1:$oracle_port" \
  --rust "http://127.0.0.1:$rust_port"
result=$?

echo "=== differential result: $result ==="
if [[ "$result" -ne 0 ]]; then
  echo "--- oracle log tail ---" >&2
  tail -n 40 "$tmp/oracle.log" >&2 || true
  echo "--- rust log tail ---" >&2
  tail -n 40 "$tmp/rust.log" >&2 || true
fi
exit "$result"
