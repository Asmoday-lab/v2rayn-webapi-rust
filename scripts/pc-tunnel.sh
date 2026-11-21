#!/usr/bin/env bash
# PC-side SSH tunnel: expose the Termux API/Nova on the PC's loopback.
#
# The API binds 127.0.0.1 only (by design), so a PC cannot reach it directly.
# This forwards PC 127.0.0.1:5080 -> Termux 127.0.0.1:5080 over the SSH alias
# `mi5x`, keeping the plaintext HTTP listener off the LAN.
#
# Usage (Git Bash / WSL / any POSIX shell with the mi5x SSH alias configured):
#   bash scripts/pc-tunnel.sh
# Then open http://127.0.0.1:5080/ in the PC browser.
#
# Overrides: LOCAL_PORT, REMOTE_PORT, MI5X_HOST.
# Stop with Ctrl+C.
set -euo pipefail

local_port="${LOCAL_PORT:-5080}"
remote_port="${REMOTE_PORT:-5080}"
host="${MI5X_HOST:-mi5x}"

echo "PC  http://127.0.0.1:${local_port}/  ->  SSH(${host})  ->  Termux 127.0.0.1:${remote_port}"
echo "Press Ctrl+C to stop."
exec ssh -N \
  -o BatchMode=yes \
  -o ExitOnForwardFailure=yes \
  -o ServerAliveInterval=30 \
  -o ServerAliveCountMax=3 \
  -L "${local_port}:127.0.0.1:${remote_port}" \
  "$host"
