#!/usr/bin/env bash
# Start the pinned C# v2rayN.WebAPI as a headless differential oracle.
#
# Usage:
#   run-oracle.sh <source-root> <port> <log-file> <pid-file>
#
# The startup recipe is copied verbatim from the upstream integration test
# test-startup-security.sh (start_with_valid_management_key_boundary): a valid
# management key, an isolated V2RAYN_DATA_HOME, autostart disabled, a loopback
# URL, --foreground --no-open, and forwarded-header env vars cleared.
set -euo pipefail

source_root="${1:?usage: run-oracle.sh <source-root> <port> <log> <pid>}"
port="${2:?port required}"
log_file="${3:?log file required}"
pid_file="${4:?pid file required}"

project="$source_root/v2rayN/v2rayN.WebAPI/v2rayN.WebAPI.csproj"
test -f "$project"

export V2RAYN_WEB_API_KEY='ci-oracle-key-1234567890'
export V2RAYN_DATA_HOME="$(mktemp -d)"
export V2RAYN_WEB_AUTOSTART='false'
export ASPNETCORE_URLS="http://127.0.0.1:${port}"

# Clear any supervisor/container markers so Program.Main sees a plain CI host.
# --no-launch-profile is essential: without it `dotnet run` loads
# Properties/launchSettings.json, whose applicationUrl overrides
# ASPNETCORE_URLS and binds 4939/4940 instead of our chosen port.
env -u INVOCATION_ID -u JOURNAL_STREAM -u NOTIFY_SOCKET \
    -u DOTNET_RUNNING_IN_CONTAINER -u container \
    dotnet run --project "$project" --configuration Release --no-launch-profile -- \
    --foreground --no-open >"$log_file" 2>&1 &
echo $! >"$pid_file"
