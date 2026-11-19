# v2rayN WebAPI (Rust)

Headless WebAPI for Termux, contract-compatible with [v2rayN.WebAPI](https://github.com/2dust/v2rayN/tree/master/v2rayN/v2rayN.WebAPI) and [v2rayN-WebUI-Nova](https://github.com/Nozilla-X/v2rayN-WebUI-Nova).

## Scope (current milestone)

This is the **skeleton milestone**: CI toolchain is being brought up green before business logic is ported.

- [x] Independent repo, does not touch the upstream v2rayN repository
- [x] x64 CI: fmt / clippy (`-D warnings`) / tests / dependency guard
- [x] aarch64-unknown-linux-musl static release build in CI (zig + cargo-zigbuild), verified `statically linked` and no `INTERP`
- [x] `oracle.lock` pins both upstream C# and Nova commits; daily drift watch opens a PR instead of silently advancing the pin
- [x] `/api/health` body matches the C# contract `{"status":"ok"}` with diagnostic headers
- [x] dependency graph guard: no `aws-lc-rs` / `openssl-sys` / `rusqlite` / native-tls
- [ ] Core process management (Xray + sing-box)
- [ ] Profile/subscription/settings endpoints (93 REST + 1 SSE)
- [ ] Differential test request corpus (harness skeleton only)
- [ ] Termux DNS / CA / wakelock adaptation

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
- **build-aarch64-musl** — release static binary, verified with `file` and `readelf`, uploaded as artifact `v2rayn-webapi-rs-aarch64-musl`.
- **contract-watch** — nightly, compares `oracle.lock` to upstream HEAD; on drift, advances the pin in a branch and opens a PR. Requires `CONTRACT_WATCH_TOKEN` secret **only if** the default `GITHUB_TOKEN` cannot push branches in your repo settings; otherwise leave it unset. Review the PR before merging so the differential baseline change is visible.

## Static binary

The shipping artifact is a fully static musl ELF with no `INTERP` segment, so the Android kernel loads it directly without the Bionic linker and it runs natively in Termux without proot.

```
aarch64-unknown-linux-musl, statically linked, no INTERP
```

## Termux acceptance (not covered by CI)

CI cannot prove these; they need a real device:

1. DNS resolution (Android has no `/etc/resolv.conf`; plan: hickory with explicit upstream or route through core's socks outbound).
2. Core binary `execve` works (Xray may ship as non-PIE `ET_EXEC` and be rejected by the Android linker).
3. Wakelock / battery-optimization exemption keeps both processes alive with screen off.
4. Nova `dist/` served from the same origin as the API, or a same-origin reverse proxy — otherwise HTTPS→HTTP localhost hits mixed-content / Private Network Access and CORS cannot override it.

## Contract tracking

`oracle.lock` pins the exact upstream commit used as the differential oracle and the exact Nova commit whose `Doc/api.md` is the client contract. `contract.yaml` maps contract surfaces to their defining upstream files and lists semantic hotspots (health headers, enum casing, case-insensitive duplicate handling, session, SSE ticket, setup policy, CORS order, capabilities). Tracking upstream is an explicit choice: on every contract change, this repo must re-run the differential suite and port the delta. It is not a frozen/one-time implementation.
