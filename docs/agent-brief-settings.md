# Brief: settings window UI

Port upstream Pulse's settings window (SwiftUI, macOS) to React in `ui/src/settings/`. The look must match
upstream (see `docs/settings.webp`, `docs/account-codex.webp`, `docs/account-claude-code.webp`) except that the
window uses the **native Windows title bar** (no traffic lights, no custom title bar).

Upstream checkout (read-only):
`C:\Users\25588\AppData\Local\Temp\claude\D--code-Pulse\eb69232a-803c-4b72-b5e5-f19486f57093\scratchpad\src-repo`
Read: `Docs/ui/settings.md`, `Sources/Pulse/Settings/{SettingsView,SettingsSidebar,SettingsPane,SettingsRow,AppearancePane,RingsPane,PlacementPane,GeneralPane,NetworkPane,AccountPane,AccountPanelGroup,AccountConnectionGroup,AccountLiveUsageGroup}.swift`.

## What exists (use it, do not change it)
- `ui/src/shared/settings.ts`: `AppSettings` type, `useSettings()`, `updateSettings(patch)`, `useUsage()`, `ProviderInfo`.
- `ui/src/shared/i18n.ts`: `t(key, ...args)` — keys are upstream's English strings (see `locales/en.json`); `%@` placeholders.
  **Every visible string goes through `t()` with upstream's exact key.** If a key is missing from `locales/en.json`, use the
  English text as the key anyway and list it in your report. Replace "Mac" with "PC" only by adding a new key and listing it.
- `ui/src/shared/copy.ts`: `windowName`, `resetText`, `unavailableMessage`.
- `ui/src/panel/Icon.tsx`: `ProviderIcon` (provider marks), `providerNames`.
- Tauri commands (`invoke` from `@tauri-apps/api/core`): `get_settings`, `update_settings {patch}`, `list_providers`
  → `ProviderInfo[]`, `has_secret {id}` → bool, `set_secret {id, value|null}`, `refresh {account}`, `get_snapshot`.
  Account id = provider id for the primary account (e.g. `"codex"`).

## Build
- Shell: left sidebar (search field, sections), right scrollable content with pane heading. Sidebar sections per
  `Docs/ui/settings.md` "Panes" table: **Panel** (Appearance, Rings and figures, Position and behavior, Token spend),
  **Application** (General, Notifications, Network and refresh), **Enabled** (enabled accounts in order),
  **Subscriptions** / **API and pay-as-you-go** (the rest; for now treat Moonshot, OpenAI API and DeepSeek as API,
  everything else as subscription), then About. Search filters panes by title and row titles, case-insensitive.
- Controls drawn to look like macOS 26 (from the screenshots): grouped rounded card per `SettingsGroup`, rows with
  title + one-line grey subtitle on the left and the control on the right, hairline separators; blue capsule switch;
  segmented control with blue selected segment; popup menus as styled `<select>`; text fields rounded.
  Light **and** dark theme via `prefers-color-scheme`. Font stack: `var(--font-rounded)` is for the panel only — the
  settings window uses `"Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif`.
- Panes, wired to real settings through `updateSettings`:
  - Appearance: Size (Small/Standard/Large), Spacing (Tight/Standard/Loose), Round ends, Liquid Glass (+ Transparency
    slider when on), Ring activity animation.
  - Rings and figures: every row listed in `Docs/ui/settings.md` for that pane that has a field in `AppSettings`.
  - Position and behavior: Show floating panel, Hide in full screen, Hide until pointed at, Follow the active display.
    (Position picker and Order list: render them disabled with a `// TODO(opus)` — placement is owned elsewhere.)
  - General: Open at login (`launchAtLogin`), Hide tray icon (`hidesTrayIcon`, label "Hide menu bar icon" key is fine),
    Language (System + five languages; changing it calls `setLanguage` and re-renders).
  - Network and refresh: Check every (Automatic / fixed minutes 2,5,10,15,30) and proxy (enabled, scheme, host, port).
  - Account pane (any provider): heading with icon + name; **Show in panel** switch (adds/removes the id in
    `enabledAccounts`); for API-key providers a key field (password input, Save/Remove, shows "saved" state via
    `has_secret`); **Current usage** card showing each window's name, percent and reset text from `useUsage()`
    (or the unavailable message), with a Refresh button.
  - Notifications, Token spend, About: heading plus a short placeholder row each (they are ported later).
- Files: only under `ui/src/settings/` (plus `ui/settings.html` if needed). Do not edit `ui/src/panel/`,
  `ui/src/shared/` or any Rust. If you need a shared change, describe it in the report.

## Done means
`npm run typecheck` passes and `npx vite build` succeeds. Commit your files with a clear message.
Final report (under 20 lines): panes done, missing locale keys added, anything left as TODO.
