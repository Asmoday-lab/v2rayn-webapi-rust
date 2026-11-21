#!/usr/bin/env bash
# Termux launcher for v2rayn-webapi-rs.
#
# Usage:
#   V2RAYN_WEB_API_KEY='your-key' bash scripts/termux-run.sh
#
# Overrides: API_BIN, NOVA_DIST_DIR, V2RAYN_DATA_HOME, ASPNETCORE_URLS.
set -euo pipefail

# Resolve defaults relative to this script so the whole folder is relocatable:
#   ~/v2rayn-nova/{v2rayn-webapi-rs,nova-dist,data,termux-run.sh,...}
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
api_bin="${API_BIN:-$script_dir/v2rayn-webapi-rs}"
nova_dist="${NOVA_DIST_DIR:-$script_dir/nova-dist}"
data_home="${V2RAYN_DATA_HOME:-$script_dir/data}"
bind="${ASPNETCORE_URLS:-http://127.0.0.1:5080}"

if [[ -z "${PREFIX:-}" || ! -d "$PREFIX" ]]; then
  echo "this launcher is intended for Termux (\$PREFIX is unset)" >&2
  exit 1
fi

if [[ ! -x "$api_bin" ]]; then
  echo "API binary not found or not executable: $api_bin" >&2
  echo "set API_BIN to the downloaded aarch64-musl artifact." >&2
  exit 1
fi

mkdir -p "$data_home"

if [[ ! -d "$nova_dist" ]]; then
  echo "NOVA_DIST_DIR does not exist: $nova_dist (API-only mode)" >&2
fi

if [[ -z "${V2RAYN_WEB_API_KEY:-}" ]]; then
  echo "V2RAYN_WEB_API_KEY is unset; first-run setup will be required." >&2
fi

if command -v termux-wake-lock >/dev/null 2>&1; then
  termux-wake-lock
  echo "wakelock acquired (release with: termux-wake-unlock)"
else
  echo "termux-wake-lock unavailable: install termux-api and disable" >&2
  echo "battery optimization for Termux in Android settings." >&2
fi

export V2RAYN_DATA_HOME="$data_home"
export ASPNETCORE_URLS="$bind"
export NOVA_DIST_DIR="$nova_dist"

echo "serving API and Nova at $bind"
exec "$api_bin"
