# FINDINGS — ingestion & verification log

Initial scaffold was created offline without a Rust toolchain. During ingestion, the following issues were resolved and verified on Windows 11 with Rust 1.100.0:

## Resolved compile, toolchain & CI items
- Generated and locked `Cargo.lock`; bumped `rust-version` in `Cargo.toml` to `"1.95"` (required by `eframe 0.37`).
- Fixed `eframe 0.37` / `egui 0.37` compile errors (`ctx.all_styles_mut` in `src/theme.rs`, semicolon on `ui.painter().image(...)` in `src/ui/devices.rs`, dead code & redundant-closure clippy lints).
- Added `raw-window-handle` + `Win32_Graphics_Dwm`, `Win32_Security`, and `Win32_UI_HiDpi` features to `Cargo.toml`.
- Switched `set_autostart` in `src/config.rs` to `RegCreateKeyExW` + `RegSetValueExW` + `RegDeleteValueW` + `RegCloseKey`.
- Replaced placeholder assets with PiDA brand icon (`assets/icon.png` + multi-size `assets/icon.ico` generated from `PiDA`'s `pida-transparent.png`, dark `#111318` squircle). Added root `LICENSE` (MIT) and updated `README.md` metadata.

## Resolved runtime & windowing risks
- **Hidden-window wakeup (`eframe 0.37`)**: moved `sync_tray`, `drain_tray_actions`, `handle_close_request`, DWM borderless policy, first-frame `Visible(false)` workaround, Esc, and `FOCUS_LOSS_GRACE` (`seen_focused_while_shown` via `viewport().focused`) into `App::logic`, while `App::ui` returns immediately when `!visible`.
- **DWM border/shadow halo**: added `ensure_borderless_window_policy` (`DWMNCRP_DISABLED`) + `FLYOUT_OUTER_WIDTH` (`FLYOUT_WIDTH + 2.0`) and `CentralPanel::default().frame(Frame::NONE)` wrapper.
- **Mixed-DPI placement**: added `tray::monitor_dpi_scale_at` (`GetDpiForMonitor`) so target monitor work-area sizing uses target DPI and converts to current-window logical `OuterPosition`.
- **Pending slider flush on hide**: `hide()` flushes pending debounced numeric edits before closing the flyout.
- **Stale texture cache**: `DevicesView::textures` keys GPU textures by `Arc::as_ptr` fingerprint so `ClearCache` / icon updates reload cleanly.

## Resolved PiDA server contract & state-sync gaps
- **Read-only state actions**: `catalog.rs` now parses `params` + empty `inputs` actions into `ReadoutSpec` (`DeviceGroup.readouts`), polls them via `GET`, and renders them as status pills without ever `GET`-ing legacy no-param actions.
- **Numeric params without both bounds**: `RangeParam` uses optional `min`/`max`; bounded params render sliders, while single-bound or unbounded numeric inputs render a clamped `DragValue` + `−`/`+` stepper.
- **`required: true` inputs**: parsed on `ParamSpec`, tracked on `SetActionSpec.required_inputs`, and automatically included from current group state when dispatching partial-delta `POST`s.
- **Per-param error revert & active-edit sync lock**: `finish_dispatch_error` rolls back only the failed param via `revert_param`; `refresh_states` and `dispatch` merge via `merge_outputs_preserving_active` so background readbacks never overwrite in-flight or actively dragged controls.
- **Disambiguated nested titles**: `DeviceGroup.subtitle` displays parent group segments when nested (e.g. `Living Room` under `Lamp`).
- **Visual alignment**: adopted `#111318` / `#14171D` / `#181B22` dark slate palette, header bottom hairline, and ring+dot tray icon style matching `pigradient-tray`.
