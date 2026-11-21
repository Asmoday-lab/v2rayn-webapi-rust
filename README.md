# v2rayN WebAPI (Rust)

Headless WebAPI for Termux, contract-compatible with [v2rayN.WebAPI](https://github.com/2dust/v2rayN/tree/master/v2rayN/v2rayN.WebAPI) and [v2rayN-WebUI-Nova](https://github.com/Nozilla-X/v2rayN-WebUI-Nova).

## Scope (current milestone)

Contract-compatible headless WebAPI for Termux. Every implemented endpoint is verified
byte-for-byte against the pinned C# oracle in CI.

- [x] Independent repo, does not touch the upstream v2rayN repository
- [x] x64 CI: fmt / clippy (`-D warnings`) / tests / dependency guard
- [x] aarch64-unknown-linux-musl static release build in CI (zig + cargo-zigbuild), verified `statically linked` and no `INTERP`
- [x] `oracle.lock` pins both upstream C# and Nova commits; daily drift watch opens a PR instead of silently advancing the pin
- [x] dependency graph guard: no `aws-lc-rs` / `openssl-sys` / `rusqlite` / native-tls
- [x] Full C# security chain: security headers, 64 KiB body cap, Origin guard (403), fixed-window rate limit, Bearer session auth, one-shot SSE ticket auth
- [x] Auth/session endpoints: login (PBKDF2-verified key), logout, sse-ticket, setup/status, setup
- [x] `/api/health` with the exact 5 diagnostic headers under `WebSetupAccessPolicy`
- [x] 16 static/near-static GET endpoints (status, editor-options, settings + dns, profile-groups, core/web-updates, operations, subscriptions, profiles…) with data captured from the oracle golden sample and embedded as JSON resources
- [x] SSE `/api/events` (heartbeat stream, one-shot ticket auth)
- [x] Differential suite: **59/59 cases match** the pinned C# oracle in CI, including auth, CORS 403, session, SSE ticket, all static endpoints, deterministic headless write operations, and validation error paths
- [x] Nova startup path complete: `/api/setup/status` → login → status/groups/profiles/subscriptions/settings/routing + `/api/logs/page` (shape-checked) + SSE ticket
- [x] Optional same-origin Nova SPA hosting via `NOVA_DIST_DIR` (avoids mixed-content / Private Network Access on HTTPS)
- [x] Oracle golden-sample dump (GET data + `__mutations__` write responses) uploaded as a CI artifact for DTO parity
- [ ] Core process management (Xray + sing-box)
- [ ] Real state mutation (profiles/subscriptions/settings persistence, backup, update triggers) — currently only the headless-deterministic no-op/error paths are implemented
- [ ] Termux DNS / CA / wakelock adaptation

## Differential test

The `differential` CI job is the authority for "contract-compatible". It checks out the
exact C# commit from `oracle.lock`, publishes it with .NET 10, starts it headless on
`:5081` using the same recipe as upstream's `test-startup-security.sh` (valid management
key, isolated `V2RAYN_DATA_HOME`, autostart off, loopback URL, `--foreground --no-open`),
starts the Rust build on `:5082`, and runs `scripts/differential.py`. That script sends
the identical request to both and compares status, JSON body, and the *set* of health
diagnostic header names (values are process/version-specific and normalized out).

To reproduce locally you need `dotnet` 10.x installed (CI has it via `setup-dotnet`):

```bash
# 1. fetch the pinned C# source into oracle-src/
# 2. run the whole thing (starts/stops both servers itself):
bash scripts/differential.sh "$PWD/oracle-src"
```

The corpus currently has 59 cases: health (plain / forwarded / foreign-origin-403),
setup-status, login (wrong / empty / valid key), unauthenticated rejections, all
18 static GET endpoints (including `/api/web-updates` and the `/api/logs/page`
pagination shape), SSE no-ticket / bogus-ticket 401, authenticated
sse-ticket minting, 11 deterministic write operations captured in the oracle's
`__mutations__` golden sample, and 18 validation/Core-error paths transcribed
from the pinned C# `V2rayRuntime.*.cs` sources. Volatile values (session/ticket
tokens, timestamps, random entity ids, log-clear generation counters,
environment diagnostic strings) are normalized before comparing. Log *content*
is process-local, so `/api/logs/page` is compared by envelope + pagination
shape only. A per-run golden sample is uploaded as the `oracle-golden-sample`
artifact and is the authoritative source for the embedded DTO resources in
`resources/`.

Operations that spawn Core processes, network downloads or real file writes are
deliberately **excluded** from the corpus (e.g. `/api/core-updates/batch`, whose
`apply=false` still launches `RunSelectedCoreUpdateBatchAsync`). See the
`unsafe-update-ops-excluded` hotspot in `contract.yaml` before adding a case.

