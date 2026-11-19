#!/usr/bin/env bash
# Guard the dependency graph: these crates drag C toolchains or the wrong TLS
# backend into an aarch64-unknown-linux-musl static build. Fail if any appears.
set -euo pipefail

cd "$(dirname "$0")/.."

for banned in aws-lc-rs aws-lc-sys openssl-sys native-tls rusqlite libsqlite3-sys; do
  if cargo tree -i "$banned" >/dev/null 2>&1; then
    echo "BANNED DEPENDENCY: $banned is in the dependency graph" >&2
    cargo tree -i "$banned" >&2
    exit 1
  fi
done

echo "dependency graph clean: no C toolchain / aws-lc / openssl / sqlite crates"
