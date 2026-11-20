# AGENTS.md

PiDA Tray: pure-Rust Windows tray app (eframe/glow) controlling a PiDA server over HTTP.

## Hard privacy rule
Never put personal IPs, hostnames, MACs, local paths, or real device/endpoint names in code,
defaults, comments, tests, docs, or git history. Use placeholders: `http://192.0.2.10:9999`,
`living_room/lamp`, `office/fan`.

## Invariants
- Schema key order is meaningful: use `Vec<(String, _)>` / `serde_json::Map` (`preserve_order`), never `HashMap`.
- `bool` params dispatch real JSON booleans.
- Only `GET` parameterized endpoints (`set` / `readouts`); never `GET` legacy no-param actions (as `GET` executes them).
- Hidden-window lifecycle (`tray.sync`, tray action draining, close cancel, focus-loss/Esc) runs in `App::logic`; `App::ui` returns immediately when `!visible`.
- The window stays mapped, parked at `PARKED_POS`, and DWM-cloaked while hidden. Reveal it with `reveal_flyout_at` (SetWindowPos + uncloak + foreground attach) so the pre-painted frame appears in one tick. Never hide it with `ViewportCommand::Visible(false)`: eframe 0.37 can then present an unpainted swap chain on the next show.
- `ensure_borderless_window_policy` installs the WndProc subclass once, then re-applies caption/sysmenu stripping and `DWMNCRP_DISABLED` every frame (DWM drops NC policy on move/resize and winit rewrites styles). `WS_THICKFRAME` stays for a normal window, but `WM_NCHITTEST` remaps every resize border to `HTCLIENT`: the flyout height is set by the card's own top-edge grip. Do not replace it with a one-shot DWM call.
- Tray mouse-down hides an open flyout and suppresses the matching mouse-up so it does not bounce back open.
- Every successful dispatch starts the Android-style settle cascade (`schedule_settle`): refresh all group states at +0.5/1/3/8 s, last action wins.
- Device cards are square tiles in a compact three-column grid. Tapping a tile toggles its primary switch (or runs a single action); press-hold or drag on a tile with a slider adjusts that slider, and the accent background fills the whole cell from the bottom. Labels and values share one bottom row. `has_more` adds the right disclosure strip. Expanded cards render full width, animate their reveal (`DevicesView::expanded_animated`), and animate closed on outside click.
- Device order is saved by path in `Settings::device_order`; new paths append, and missing paths/state are retained so devices return to their slot after transient schema omissions or re-additions.
- `PiDaTrayApp::view_mix` drives the settings transition: the settings sheet is laid out as a page of fixed size (`settings_card_h`) shifted by `slide = (1 - settle_p) * sheet_h` and masked by a fixed screen-space clip inside the fixed card shell (never `with_visual_transform`, which also moves the clip rect). The card height and the sheet slide follow the same `settle_p` (cubic_out on open, smoothstep on close), so the page is never squeezed.
- One native window "session canvas" per shown session: `canvas_target` is sized once, while the window is still cloaked, to the tallest card the content can need (settings card, or the device grid plus one expanded card). After the reveal, geometry is frozen until hide; the card animates inside it. Windows composites the previous surface across a native resize, so a visible resize always shows one stale frame. All resizes go through `ViewportCommand` while cloaked; never `SetWindowPos` (it leaves eframe's render surface at the old size).
- The window is clipped to the card with a region (`SetWindowRgn`) at rest, so the empty canvas above a short card is click-through. The cache key includes the client size (Windows does not resize a region) and the region is only applied while the window is shown. During a card animation or an edge drag the region is cleared so the card is never clipped.
- Per-view bookkeeping (`devices_card_h`, `settings_card_h`, the dragged-height bounds) and the card ease are keyed to the view that was actually laid out that frame (`laid_out_view`), because header actions flip `self.view` after the layout.
- A dragged height applies to the device grid only, only shrinks, and clamps to `[one tile row or the empty state, devices_card_h]`; the bounds follow the tile count. Settings is always shown whole.
- `FLYOUT_MIN_HEIGHT` is a 72 pt sanity floor. Requested heights go through `quantize_height` (round up to a physical pixel), and the devices `ScrollArea` sets `min_scrolled_height(0.0)` so a short flyout's card never overflows its window.
- UI sound effects go through `sound::play` only when `Settings::ui_sounds` is enabled.
- `FLYOUT_DEFAULT_HEIGHT` is the initial window height before the content has been measured.
- Demo mode (`main::demo_count`, `poller::seed_demo`, `poller::run_demo`) runs `--demo N` as an isolated instance: separate single-instance id, a throwaway temp `app_dir`, synthetic schema/state, and a command loop that only clears in-flight locks. No network or real config is touched.
- Use `ui::button::pill_button` for custom buttons; do not use explicit `.fill()` on `egui::Button` or hover feedback is lost.
- Device icons: a server PNG from `/api/icon/<endpoint>` always wins. The local fallback must stay generic and privacy-neutral — broad device-category terms only, never personal rooms/devices.
- Sync `InputState::focused` from `flyout_has_foreground` so egui draws the text caret and focus rings correctly.
- Tray icon/tooltip updates are diff-cached.
- Pure logic lives in `config`, `api`, `catalog`, `cache`, `poller` helpers, and `tray` placement so it is unit-testable.

## Checks (all must pass)
```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items
cargo build --release
```

## Commits
gitmoji + Conventional Commits: `✨ feat:`, `🐛 fix:`, `👷 ci:`, `📝 docs:`, `🔧 chore:`, `🎨 assets:`, `✅ test:`.