## Local vs CI compilation policy

| Operation | Local | CI |
|---|---|---|
| `cargo fmt` / `cargo check` / `cargo clippy` | allowed | runs on every push/PR |
| `cargo test` (x64 debug) | allowed | runs on every push/PR |
| `cargo build` (x64 debug) | allowed | - |
| **aarch64-musl release artifact** | **banned by policy** | **only here** (`build-aarch64-musl` job) |

The ban on local release builds is a project policy, enforced by workflow design rather than by a technical guard: only CI has the zig toolchain step, and only CI artifacts are published.

## Local verification

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
bash scripts/check-deps.sh
```

## CI

`.github/workflows/ci.yml` runs on push/PR (verify job) and nightly 03:17 UTC (contract-watch job).

- **verify** — x64 fast feedback: fmt, clippy, tests, dependency guard, `oracle.lock` schema.
- **differential** — x64, checks out the pinned C# commit, builds and starts both servers headless, runs the request corpus against both. This is the contract authority; needs-dotnet 10.x.
- **build-aarch64-musl** — release static binary, verified with `file` and `readelf`, uploaded as artifact `v2rayn-webapi-rs-aarch64-musl`.
- **contract-watch** — nightly, compares `oracle.lock` to upstream HEAD; on drift, advances the pin in a branch and opens a PR. Requires `CONTRACT_WATCH_TOKEN` secret **only if** the default `GITHUB_TOKEN` cannot push branches in your repo settings; otherwise leave it unset. Review the PR before merging so the differential baseline change is visible.

## Static binary

The shipping artifact is a fully static musl ELF with no `INTERP` segment, so the Android kernel loads it directly without the Bionic linker and it runs natively in Termux without proot.

```
aarch64-unknown-linux-musl, statically linked, no INTERP
```

## Same-origin Nova hosting

Point `NOVA_DIST_DIR` at the directory containing Nova's built `index.html` and assets:

```bash
NOVA_DIST_DIR=$HOME/v2rayN-WebUI-Nova/dist ./v2rayn-webapi-rs
```

The `/api/*` router keeps priority; every other path is served from that directory with an SPA fallback to `index.html`. Path traversal, backslashes and NUL bytes are rejected. When the variable is unset the process stays API-only. Without same-origin hosting, an HTTPS-served Nova build cannot call the loopback HTTP API (mixed content / Private Network Access), and CORS cannot override that.

## Termux deployment

See [docs/TERMUX.md](docs/TERMUX.md) for the full setup: artifact install,
Nova same-origin hosting, the Android PIE trap for core binaries, explicit DNS,
and wakelock / battery optimization. `scripts/termux-run.sh` is a ready-to-use
launcher with wakelock acquisition.

There is no separate release workflow: the shipping artifact is produced only
by the `build-aarch64-musl` CI job (**release builds are CI-only by policy**).
Download `v2rayn-webapi-rs-aarch64-musl.zip` from any successful run and record
its run id + commit SHA.

## Termux acceptance (not covered by CI)

CI cannot prove native execution, browser login, or screen-off survival. The
on-device procedure is in [docs/ACCEPTANCE.md](docs/ACCEPTANCE.md); the automated
part is `scripts/termux-smoke.sh` (run it with `API_BIN` and `NOVA_DIST_DIR`
set). It covers:

1. Static ELF actually `execve`s on Android/Bionic (no proot, no root).
2. Full local auth chain + 5 health diagnostic headers + Nova same-origin load.
3. Wakelock / battery-optimization survival with screen off.
4. Device-specific DNS caveats.

## Known limitations (not smoke failures)

- Core process management (Xray + sing-box) is **not implemented**:
  `POST /api/core/start` / `restart` return the oracle's deterministic
  `409` errors. Nova can browse/manage but cannot open a proxy connection yet.
- Real persistence of profiles/subscriptions/settings is **not implemented**;
  writes are deterministic no-op/error paths and do not survive restart.
- The API binds loopback only; WAN access needs an SSH tunnel or reverse proxy.

## Contract tracking

`oracle.lock` pins the exact upstream commit used as the differential oracle and the exact Nova commit whose `Doc/api.md` is the client contract. `contract.yaml` maps contract surfaces to their defining upstream files and lists semantic hotspots (health headers, enum casing, case-insensitive duplicate handling, session, SSE ticket, setup policy, CORS order, capabilities). Tracking upstream is an explicit choice: on every contract change, this repo must re-run the differential suite and port the delta. It is not a frozen/one-time implementation.
