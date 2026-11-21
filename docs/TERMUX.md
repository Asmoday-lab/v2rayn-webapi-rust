# Termux deployment

This document describes how to run `v2rayn-webapi-rs` natively on Termux and
serve the Nova web UI from the same origin. It records the device-specific
pitfalls that CI cannot reproduce.

## Why static musl

The upstream `dotnet publish -r linux-arm64` output is a glibc-linked .NET
binary. Android/Termux uses Bionic libc and has no glibc loader, so that binary
cannot run without proot. The CI artifact is a fully static
`aarch64-unknown-linux-musl` ELF with no `INTERP` segment: the Android kernel
loads it directly, no proot and no root required.

## Prerequisites

1. Install Termux from F-Droid or the project's GitHub release (the Play Store
   build is abandoned and its packages are stale).
2. Install the base tools:

   ```sh
   pkg update && pkg upgrade -y
   pkg install curl wget jq termux-api -y
   ```

3. Get the API binary from CI. Download the `v2rayn-webapi-rs-aarch64-musl`
   artifact from the latest successful run, unzip it, and place it somewhere
   persistent, e.g. `~/bin/v2rayn-webapi-rs`. Verify it is static:

   ```sh
   readelf -l ~/bin/v2rayn-webapi-rs | grep INTERP   # must print nothing
   file ~/bin/v2rayn-webapi-rs                       # statically linked
   ```

4. Build the Nova UI (node is not in Termux by default; build on a workstation
   and copy the result). Place the built `dist/` directory somewhere stable,
   e.g. `~/v2rayN-WebUI-Nova/dist`.

## Launch

```sh
export V2RAYN_WEB_API_KEY='choose-a-long-random-key'
export NOVA_DIST_DIR="$HOME/v2rayN-WebUI-Nova/dist"
export ASPNETCORE_URLS='http://127.0.0.1:5080'
~/bin/v2rayn-webapi-rs
```

Then open `http://127.0.0.1:5080/` in the device browser. The API router keeps
priority on `/api/*`; every other path is served from `NOVA_DIST_DIR` with an
SPA fallback to `index.html`.

If `V2RAYN_WEB_API_KEY` is unset, the API starts in setup mode: Nova will show
the first-run setup screen and store the key through `POST /api/setup`. The key
is then verified with PBKDF2-HMAC-SHA256 (600k iterations) on every login.

For a repeatable launch including wakelock, use the helper script:

```sh
bash scripts/termux-run.sh
```

## Accessing from a workstation browser

The API binds **loopback only** (`127.0.0.1:5080` on the phone). The device's
Wi-Fi address (e.g. `192.168.1.5`) is *not* listening, so opening
`http://192.168.1.5:5080/` from a PC gets a connection refusal. Android also
filters inbound connections to Termux, and some routers isolate Wi-Fi clients.
Use an SSH local forward instead — it terminates on the phone's own loopback
and reaches the browser as the same origin:

```sh
ssh -N -o BatchMode=yes -o ExitOnForwardFailure=yes \
  -o ServerAliveInterval=30 -L 5080:127.0.0.1:5080 mi5x
```

Then open **http://127.0.0.1:5080/** on the PC and fill the Nova login form:

| Field | Value |
| --- | --- |
| Backend API 地址 | **Leave empty.** Nova is served from this exact origin, so its same-origin default is correct. Explicit `http://127.0.0.1:5080` also works. |
| 管理密钥 | The value of `V2RAYN_WEB_API_KEY` used by `termux-run.sh` (on the recorded device: `ondevice-verification-key-1234`). |

Do not enter `192.168.1.5`, `localhost:5081`, or the SSH host name: a
different origin would require `V2RAYN_WEB_ALLOWED_ORIGINS` and would reintroduce
CORS / Mixed Content / Private Network Access problems. The WiFi icon in the
form is only a connectivity test; it does not substitute for the management key.

`scripts/pc-tunnel.sh` starts the same forward if you prefer a script.

## Serving Nova from an existing nginx (aggregation page)

When the device already runs nginx as the LAN entry point, keep the API as the
loopback-only process and let nginx own the public origin. Run
`scripts/termux-nginx-deploy.py` on the device after each Nova rebuild:

```sh
python3 ~/v2rayn-nova/termux-nginx-deploy.py
nginx -t -p "$HOME/.nginx" -c "$HOME/.config/nginx/nginx.conf"
nginx -s reload -p "$HOME/.nginx" -c "$HOME/.config/nginx/nginx.conf"
```

It copies `~/v2rayn-nova/nova-dist` to `~/.config/nginx/html/nova/`, injects
`<base href="/nova/">` so Vue history-mode deep links still resolve their
relative chunks, adds the Nova card to the aggregation page, and inserts two
locations into the existing `nginx.conf` (CRLF preserved, idempotent):

```nginx
location /nova/ {
    root   /data/data/com.termux/files/home/.config/nginx/html;
    index  index.html;
    try_files $uri $uri/ /nova/index.html;
}
location /api/ {
    proxy_pass http://127.0.0.1:5080;
    proxy_http_version 1.1;
    proxy_set_header Host $http_host;   # must forward: Origin guard compares against Host
    proxy_buffering off;                # SSE on /api/events
    proxy_read_timeout 3600s;
}
```

`proxy_set_header Host $http_host` is the load-bearing line: the API's
`is_same_origin()` accepts a request when `Origin == http://<Host>`. Forwarding
the browser's original Host makes the nginx-proxied call same-origin, so no
`V2RAYN_WEB_ALLOWED_ORIGINS` entry or `apiBaseUrl` change is needed — the shipped
`webui-config.js` already has `apiBaseUrl: ''`. A foreign Origin is still 403.

