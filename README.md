<!-- PROJECT: PiDA -->
<!-- TITLE: PiDA Tray -->
<!-- KEYWORDS: Windows, System Tray, Home Automation, Rust, REST API -->
<!-- LANGUAGES: Rust -->
<!-- TECHNOLOGY: egui, eframe, Tokio, Win32 -->

<!-- LOGO -->
![PiDA Tray](assets/icon.png)

[About](#about) | [Key Features](#key-features) | [Usage](#usage) | [Security](#security) | [Related](#related) | [License](#license)

## Status

<!-- STATUS -->
**Prototype**

## About
<!-- DESCRIPTION START -->
PiDA Tray is a lightweight Windows system tray companion for controlling a PiDA server over HTTP. It renders a taskbar-anchored flyout built dynamically from the server schema with cache-first state persistence and near-zero idle resource usage.
<!-- DESCRIPTION END -->

### Why
<!-- WHY START -->
Desktop access to server-managed home automation devices should not require keeping a browser tab open or running a heavy web runtime in the background. PiDA Tray compiles to a single native Windows executable that stays hidden in the tray until needed and skips UI rendering while closed.
<!-- WHY END -->

## Key Features

- Single native Windows `.exe` built with `eframe` (`glow` renderer) and `tray-icon` — no WebView, Electron, or external runtime.
- Early-returns UI rendering while hidden, with an optional slow `/api/health` check (`Off`, `30s`, `60s`, or `120s`) and Windows resume detection (`WM_POWERBROADCAST`).
- Cache-first startup: loads the last saved `/api/schema`, device states, and `/api/icon/<path>` PNGs from `%APPDATA%\PiDA-Tray\` before refreshing in the background.
- After every successful action, re-reads every group's state at 0.5 s, 1 s, 3 s, and 8 s (a newer action restarts the cascade), matching the Android client so fan-out actions and late-reflecting devices converge.
- Tray icon uses the PiDA mark: full colour while connected, grayscale when offline or unconfigured, and red after a failed action.
- Device cards use a server-provided `/api/icon/<endpoint>` PNG when available; otherwise a broad category fallback infers a generic icon from device terms (lighting, fan, display, computer, audio, sensor, camera, lock, cover, power, or a neutral generic device).
- Schema-driven controls preserving server YAML key order: boolean toggles, bounded sliders, numeric steppers, enum pills/dropdowns, string inputs, read-only state readouts, and legacy one-shot action buttons.
- Compact three-column grid of square device tiles. A single-toggle tile toggles from the whole card; a toggle+slider tile uses the whole card for tap-to-toggle and press/hold-or-drag to set the first slider, whose value fills the full cell from the bottom behind the disclosure strip. Every tile keeps its label and value on one shared bottom row.
- Expanding a tile animates in place to the full-width card with every control, status value, and action; clicking outside animates it closed. Expanded cards also expose ↑/↓ device-reorder controls.
- Reorder devices with the ↑/↓ controls in an expanded card. Order is saved by schema path, new devices append, and missing devices keep their slot and cached state so a transient schema omission does not lose their configuration.
- Subtle synthesized UI ticks for toggles, actions, and expansion; can be disabled with **Settings → Subtle UI sounds**.
- Opening Settings slides the page up over the device grid and eases the flyout between its two heights; closing slides it back down.
- Taskbar-anchored dark flyout with focus-loss auto-hide, pin toggle, and optional Windows startup registration (`HKCU\...\Run`).
- Mapped, pre-painted flyout with a one-tick reveal: no white flash or resize jump when opening, and no unpainted swap chain after hiding.

## Usage

### Requirements

- Windows 10/11 (x86_64).
- Rust 1.95+ (edition 2024) to build from source.
- A reachable [PiDA](https://github.com/LeHuman/PiDA) server.

### Building

Run the unit tests and build the release binary:

```sh
cargo test --all-features
cargo build --release
```

The compiled executable is written to `target/release/pida-tray.exe` (or download `pida-tray-windows-x86_64.exe` from GitHub Releases).

### Running & Configuration

1. Launch `pida-tray.exe`. It starts minimized to the Windows notification area.
2. Left-click the tray icon to toggle the flyout, or right-click for the context menu (`Open / Hide Flyout`, `Refresh`, `Settings...`, `Quit PiDA Tray`).
3. Open **Settings** and enter your server URL (for example `http://192.0.2.10:9999`), idle health check interval, and optional Windows startup toggle, then click **Save**.
4. Settings (`settings.json`), cached schema/state (`cache.json`), and cached icons (`icons/`) are stored under `%APPDATA%\PiDA-Tray\`.

## Security

Keep the PiDA server on a trusted private network or behind authentication. Do not expose an unauthenticated server to the public internet; for remote access, use a VPN or another authenticated tunnel.

## Related

- [PiDA](https://github.com/LeHuman/PiDA) — the server this tray app controls.
- [PiDA App](https://github.com/LeHuman/PiDA-App) — the Android companion app.
- [egui / eframe](https://github.com/emilk/egui) — immediate-mode Rust GUI framework.
- [tray-icon](https://github.com/tauri-apps/tray-icon) — cross-platform system tray library.

## License

This project is licensed under the [MIT License](LICENSE).
