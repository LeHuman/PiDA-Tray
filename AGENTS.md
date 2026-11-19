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
- `ensure_borderless_window_policy` subclasses the HWND every frame, strips caption/sysmenu styles, keeps `WS_THICKFRAME` for vertical resizing, and re-applies `DWMNCRP_DISABLED`. Do not replace it with a one-shot DWM call.
- Tray mouse-down hides an open flyout and suppresses the matching mouse-up so it does not bounce back open.
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
