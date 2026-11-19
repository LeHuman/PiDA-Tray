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
- **Collapsible cards + aligned controls**: device cards are collapsed by default, show their first interactive control while collapsed (slider/stepper/enum/string or first action), have a chevron disclosure, section labels, two-column action tiles, and fixed label/value columns so every slider track is the same length. Slider rails use a dedicated `PALETTE.track` colour instead of blending into the card.
- **Dynamic fit**: the card height is derived from the scroll content's natural size (`content_size - inner_rect`), so collapsing cards shrinks the flyout and expanding grows it again. The last card has no trailing spacer, so the bottom inset matches the side padding.
- **Hover/pressed feedback**: shared `ui::button::pill_button` applies explicit resting/hover/pressed fills, border changes, focus rings, and a pointing cursor to settings actions, enum segments, and legacy action tiles; switches, chevrons, combos, and header icons do the same.
- **Settings footer**: Save/Cancel are pushed to the bottom of the card instead of floating under the form.
- **Fixed panel height**: the flyout no longer derives its window height from the `ScrollArea` measurement (inside a live window that returns the window height, letting it shrink once and never grow). It uses a stable 720 pt default, a 260–900 pt range, and the user's dragged height when present.
- **Resizable, monitor-aware flyout**: the window is resizable vertically (width locked), caps at the monitor work area, remembers a user drag in `settings.json`, and offers **Reset flyout size** in Settings.
- **Mapped-window reveal fix**: hiding with `ViewportCommand::Visible(false)` left eframe 0.37 with a blank swap chain on the next show. The window now stays mapped, is parked at `-32000,-32000`, and is DWM-cloaked while hidden (`main::set_window_cloaked`); show uncloaks and repositions it.

## Ported PiGradient tray UI fixes
- **One-tick reveal**: `RevealPhase::{RevealNow, Settling, ReadyToUncloak, Shown}` + `painted_once` + `reveal_flyout_at` (`SetWindowPos` / uncloak / foreground attach) replace the old position-then-show sequence, removing resize jumps and white flashes.
- **Window policy**: HWND subclass now returns `0` for `WM_NCCALCSIZE`, suppresses `WM_NCACTIVATE` / `WM_NCPAINT` / `WM_ERASEBKGND`, strips `WS_CAPTION|WS_BORDER|WS_SYSMENU|WS_MIN/MAXIMIZEBOX` plus `WS_EX_APPWINDOW`, and re-applies `DWMNCRP_DISABLED` every frame. `WS_THICKFRAME` is kept so the flyout remains height-resizable.
- **Focus truth**: `InputState::focused` is synced from `GetForegroundWindow`; focus-loss hiding only fires after the flyout genuinely held focus, and pulses at 10 Hz while shown so click-away is noticed promptly.
- **Tray bounce guard**: `TrayAction::PointerDown` hides an open flyout and suppresses the matching mouse-up; tray events also carry the icon rect for placement.
- **Card rendering**: 1px transparent `CARD_BLEED`, window width includes stroke + bleed, and the card border is repainted after children so header/footer fills cannot cover it.
- **Controls**: icon buttons have hover/pressed feedback and a pointing cursor; the server URL field keeps a 1px focus stroke so focusing it cannot grow the card; constant label/value columns keep slider tracks aligned.
