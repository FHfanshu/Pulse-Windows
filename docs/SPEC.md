# Pulse for Windows — Port Specification

Windows port of [qunqin24/Pulse](https://github.com/qunqin24/Pulse) (macOS, Swift, Apache-2.0, upstream v1.8.3).
Goal: same product, same UI/UX, on Windows 10/11. Upstream is the reference for every behaviour and
every number below; when this document and upstream disagree, upstream wins unless this document
lists the difference under **Deliberate differences**.

## 1. Stack

| Layer | Tech | Location |
|---|---|---|
| Core (providers, store, cache, refresh, alerts, `--json`) | Rust 2021, `tokio`, `reqwest` (rustls), `serde` | `crates/pulse-core` |
| Shell (windows, tray, shortcuts, deep links, autostart, updater) | Tauri 2 | `src-tauri` |
| UI (panel, card, settings, recap) | React 18 + TypeScript + Vite, plain CSS + `motion` | `ui/` |
| Locales | flat JSON converted from upstream `.strings` | `locales/*.json` |
| Provider icons | upstream SVGs (LobeHub icons) | `assets/icons/` |

`pulse-core` has **no Tauri dependency**: `pulse.exe --json` runs the core headless.

## 2. Deliberate differences from upstream

| Upstream (macOS) | Windows | Why |
|---|---|---|
| Bot mark animation (`Panel/BotMark`) | removed; the plain provider icon is always drawn | owner decision |
| Notch berth / notch alert | removed | no notch on Windows |
| Liquid Glass | Win11 Acrylic (`window-vibrancy`) + the same `PanelGlass.dim` black scrim | no lensing API on Windows |
| SF Pro Rounded | `"SF Pro Rounded"` if installed, else **Nunito** (bundled, OFL) then Segoe UI Variable | SF fonts may not ship on non-Apple platforms |
| SF Symbols in settings | Lucide icons (MIT) chosen to match | same licensing reason |
| Traffic-light title bar | native Windows caption buttons; window content identical | owner decision |
| `NSStatusItem` menu bar | system tray icon + menu; `MenuDashboard` becomes a tray popup | platform |
| Keychain / `keys.dat` key derived from the Mac | DPAPI (`CryptProtectData`, user scope) | platform |
| Browser cookie import (Chrome/Edge app-bound encryption) | Firefox cookie DB read directly; Chromium sites signed in through an embedded WebView2 window | Chrome ≥127 cookies cannot be decrypted by other processes |
| Sparkle | Tauri updater (GitHub Releases) | platform |
| Raycast integration | not ported | owner decision |
| Providers | first 20 (§6), others later | owner decision |

## 3. Layout tokens (all multiplied by `scale`)

`scale` = Small 0.82 / Standard 1 / Large 1.22. `spacing` = Tight 0.6 / Standard 1 / Loose 1.4.
Source of truth: `ui/src/panel/layout.ts`, a line-for-line port of `DockLayout`, `DetailCardLayout`,
`FloatingPanelController.Layout`. Keep names identical to upstream so diffs stay readable.

Rail: width 64, ring Ø36, ring line 4, ring→text 6 (11 with window clock), percent font 13 medium rounded,
percent text 16h × 38w, item spacing 30×spacing, vertical padding 46, horizontal padding 10, corner 26,
flare 24h × 38w (round ends: corner = width/2), second ring Ø26 line 2.5, collapsed sliver 6 × 96 (hit 20).
Card: width 250, padding 18, corner (ringØ + ringLine)/2, pointer 20w × 40h, gap 8, content spacing 14,
row spacing 7, bar 6, header 19, title 14 semibold, row 11.5, message 12, footnote 11, header icon 16.
Colours: good `rgb(0,230,140)`, caution `rgb(255,194,38)`, warning `rgb(255,79,66)`, spent `rgb(217,23,33)`.
Thresholds: green < 0.5, amber < warningThreshold (default 0.75), red above, spent from the provider's flag or ≥ 1.
Track opacity 0.18 (ring), 0.17 (bar). Selected ring scale 1.06, halo radius 10 at 0.42 opacity.

Motion: arc spring (response 0.5, damping 0.85); card spring 0.28 s; row insert opacity + 6pt drop
0.14 s after 0.06 s, removal 0.06 s; header in 0.1 s out 0.06 s; rings fade in 0.18 s after 0.12 s on expand,
out 0.10 s on collapse; refresh mark sweep 0.16 / 0.85 s; busy mark sweep 0.22 / 1.0 s.

## 4. Panel window (the hard part)

- One transparent, borderless, always-on-top, **non-activating** window
  (`WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED`), skip taskbar, sized by `Layout.size(edge)`,
  rounded **up** to whole pixels (DIP). The window never resizes while a card opens; the card is an
  overlay on the rail. Axis change (side ↔ top) may resize.
- Click-through: the window is `WS_EX_TRANSPARENT` except while the pointer is over the rail's hit
  rect or the open card. A Rust pointer sampler (60 Hz while near, 4 Hz idle) toggles it and drives
  hover enter/leave — mirroring upstream's "enter from tracking, leave from sampling".
- Drag: press on the rail starts a native move loop driven from Rust; on release `PanelPlacement`
  snaps to the nearest edge within the snap distance or stays free. Click (no drag) on a ring refreshes it.
  Right-click opens the panel menu.
- Hide-until-pointed (auto-collapse), follow-active-display, hide-in-full-screen (foreground window
  covers its monitor) behave as upstream.
- Multi-monitor and per-monitor DPI: every geometry value is in DIPs; convert at the Win32 boundary only.

## 5. Core model (Rust ↔ TS, serde camelCase)

Ported 1:1 from `Usage/ProviderUsage.swift`: `UsageWindow { id, kind, scope, usedFraction, windowSeconds,
resetsAt, reportsLength, estimate, isExhausted, nextExpiry, label }`, `ProviderUsage { account, windows,
observedAt, state, plan, creditBalance, creditRemaining, origin, isCached }`, `Unavailability` (string enum,
same case names; messages are localized in the UI from the same English keys).
Rules ported verbatim: `percentValue` (never 0 when used, never 100 when not all), `headlineWindow`,
`secondWindow`, `current(at)`, `hasTurnedOver`, `elapsedFraction`.

Provider trait:

```rust
#[async_trait]
pub trait UsageService: Send + Sync {
    fn provider(&self) -> Provider;
    async fn fetch(&self, ctx: &FetchContext, account: &AccountKey) -> ProviderUsage;
}
```

`FetchContext` gives HTTP client (proxy-aware), secrets store, settings snapshot, clock. Profiled
providers implement `ProviderProfile` (data + one parse fn) instead; see `crates/pulse-core/src/providers/profiled/`.

IPC: Tauri commands `get_snapshot`, `refresh(account?)`, `get_settings`, `set_setting(key, value)`;
events `usage-changed`, `settings-changed`, `pointer` (hover/drag state from the sampler).

## 6. First providers and who ports them

| Tier | Providers |
|---|---|
| Opus (hand-written, OAuth / logs / helper processes) | Claude Code, Codex, Cursor, GitHub Copilot, Antigravity, Kiro |
| Sonnet (hand-written, key or console session) | DeepSeek, Kimi Code, Z.ai, MiniMax, OpenCode Go, Grok |
| Haiku (profiled) | Gemini, Windsurf, OpenAI API, Amp, Augment, Factory, Warp, Moonshot |

Each provider port must: read upstream `Sources/Pulse/Providers/...` and `Docs/providers/<name>.md`,
map macOS paths to Windows (`%USERPROFILE%`, `%APPDATA%`, `%LOCALAPPDATA%`), and pass the upstream
fixtures copied to `crates/pulse-core/tests/fixtures/<provider>/`.

## 7. Delegation contract

Every sub-agent task states: files it may touch, the upstream files to port, the template to copy,
and the command that must pass (`cargo test -p pulse-core <filter>` or `npm run typecheck`).
A task is accepted only when that command passes; Opus spot-checks rather than re-reading.

## 8. Phases

1. Skeleton: core model + mock data, panel window, rail, rings, card, settings shell. ← current
2. First 5 providers end to end (Claude Code, Codex, Cursor, Copilot, Gemini).
3. Remaining 15 providers.
4. Token spend, recap, notifications, tray reading, window starter.
5. Packaging, updater, localization review.
