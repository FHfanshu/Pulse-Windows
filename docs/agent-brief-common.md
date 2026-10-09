# Rules for every delegated task

- You work in an isolated git worktree of `D:\code\Pulse` (Windows port of upstream Pulse; read `docs/SPEC.md` first).
  Use absolute paths inside **your worktree**; never `cd` into or edit `D:\code\Pulse` itself.
- Upstream (read-only, Swift/macOS):
  `C:\Users\25588\AppData\Local\Temp\claude\D--code-Pulse\eb69232a-803c-4b72-b5e5-f19486f57093\scratchpad\src-repo`
  Port behaviour and wording faithfully; where macOS has no Windows equivalent, pick the nearest Windows idiom and say so.
  Head each new file with `// Ported from upstream <path>` when it is a port.
- Rust: `export PATH="$HOME/.cargo/bin:$PATH"`. Core logic goes in `crates/pulse-core` (no Tauri) with unit tests;
  Tauri glue in `src-tauri/src/<feature>_ipc.rs` (register commands in `main.rs`'s `generate_handler!`).
  Never run `npx tauri dev` or launch `pulse.exe`; the lead runs the app. `cargo build -p pulse` is fine.
- UI: React + TS under `ui/src/`. Every visible string goes through `t()` with upstream's exact English key.
  **Do not edit `locales/*.json`.** Put keys missing from `locales/en.json` in `locales/pending/<your-branch>.json`
  with all five languages (en, zh-Hans, zh-Hant, ja, ko), taking upstream's translations from
  `Sources/Pulse/Resources/*.lproj/Localizable.strings` where they exist. Say "PC"/"this PC", never "Mac".
- Never read real credentials on this machine (`~/.claude/.credentials.json`, `%APPDATA%\Claude\config.json`, browser
  cookie stores, `%APPDATA%\Pulse\keys.dat`). Test with fixtures (`crates/pulse-core/tests/fixtures/`).
- Done means: `cargo test -p pulse-core` passes, `cargo build -p pulse` clean (no new warnings),
  `npm run typecheck` clean, `npx vite build` succeeds (run `npm ci` first if `node_modules` is missing).
- Commit on your branch: `git -c user.email=fhfanshu@gmail.com -c user.name=fhfanshu commit`, message ending with
  `Co-Authored-By: Claude <model> <noreply@anthropic.com>`. Don't push. Don't merge main unless told to.
- Final report under 25 lines: what is done, files touched outside your area, what is left TODO and why.
