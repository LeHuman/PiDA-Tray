# PiDA Tray — Architecture & Implementation Handoff

Build a lightweight, low-footprint Windows system tray application (**PiDA Tray**) in **pure Rust** for controlling a [PiDA](https://github.com/LeHuman/PiDA) server over HTTP, modeled closely on the architecture, visual style, and CI/CD setup of `pigradient-tray`.

> **Hard Privacy Rule:** Never include personal IPs, hostnames, MACs, local paths, or real device/endpoint names (e.g., no specific room/device examples from any live install) in code, defaults, comments, tests, or git history. Use generic placeholders (`http://192.0.2.10:9999`, `living_room/lamp`, `office/fan`).

---

## 1. Core Goals & Constraints

1. **Near-zero idle resource usage:**
   - Hidden flyout at startup; early-return in `App::ui` when `!visible` (0% GPU/CPU render while idle).
   - No WebView2, no Electron, no Node/Python runtime. Single statically-linked native `.exe` via `eframe` (`glow` OpenGL backend).
   - No aggressive background polling: while hidden, either no polling (`0` / on-open only) or a slow, configurable `/api/health` ping (default `60s`, options `Off`, `30s`, `60s`, `120s`) using `tokio::select!` + `Notify` (zero busy-wait).
   - Diff-cached tray icon & tooltip updates: only call `Shell_NotifyIconW` when the rendered state key or tooltip text actually changes.
2. **Clean Material 3 dark interface:**
   - Taskbar-anchored borderless transparent flyout (`360px` wide, content-hugging height clamped to a max height with scroll if many groups).
   - Pure-vector UI + local `%WINDIR%\Fonts` (`Segoe UI`, `Consolas` fallback) — no remote fonts or CDNs.
   - Cache-first rendering: flyout opens immediately (<16ms) with last-confirmed schema, device states, and cached PNG icons; background refresh updates in place without flicker.
3. **Dynamic schema-driven controls:**
   - Discovers devices and actions from `GET /api/schema` (preserving server JSON key declaration order!).
   - Supports `bool` toggles, `int`/`float` sliders, `enum` segmented/dropdown controls, `string` inputs, and legacy one-shot action buttons.

---

## 2. Tech Stack & Crate Setup

Model `Cargo.toml` and `build.rs` on `pigradient-tray`:

- **Language:** Rust (edition 2024)
- **GUI / Tray:**
  - `eframe = { version = "0.37", default-features = false, features = ["accesskit", "clipboard", "default_fonts", "glow"] }`
  - `tray-icon` (with `muda` context menu)
  - `single-instance` (prevent duplicate processes)
- **Async / HTTP / Serialization:**
  - `tokio` (`rt`, `rt-multi-thread`, `sync`, `time`, `macros` — 2 worker threads)
  - `reqwest` (`default-features = false`, `features = ["native-tls", "json"]` — uses Windows SChannel, avoids CMake/NASM)
  - `serde`, `serde_json`
  - `dirs` (for `%APPDATA%\PiDA-Tray\`)
  - `image` (or minimal PNG decoder) *only* if decoding `/api/icon/<path>` PNGs into `egui::ColorImage` — otherwise keep dependency count minimal.
- **Windows integration (`[target.'cfg(windows)'.dependencies]`):**
  - `windows` (`Win32_Foundation`, `Win32_Graphics_Gdi`, `Win32_System_Power`, `Win32_System_Registry`, `Win32_UI_WindowsAndMessaging`)
  - `[build-dependencies] winresource` (embed `assets/icon.ico` + Explorer metadata in `build.rs`; downgrade errors to `cargo:warning` on non-Windows).
- **Release profile (`Cargo.toml`):**
  ```toml
  [profile.release]
  opt-level = "s"
  lto = "thin"
  codegen-units = 1
  strip = true
  ```

---

## 3. PiDA HTTP API Contract

Reference server: `LeHuman/PiDA` (`src/server.py`, `src/pipeline.py`). Reference Android client: `LeHuman/PiDA-App` (`PiApiClient.kt`, `ControlCatalog.kt`, `StateCache.kt`).

- **Auth:** None (trusted LAN/VPN; cleartext HTTP allowed).
- **Key ordering:** Server preserves YAML declaration order (`app.json.sort_keys = False`). **Do not** deserialize schema maps into `HashMap` / `BTreeMap` where order matters; use `serde_json::Map` (with `preserve_order` feature on `serde_json` if needed) or `Vec<(String, ...)>` so the first `bool` param and numeric `range` params keep server order.

### Endpoints

| Method | Path | Purpose & Payload |
|---|---|---|
| `GET` | `/api/health` | `{"ok": true, "deps": "<str>", "groups": [...], "endpoints": <int>}` (500 on bad YAML). |
| `GET` | `/api/schema` | `{"groups": {"<groupPath>": {"actions": {"<name>": {"params": {...}, "inputs": [...], "outputs": [...], "timeout": <num>?}}}}}`. Group key may be `""` for top-level actions. |
| `GET` | `/api/icon/<path>` | Returns `image/png` bytes, or `404 {"ok": false, "error": "no icon", "path": "..."}`. |
| `GET` | `/api/<path>` | Empty-delta state readback for parameterized endpoints. |
| `POST` | `/api/<path>` | Execute action. Parameterized: JSON partial-delta body `{"<param>": <value>}`. Legacy (no `params`): empty body `{}` or no body. |

### Schema & Parameter Rules

- `param` types:
  - `bool`: **must** dispatch real JSON boolean (`true`/`false`), never `1`/`0` or `"true"`.
  - `int`: dispatch JSON integer (`i64`), clamp to `[min, max]` if present.
  - `float`: dispatch JSON number (`f64`), clamp to `[min, max]` if present.
  - `enum`: requires `enum` array; dispatch exact string/value member.
  - `string`: JSON string.
- **Response shapes:**
  - Parameterized `200`: `{"ok": true, "<outputKey>": <value>, ...}` (server runs a built-in 4s settle loop and returns converged outputs).
  - Legacy `200`: `{"ok": true, "steps": [...], "path": "..."}`.
  - Errors: `400` validation, `404` unknown endpoint, `409` superseded in group queue, `500` execution failure, `503` group queue busy (>30s). All return `{"ok": false, "error": "..."}`.

---

## 4. Application Architecture & Modules

Mirror `pigradient-tray`'s clean module split:

```
pida-tray/
├─ Cargo.toml
├─ build.rs
├─ assets/                    # icon.ico, icon.svg, PNGs
├─ .github/workflows/         # ci.yml, cd.yml, audit.yml (+ dependabot.yml)
└─ src/
   ├─ main.rs                 # SingleInstance, 2-thread Tokio rt, hidden glow eframe, WM_POWERBROADCAST
   ├─ constants.rs            # App ID, timings, geometry (360px width), Material 3 palette
   ├─ config.rs               # settings.json persistence, URL normalization, HKCU Run autostart
   ├─ cache.rs                # Disk cache for schema + last-known group states + negative icon 404 cache
   ├─ api.rs                  # PiDA HTTP client (health, schema, icon, get_state, dispatch)
   ├─ catalog.rs              # Schema -> DeviceGroup / SetActionSpec / LegacyActionSpec mapping
   ├─ poller.rs               # SharedAppState, idle health ping, on-open schema+state sync, dispatch queue
   ├─ tray.rs                 # In-memory 32x32 RGBA status icon, tooltip, muda menu, work-area placement
   ├─ theme.rs                # Local Windows fonts, Material 3 Dark Visuals, contrast helpers
   └─ ui/
      ├─ mod.rs               # PiDaTrayApp (eframe::App), flyout show/hide, focus-loss grace (220ms), card shell
      ├─ header.rs            # Status dot, title, Refresh / Pin / Settings / Hide vector buttons
      ├─ devices.rs           # Scrollable DeviceGroup cards (toggle, sliders, enum pills, legacy buttons)
      ├─ settings.rs          # Inline settings form (Server URL, idle poll interval, pin, autostart, clear cache)
      └─ icons.rs             # Pure-vector Painter icons (power, light, fan, monitor, computer, refresh, gear, pin)
```

### Key Behavioral Details

1. **Catalog Mapping (`catalog.rs`)** — match `LeHuman/PiDA-App`'s `ControlCatalog.kt`:
   - Build one `DeviceGroup` per action-bearing group.
   - First action with `params` + `inputs` becomes the card's `SetActionSpec`:
     - `toggle_param`: first `bool` input.
     - `range_params`: `int`/`float` inputs in declaration order.
     - `enum_params`: `enum` inputs in declaration order (render as segmented pills if `<= 4` options, dropdown otherwise — fixing the Android app's omission).
   - Actions without `params`/`inputs` become `LegacyActionSpec` tap buttons.
   - Merge sub-groups that only have legacy actions into their longest-prefix parent `DeviceGroup` (e.g., `room/light/cycle` actions appear on `room/light`).
   - Prettify group/action names (`split('_')`, Title Case). Infer fallback vector icon from leaf name (`light`/`lamp`, `fan`, `monitor`/`tv`, `computer`/`pc`, generic power).

2. **Cache-First & Optimistic State (`cache.rs` + `poller.rs`):**
   - Persist `schema.json` and `group_states` to `%APPDATA%\PiDA-Tray\cache.json` (atomic write or clean fallback on parse error).
   - Cache `/api/icon/<path>` PNGs under `%APPDATA%\PiDA-Tray\icons\<sanitized>.png` + in-memory texture map. Keep a session-level negative cache for `404` icon paths so missing icons are never re-requested repeatedly.
   - On flyout open: render cached catalog + states immediately, then trigger async `/api/schema` + per-group `GET /api/<set_path>` readbacks. Only remove a group from cache when a fresh `/api/schema` confirms it is gone.
   - On control interaction:
     - **Toggle:** optimistic flip → `POST` real bool → write-through response outputs to cache; on error, revert to last confirmed state and show inline error banner.
     - **Slider:** update local UI value smoothly while dragging; debounce `250ms` (or dispatch on pointer release) before `POST`; write-through response outputs.
     - **Legacy button:** show subtle in-flight spinner/highlight → `POST` → update status.

3. **Window & Tray Behavior (`tray.rs` + `ui/mod.rs`):**
   - Left-click tray icon toggles flyout; right-click opens `muda` menu (`Open / Hide Flyout`, `Refresh`, `Settings...`, `Quit PiDA Tray`).
   - Placement: `MonitorFromPoint` + `GetMonitorInfoW` work area, centered horizontally on icon, anchored above bottom taskbar (or below top taskbar), `8pt` screen margin (`FLYOUT_SCREEN_MARGIN`).
   - Auto-hide on focus loss after `220ms` grace unless pinned; `Esc` exits settings or hides flyout; OS close (`Alt+F4`) cancels close and hides unless Quit was triggered.
   - Hidden Win32 message window listens for `WM_POWERBROADCAST` (`PBT_APMRESUMEAUTOMATIC`) to wake `poller` and refresh health/state after sleep.

4. **Settings (`config.rs`):**
   - `%APPDATA%\PiDA-Tray\settings.json` (`#[serde(default)]` on all fields):
     - `server_url: String` (default `""` — show inline "Configure Server URL" prompt in flyout when empty)
     - `idle_poll_seconds: u64` (default `60`, `0` = disabled/on-open only)
     - `pin_flyout: bool` (default `false`)
     - `launch_at_startup: bool` (default `false` → `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\PiDATray`)
   - `normalize_server_url`: prepend `http://` if scheme missing, validate `http`/`https` + host, strip trailing `/`.

---

## 5. CI/CD, Versioning & Repo Setup

Align with `LeHuman/PiDA` and `LeHuman/pigradient-tray`:

- **GitHub Repo:** `LeHuman/PiDA-Tray` (public, `main` default branch).
- **Commit Format:** gitmoji + Conventional Commits (`✨ feat:`, `🐛 fix:`, `👷 ci:`, `📝 docs:`, `🔧 chore:`, `🎨 assets:`, `✅ test:`).
- **Versioning Scheme:**
  - Tag builds (`v*` or `x.x.x`): strip optional leading `v` (`${ver#v}`).
  - Non-tag builds: `0.0.0+sha.<7>`.
- **GitHub Actions (`.github/workflows/`):**
  1. `ci.yml` (push `main`, PRs, `workflow_dispatch`):
     - `windows-latest`, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`
     - Jobs: `test` (`cargo test --all-features`), `fmt` (`cargo fmt --all -- --check`), `clippy` (`cargo clippy --all-targets --all-features -- -D warnings`), `doc` (`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items`), `build` (`cargo build --release`, upload `pida-tray-windows-x86_64.exe` artifact).
  2. `cd.yml` (tags `v*`, `[0-9]+.[0-9]+.[0-9]+`, `workflow_dispatch`):
     - Build `--release`, stage `target/release-dist/pida-tray-windows-x86_64.exe` + `.sha256` checksum.
     - Publish GitHub Release using `softprops/action-gh-release@v2` (`generate_release_notes: true`).
  3. `audit.yml` + `.github/dependabot.yml`:
     - `rustsec/audit-check@v2.0.0` on schedule + `Cargo.lock` changes; weekly Dependabot for `cargo` and `github-actions`.
- **VS Code (`.vscode/`):**
  - `tasks.json` (build debug/release, test, clippy `-D warnings`, fmt), `launch.json` (`cppvsdbg` / CodeLLDB). (Do **not** copy `pigradient-tray`'s stale `src-tauri` `settings.json`.)

---

## 6. Verification Checklist (Before Ingestion)

1. `cargo fmt --all -- --check` passes.
2. `cargo clippy --all-targets --all-features -- -D warnings` passes with zero warnings.
3. `cargo test --all-features` passes (unit tests for URL normalization, schema parsing & key order preservation, sub-group merging, cache round-trip, tooltip clamping).
4. `cargo build --release` produces a single `.exe` in `target/release/pida-tray.exe` with embedded `.ico`.
5. Manual smoke test on Windows:
   - Starts hidden in tray with no console window (`#![windows_subsystem = "windows"]` in release) and ~0% CPU when idle.
   - Left-click opens flyout anchored cleanly above taskbar; Esc / click-away hides it.
   - Unconfigured state prompts for Server URL; configuring a PiDA URL populates devices from `/api/schema`, toggles/sliders/enums/buttons dispatch and persist state across flyout reopen.
   - Offline server shows cached devices + offline indicator without crashing or clearing cache.
