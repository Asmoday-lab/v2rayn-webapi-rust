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

## Same-origin requirement

Do **not** serve Nova from an HTTPS origin while the API listens on plain HTTP
localhost. That is mixed content / Private Network Access, and CORS cannot
override it. Either use the same-origin hosting above, or put both behind one
reverse proxy. The API's Origin guard and CORS already accept same-origin
requests.

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
