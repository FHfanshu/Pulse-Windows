# Pulse for Windows

A Windows port of [Pulse](https://github.com/qunqin24/Pulse), the menu-bar monitor for AI coding
plan limits. A floating rail of rings at the screen edge shows how much of each provider's
allowance is used; pointing at a ring opens a card with every window and its reset time.

The UI and behaviour follow upstream closely; the deliberate differences (Windows title bar,
tray icon instead of menu bar, Acrylic instead of Liquid Glass, no bot-mark animation) are
listed in [docs/SPEC.md](docs/SPEC.md).

## Stack

- `crates/pulse-core` — Rust: providers, usage store, cache, adaptive refresh, DPAPI key store
- `src-tauri` — Tauri 2 shell: the non-activating panel window, tray, settings window
- `ui` — React + TypeScript: panel, card and settings UI
- `locales` — the five upstream translations, converted to JSON

## Develop

Requires Rust (MSVC toolchain), Node 20+, and the WebView2 runtime.

```bash
npm install
npm run tauri dev
cargo test -p pulse-core
```

`PULSE_MOCK=1` shows sample readings without any provider configured.
Pulse keeps its files in `%APPDATA%\Pulse`.

## License

Apache-2.0. Derived from Pulse by qunqin24; see [NOTICE](NOTICE) and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
