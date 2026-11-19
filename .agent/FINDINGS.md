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

## Follow-up UI pass
- **Branded tray icon states**: `assets/icon.png` is compiled into the executable and resized to 32×32; online keeps the full PiDA colours, offline/unconfigured/unknown is grayscale, and a failed action applies a red tint.
- **Collapsible cards + aligned controls**: device cards have a chevron disclosure, section labels (`Controls` / `Status` / `Actions`), two-column action tiles, and fixed label/value columns so every slider track is the same length. Slider rails use a dedicated `PALETTE.track` colour instead of blending into the card.
- **Resizable, monitor-aware flyout**: the window is resizable vertically (width locked), starts at 720 pt / caps at 900 pt or the monitor work area, remembers a user drag in `settings.json`, and offers **Reset flyout size** in Settings.
- **Mapped-window reveal fix**: hiding with `ViewportCommand::Visible(false)` left eframe 0.37 with a blank swap chain on the next show. The window now stays mapped, is parked at `-32000,-32000`, and is DWM-cloaked while hidden (`main::set_window_cloaked`); show uncloaks and repositions it.
