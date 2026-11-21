#!/usr/bin/env bash
# On-device Core lifecycle verification for v2rayn-webapi-rs.
# Run over SSH with: ssh mi5x 'bash -s' < scripts/device-core-check.sh
set -uo pipefail

BASE_DIRECT=http://127.0.0.1:5080
BASE_NGINX=http://127.0.0.1:8080
KEY=ondevice-verification-key-1234
DIR="$HOME/v2rayn-nova"

say() { printf '\n=== %s ===\n' "$1"; }
j()  { curl -sS --noproxy '*' "$@"; echo; }

say '0. clean up orphan core'
pkill -f '/usr/bin/sing-box run -c' 2>/dev/null || true
sleep 1

say '1. ensure API is up (reuse detached instance if healthy)'
api_healthy() {
  curl -sS --noproxy '*' --max-time 2 "$BASE_DIRECT/api/health" 2>/dev/null | grep -q '"status"'
}
if ! api_healthy; then
  cd "$DIR"
  export V2RAYN_WEB_API_KEY="$KEY"
  export V2RAYN_DATA_HOME="$DIR/data"
  export V2RAYN_CORE_BIN="$(command -v sing-box)"
  setsid nohup bash termux-run.sh > api.log 2>&1 < /dev/null &
  disown || true
fi

say '2. wait for health'
for i in $(seq 1 20); do api_healthy && break; sleep 1; done
if ! api_healthy; then
  echo 'API DID NOT COME UP'; cat api.log; exit 1
fi
curl -sS --noproxy '*' "$BASE_DIRECT/api/health"; echo

say '3. api process'
ps -eo pid,ppid,stat,cmd | grep '[v]2rayn-webapi-rs' || true

say '4. login'
LOGIN=$(j -X POST "$BASE_DIRECT/api/auth/login" -H 'Content-Type: application/json' -d "{\"key\":\"$KEY\"}")
echo "$LOGIN" | head -c 200; echo
TOK=$(printf '%s' "$LOGIN" | sed -E 's/.*"token":"([^"]+)".*/\1/')
echo "token_len=${#TOK}"
[[ ${#TOK} -gt 20 ]] || { echo 'LOGIN FAILED'; exit 1; }
AUTH="Authorization: Bearer $TOK"

say '5. core/status initial'
j -H "$AUTH" "$BASE_DIRECT/api/core/status"

say '6. core/start'
j -X POST -H "$AUTH" "$BASE_DIRECT/api/core/start"

say '7. core/status after start'
j -H "$AUTH" "$BASE_DIRECT/api/core/status"

say '8. generated config'
head -c 1000 "$DIR/data/binConfigs/config.json" 2>/dev/null; echo

say '9. processes after start'
ps -eo pid,ppid,stat,cmd | grep -E '[s]ing-box|[v]2rayn-webapi-rs' || true

say '10. listener check'
ss -lntp 2>/dev/null | grep 10808 || netstat -lntp 2>/dev/null | grep 10808 || echo 'no 10808 listener in ss/netstat'

say '11. core/restart'
j -X POST -H "$AUTH" "$BASE_DIRECT/api/core/restart"
sleep 2
j -H "$AUTH" "$BASE_DIRECT/api/core/status"

say '12. core/stop'
j -X POST -H "$AUTH" "$BASE_DIRECT/api/core/stop"
j -H "$AUTH" "$BASE_DIRECT/api/core/status"

say '13. processes after stop'
ps -eo pid,ppid,stat,cmd | grep -E '[s]ing-box|[v]2rayn-webapi-rs' || true

say '14. nginx proxy smoke (GET status)'
j -H "$AUTH" "$BASE_NGINX/api/core/status"

say '15. api.log tail'
tail -n 30 api.log
