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
- **Generic device icons**: server `/api/icon/<endpoint>` PNGs take precedence. The fallback is token-based and covers broad device classes (`light`, `fan`, `display`, `computer`, `audio`, `sensor`, `camera`, `lock`, `cover`, `power`) with a neutral `generic` chip for everything else. No installation-specific room/device names are part of the mapping.
- **Square widget tiles**: a compact three-column grid of square tiles. Tapping a tile toggles its primary switch or runs a single action; press-hold/drag on a slider tile adjusts the first slider, and the accent background fills the whole cell from the bottom (clipped to the card's own rounded shape). Extra options use the right-side disclosure strip. Expanded cards render exactly one full-width detail card in place, animate their height/opacity open and closed, and collapse on an outside click.
- **Collapsible cards + aligned controls**: expanded cards keep section labels, two-column action tiles, and fixed label/value columns so every slider track is the same length. Slider rails use a dedicated `PALETTE.track` colour instead of blending into the card.
- **Dynamic fit**: the card height is derived from the scroll content's natural size (`content_size - inner_rect`), so collapsing cards shrinks the flyout and expanding grows it again. The last card has no trailing spacer, so the bottom inset matches the side padding.
- **Hover/pressed feedback**: shared `ui::button::pill_button` applies explicit resting/hover/pressed fills, border changes, focus rings, and a pointing cursor to settings actions, enum segments, and legacy action tiles; switches, chevrons, combos, and header icons do the same.
- **Status checks**: every successful dispatch (set/slider/legacy) starts a settle cascade that re-reads all group states at +0.5 s, 1 s, 3 s, and 8 s. A newer action bumps a generation counter and restarts the cascade, so the last action wins. This mirrors `PiStateSync` in the Android client and catches fan-out/late-reflecting devices.
- **Reorderable + resilient devices**: `Settings::device_order` stores paths, not indices. New devices append in schema order; paths absent from a schema keep their slot, and cached group state is no longer pruned on successful schema loads, so a dropped/re-added device returns with its state and ordering intact.
- **Subtle sounds**: synthesized 16-bit mono WAV ticks play through Win32 `PlaySoundW` (`SND_MEMORY|SND_ASYNC`) for toggle/action/open/close and respect `Settings::ui_sounds`.
- **PiGradient audit (post-2bb23eb)**: reviewed `f0ee26e` (vertical-taskbar anchoring — already covered by our side-taskbar placement), `033872b` (settings header already mirrored here), `a2b9e9f` (atomic settings already present; adopted `shutdown_background`), and `3821208` (SSE backoff — not applicable, no SSE).
- **Settings footer**: Save/Cancel are pushed to the bottom of the card instead of floating under the form.
- **Settings transition**: `App::view_mix` eases between the devices grid and settings; the settings sheet slides up from the bottom with an opaque backdrop and the flyout height lerps from the captured start height, so neither the content swap nor the size change snaps.
- **Fixed panel height**: the flyout no longer derives its window height from the `ScrollArea` measurement (inside a live window that returns the window height, letting it shrink once and never grow). It uses a stable 720 pt default, a 260–900 pt range, and the user's dragged height when present.
- **Resizable, monitor-aware flyout**: the window is resizable vertically (width locked), caps at the monitor work area, remembers a user drag in `settings.json`, and offers **Reset flyout size** in Settings. Auto-fit no longer has a 260 pt floor: `FLYOUT_MIN_HEIGHT` is a 72 pt sanity floor, so a single row or the empty state fits exactly.
- **Short-flyout edge guards**: the devices `ScrollArea` sets `min_scrolled_height(0.0)` (egui's 64 pt default made the card taller than the window and clipped its bottom), and `quantize_height` rounds requested inner heights up to whole physical pixels so DPI truncation can't cut the bottom edge. The settings footer reserves 50 pt, and the fully open sheet skips its scene clip so the footer fill reaches the card border without a surface seam.
- **UI demo mode**: `pida-tray.exe --demo N` (or `PIDA_TRAY_DEMO=N`) seeds N synthetic devices in memory, uses a separate instance id and temp app dir, and runs a no-network command loop for testing tile counts and transitions.
- **Mapped-window reveal fix**: hiding with `ViewportCommand::Visible(false)` left eframe 0.37 with a blank swap chain on the next show. The window now stays mapped, is parked at `-32000,-32000`, and is DWM-cloaked while hidden (`main::set_window_cloaked`); show uncloaks and repositions it.

## Ported PiGradient tray UI fixes
- **One-tick reveal**: `RevealPhase::{RevealNow, Settling, ReadyToUncloak, Shown}` + `painted_once` + `reveal_flyout_at` (`SetWindowPos` / uncloak / foreground attach) replace the old position-then-show sequence, removing resize jumps and white flashes.
- **Window policy**: HWND subclass now returns `0` for `WM_NCCALCSIZE`, suppresses `WM_NCACTIVATE` / `WM_NCPAINT` / `WM_ERASEBKGND`, strips `WS_CAPTION|WS_BORDER|WS_SYSMENU|WS_MIN/MAXIMIZEBOX` plus `WS_EX_APPWINDOW`, and re-applies `DWMNCRP_DISABLED` every frame. `WS_THICKFRAME` is kept so the flyout remains height-resizable.
- **Focus truth**: `InputState::focused` is synced from `GetForegroundWindow`; focus-loss hiding only fires after the flyout genuinely held focus, and pulses at 10 Hz while shown so click-away is noticed promptly.
- **Tray bounce guard**: `TrayAction::PointerDown` hides an open flyout and suppresses the matching mouse-up; tray events also carry the icon rect for placement.
- **Card rendering**: 1px transparent `CARD_BLEED`, window width includes stroke + bleed, and the card border is repainted after children so header/footer fills cannot cover it.
- **Controls**: icon buttons have hover/pressed feedback and a pointing cursor; the server URL field keeps a 1px focus stroke so focusing it cannot grow the card; constant label/value columns keep slider tracks aligned.
