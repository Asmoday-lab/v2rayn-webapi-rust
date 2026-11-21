# Termux on-device acceptance checklist

CI proves the Rust binary is a static `aarch64-unknown-linux-musl` ELF and that
59 HTTP cases match the pinned C# oracle. It **cannot** prove the binary runs on
Android/Bionic, that Nova loads from it, or that it survives screen-off. This
checklist covers those gaps on a real device.

Use [`scripts/termux-smoke.sh`](../scripts/termux-smoke.sh) for the automated
parts, then walk the manual checks below. Record results against a specific
artifact run (commit SHA + CI run id).

## 0. Artifact identity

- [ ] Download `v2rayn-webapi-rs-aarch64-musl.zip` from a **successful** CI run.
- [ ] Record the run id and commit SHA: `__________`.
- [ ] On device, verify the ELF is static:

  ```sh
  file v2rayn-webapi-rs        # ELF 64-bit LSB executable, ARM aarch64, statically linked
  readelf -l v2rayn-webapi-rs | grep INTERP   # must print NOTHING
  ```

  If `readelf`/`file` are unavailable in Termux (`binutils`) that is fine — CI
  already verified both; the real proof is step 1 (it executes).

## 1. Native execution (no proot, no root)

- [ ] `chmod +x v2rayn-webapi-rs`
- [ ] Run it directly — **not** under `proot`, not with `sudo`:

  ```sh
  V2RAYN_WEB_API_KEY='a-long-random-key-1234' ./v2rayn-webapi-rs
  ```

- [ ] Process starts and logs `v2rayN WebAPI (Rust) starting`.
- [ ] **Expected pass**: it runs at all. A glibc binary would fail with
      `cannot execute: required file not found` (missing glibc loader). A
      non-PIE core binary would fail with `has unexpected e_type: 2`.
- [ ] **If it fails**: confirm `uname -m` is `aarch64`; re-download the artifact
      from a green run; do not build locally (release builds are CI-only by
      policy).

## 2. Automated smoke suite

- [ ] Build/copy Nova's `dist/` to the device, e.g. `~/v2rayN-WebUI-Nova/dist`.
- [ ] Run:

  ```sh
  API_BIN="$HOME/v2rayn-webapi-rs" \
  NOVA_DIST_DIR="$HOME/v2rayN-WebUI-Nova/dist" \
    bash scripts/termux-smoke.sh
  ```

- [ ] All checks pass: ELF sanity, health + 5 diagnostic headers, wrong-key
      401, valid login issues token, protected endpoint 401→200, SSE ticket,
      same-origin `GET /` serves Nova, path traversal rejected.
- [ ] The smoke script kills its own API process on exit; confirm no listener
      remains: `curl --noproxy '*' http://127.0.0.1:5090/api/health` fails.

## 3. Nova end-to-end (browser on the device)

- [ ] Launch via the helper (acquires wakelock):

  ```sh
  V2RAYN_WEB_API_KEY='a-long-random-key-1234' \
  NOVA_DIST_DIR="$HOME/v2rayN-WebUI-Nova/dist" \
    bash scripts/termux-run.sh
  ```

- [ ] Open `http://127.0.0.1:5080/` in the device browser (same origin as API).
- [ ] First-run setup screen appears if no key was pre-set; complete setup.
- [ ] Login with the management key succeeds (no CORS/mixed-content error in
      browser console). If console shows `Mixed Content` or `Private Network
      Access`, Nova is NOT being served from the API origin — go back to step 3
      launch command; do not try to "fix" it with CORS.
- [ ] After login the main shell loads: status, profile groups, profiles,
      subscriptions, settings, routing all render (these are the golden-sample
      endpoints; empty lists are expected on a fresh data home).
- [ ] SSE connects (the connection indicator stabilizes). Expect a 60s
      keep-alive comment; no need to keep the tab open for the API to run.

## 4. Wakelock / background survival

- [ ] `termux-wake-lock` acquired (the launcher prints so). If unavailable,
      install `termux-api` and confirm the Termux notification shows a wakelock.
- [ ] Android Settings → Battery → disable optimization for Termux; pin Termux
      in the recent-apps list.
- [ ] Turn the screen off for 10+ minutes on Wi-Fi.
- [ ] Screen on: `curl --noproxy '*' http://127.0.0.1:5080/api/health` still
      returns ok and the PID is unchanged (compare `x-v2rayn-webapi-instance-pid`).
- [ ] **If the process died**: wakelock was not held or battery optimization is
      still on. This is the most common real-device failure and is unrelated to
      the binary.

## 5. DNS (device-specific, no CI coverage)

- [ ] Android has no conventional `/etc/resolv.conf`. Any component that resolves
      names via the system resolver may fail with `[::1]:53 connection refused`.
- [ ] This API itself makes no outbound DNS calls, so the smoke suite passing is
      sufficient for the API. DNS becomes relevant only once a real Core is
      wired in (future phase) — then configure explicit upstream DNS in the
      Core config, never rely on the system resolver.

## 6. Known out-of-scope (do NOT report as a smoke failure)

- Core process management (Xray/sing-box start/stop/status) is **not
  implemented**. `POST /api/core/start` returns the oracle's deterministic
  `409 profile_not_selected`; Nova can browse/manage but cannot establish a
  proxy connection yet.
- Real persistence of profiles/subscriptions/settings is **not implemented**;
  writes are deterministic no-op/error paths. Data does not survive restart.
- Remote access: the API binds loopback only. WAN access needs an SSH tunnel or
  reverse proxy; do not expose the plain HTTP listener directly.

## Reporting a result

When filing an issue, include: CI run id + commit SHA, `uname -m`,
`termux-info` output, the smoke suite pass/fail counts, and the API log tail.
For step-3 browser failures, include the console error verbatim (especially
whether it is `Mixed Content` / PNA vs an auth error).