Nova is then available at `http://192.168.1.5:8080/nova/` (or the device's
current LAN IP / `alisa.v6.rocks`), and the API's own same-origin hosting on
`:5080` stays available for the SSH-tunnel path. Both coexist; only one needs
to be used. Back up `nginx.conf` and the portal `index.html` before first run
(the script is idempotent but does overwrite `html/nova/` on every rebuild).

**Prefer the nginx path over the SSH tunnel.** nginx binds `0.0.0.0:8080`, so a
workstation on the same LAN can reach `http://<device-ip>:8080/nova/` directly —
no tunnel, and it survives Wi-Fi sleep that resets an idle SSH session
(`Connection reset by peer`). The tunnel below is only a fallback for when no
LAN route exists. Keep the login values the same.

## Same-origin requirement

Do **not** serve Nova from an HTTPS origin while the API listens on plain HTTP
localhost. That is mixed content / Private Network Access, and CORS cannot
override it. Either use the same-origin hosting above, or put both behind one
reverse proxy. The API's Origin guard and CORS already accept same-origin
requests.

## Real persistence and Core management

The original headless build returned deterministic golden/no-op responses: Nova
could log in and render, but writes did nothing and the data directory stayed
empty. The current build replaces those with a real store and real Core control:

- **Store**: `V2RAYN_DATA_HOME/data.json` (default `./data`). A single pretty
  JSON document written atomically (temp + rename). A fresh/missing file is
  seeded from the embedded golden DTOs, so an empty install answers exactly like
  the pinned C# oracle — that is what keeps the CI differential suite green.
- **CRUD**: subscriptions and profiles (create/read/update/delete/select), plus
  settings sections (inbound/core/application/speedtest/routing/apply), DNS
  simple + profiles, WebDAV, and core templates now read/write the store.
- **Core**: `POST /api/core/start` generates a sing-box config from the
  selected profile + settings into `V2RAYN_DATA_HOME/binConfigs/config.json`,
  then spawns the core with `run -c <path> --disable-color`. Stop/restart/status
  manage the live child PID. Override the binary with `V2RAYN_CORE_BIN`
  (default `sing-box`). Supported outbounds: VMess, Shadowsocks, VLESS, Trojan,
  Hysteria2, TUIC, WireGuard; SOCKS fallback for unknown types.

Wire shapes stay camelCase `ApiEnvelope<T>` and the security chain is unchanged.
The differential corpus still asserts the exact failure codes on empty/invalid
input (verified locally: 13/13 GET golden parity, 11/11 write-case parity).

On-device preparation:

```sh
pkg install sing-box -y           # 1.14.4, android/arm64 PIE
readelf -h "$(command -v sing-box)" | grep 'Type:'   # want DYN (PIE)
```

Launcher env additions:

```sh
export V2RAYN_DATA_HOME="$HOME/v2rayn-nova/data"   # persists across restarts
export V2RAYN_CORE_BIN="$(command -v sing-box)"     # optional; default sing-box
bash scripts/termux-run.sh                          # forwards both env vars
```

Still out of scope (return JSON 404 / no-op): subscription content fetching
(share URLs are stored, not downloaded), active speed/latency tests, backup /
WebDAV restore, core/web self-updates, and geo file updates. Full routing-rule
and DNS-profile *editing* CRUD is implemented for the common fields; exotic
protocol-specific extras round-trip as stored JSON but are not individually
validated against every C# rule.

## Core binaries: the PIE trap

Generic Linux ARM64 release binaries of proxy cores are frequently **non-PIE**
`ET_EXEC` executables (`e_type: 2`). Android's linker rejects them with:

```
has unexpected e_type: 2
```

This is independent of our Rust binary: it affects the Xray/sing-box process
that the API would launch. Use the Termux-native package instead:

```sh
pkg install sing-box -y
sing-box version
```

`sing-box` from the Termux repo is built as PIE for Bionic and runs directly.
Verify any other core before trusting it:

```sh
readelf -h "$(command -v xray)" | grep 'Type:'   # want DYN (PIE), not EXEC
```

Do not replace it with the generic tarball from a GitHub release just because
the version number looks newer.

## DNS

Termux has no conventional `/etc/resolv.conf` pointing at a system resolver.
Go-based cores (Xray) have been observed trying `[::1]:53` and failing with
`connection refused`, even when their own logs look healthy. Configure DNS
explicitly in the core config rather than relying on the system:

```json
"dns": { "servers": ["8.8.8.8", "1.1.1.1"] }
```

On flaky IPv6 routes, also force IPv4 for outbound connections
(`domainStrategy: UseIP` for Xray). These are core-side settings; the Rust API
does not rewrite core configs.

## Keeping the processes alive

Android kills background processes aggressively. All three steps are needed
for unattended operation:

1. Acquire a wakelock: `termux-wake-lock` (the helper script does this).
   Release with `termux-wake-unlock` when done.
2. Disable battery optimization for Termux in Android settings, and keep
   Termux pinned in the recent-apps list.
3. Prefer Wi-Fi over mobile data for a permanently installed node.

Without a wakelock the process typically dies within minutes of screen off.

## Current limitations

- The Rust API currently implements deterministic headless responses for core
  lifecycle endpoints (`POST /api/core/start` and `/api/core/restart` return
  the pinned oracle's no-profile / not-running errors). **Real core process
  launch and config generation are not implemented yet**, so Nova can manage
  profiles/settings but cannot actually start a proxy connection today.
- The API binds to loopback only. Remote access requires a reverse proxy or an
  SSH tunnel; do not expose the plain HTTP listener directly.
- These device-specific behaviours are not covered by CI and need a real
  handset to verify end to end.
