# Brief: port a profiled provider to Rust

You are porting usage readers from upstream Pulse (macOS, Swift, Apache-2.0) to this Windows port's
Rust core. Upstream checkout (read-only reference):
`C:\Users\25588\AppData\Local\Temp\claude\D--code-Pulse\eb69232a-803c-4b72-b5e5-f19486f57093\scratchpad\src-repo`

## Read first
1. `crates/pulse-core/src/providers/moonshot.rs` — **the template. Copy its structure exactly.**
2. `crates/pulse-core/src/providers/profile.rs` — helpers you must use (`get_bearer`, `send`, `decode`, `date`, `epoch`, `reading`, `balance_reading`, `keep_cookies`, `test_support::{context, fixture}`).
3. `crates/pulse-core/src/model.rs` — `UsageWindow`, `WindowKind`, `ProviderUsage`, `Unavailability`.
4. `crates/pulse-core/src/service.rs` — `FetchContext` (`api_key`, `home`, `app_data`, `local_app_data`, `http`, `now`).
5. For your provider X: upstream `Sources/Pulse/Providers/Profiled/<X>UsageService.swift`,
   `Docs/providers/<x>.md`, and `Tests/PulseTests/<X>Tests.swift`.

## Rules
- Write only `crates/pulse-core/src/providers/<module>.rs` (the file exists as a placeholder; replace it).
  Do **not** edit `providers/mod.rs`, `model.rs`, `profile.rs` or any other file. If you believe a shared
  file needs a change, say so in your final report instead.
- Struct name: the provider in UpperCamelCase (e.g. `OpenAiPlatform`), `#[derive(Default)]`, implementing
  `UsageService` with `fn provider()` returning the matching `Provider::` variant.
- Same endpoints, headers, JSON fields, window ids, `WindowKind`s, scopes, `is_exhausted` rules and
  unavailability cases as upstream. Never invent a percentage: if upstream returns no window, you return none.
- Split network from parsing: a pure `fn reading(body: &[u8], ctx, account) -> Result<ProviderUsage, Fail>`
  (or several) that tests can call without network.
- Credentials: an API key comes from `ctx.api_key(account)`. A pasted cookie header/session also comes from
  `ctx.api_key(account)` (filter it with `keep_cookies` as upstream does). A **local login file** that upstream
  reads from `~/...` is read from `ctx.home.join(...)` on Windows (same relative path under `%USERPROFILE%`);
  if upstream reads `~/Library/Application Support/<App>/...`, use `ctx.app_data.join("<App>")...` and leave a
  `// WINDOWS-PATH: unverified` comment. Never read macOS Keychain; if upstream needs it, return
  `Unavailability::LocalLoginMissing` for that route and note it in your report.
- Tests: port every upstream test case that exercises parsing, using fixtures with
  `test_support::fixture("<file>.json")` (fixtures are already copied to `crates/pulse-core/tests/fixtures/`).
- Comments: short, in your own words. Header line: `// Ported from upstream Providers/Profiled/<X>UsageService.swift.`

## Done means
`cargo test -p pulse-core providers::<module>` passes with **zero warnings** in your file
(run with `$HOME/.cargo/bin` on PATH; on this machine use the Bash tool:
`export PATH="$HOME/.cargo/bin:$PATH"; cd /d/code/Pulse && cargo test -p pulse-core providers::<module>`).
Then commit only your file: `git add crates/pulse-core/src/providers/<module>.rs && git commit -m "Port <X> provider"`.

## Final report (keep it under 15 lines)
Module, tests ported (count), anything you could not port and why, any WINDOWS-PATH guesses.
