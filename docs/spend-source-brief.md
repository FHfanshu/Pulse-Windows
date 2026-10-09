# Adding a token spend source

A source is an agent that leaves a record of its work on this PC (OpenCode, Grok Build...). It is **one file** in
`crates/pulse-core/src/spend/sources/` and **one line** in the registry in `sources/mod.rs`. Nothing else changes: the
Token spend pane, the recap's "By Agent", the detailed card and the IPC all follow the registry.

Read first: `docs/agent-brief-common.md`. Upstream's list of all 54 sources, with their macOS paths, counters and caveats,
is `Docs/token-spend-sources.md` in the upstream repo; the Swift readers are under `Sources/Pulse/Usage/` and
`Sources/Pulse/Usage/Readers/`. The five ported so far (`opencode.rs`, `kilo.rs`, `grok.rs`, `kimi.rs`, `devin.rs`) are
the examples to copy: `grok.rs` for a folder of JSONL logs, `devin.rs` for one SQLite database.

## 1. The file

Name it for the product (`goose.rs`), head it with `// Ported from upstream Sources/Pulse/Usage/Readers/<File>.swift`, and
put upstream's doc comment on the store's format at the top, with what the Windows location is.

```rust
pub struct Goose;                       // a unit struct, nothing else

impl SpendSource for Goose {
    fn raw(&self) -> &'static str { "goose" }          // persisted id: upstream's Swift case name; never changes
    fn source_id(&self) -> &'static str { "goose" }    // upstream's canonical id
    fn display_name(&self) -> &'static str { "Goose" } // product name, untranslated
    fn icon(&self) -> Option<&'static str> { Some("goose") }   // stem in assets/icons; None draws no mark
    fn card_provider(&self) -> Option<Provider> { None }       // only a Provider that already exists
    fn price_vendor(&self) -> Option<&'static str> { None }    // models.dev plan vendor, if upstream names one
    fn env_vars(&self) -> &'static [&'static str] { &["GOOSE_HOME"] } // every variable `inputs` reads
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> { ... }        // where to look
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> { ... } // how to read it
}
```

Optional methods, with the answers upstream's `SpendAgent.swift` gives: `reports_cache_reads` (false for a store with no
cache column, e.g. Goose, Kiro), `requires_usage_export` (true for the Group E exports), `has_captured_validation`
(true only for the seven machine-verified ones; leave it false).

### `inputs`: where the records are

Return **every** file or folder the source is read from, whether or not it exists. A missing root is still named, so a
store that appears later is noticed. Use `Sources` (never `std::env` or `dirs` inside a source, tests inject it):

| Need | Use |
|---|---|
| `%USERPROFILE%` | `sources.home` |
| `%APPDATA%`, `%LOCALAPPDATA%` | `sources.app_data()`, `sources.local_app_data()` |
| `$XDG_DATA_HOME` or `~/.local/share` | `sources.xdg_data_home()` |
| any other variable | `sources.var("NAME")`, and list `"NAME"` in `env_vars()` |

Do not list a huge folder when two small ones will do (OpenCode names `storage/message`, not `storage`): the cache walks
every root on every read.

### `records`: what was done

Return one `AgentUsageRecord` per request (`spend/record.rs`): `AgentUsageRecord::new(timestamp, model, tally)`, then
set `session_id` (via `.session(..)`), `session_name`, `title` and `project` (a working directory; Windows paths are
fine). Rules, all from upstream:

- **Tokens are incremental.** If the store keeps a running total, difference it here.
- **Four kinds:** `TokenTally { input, cache_write, cache_read, output }`. `input` is *fresh* input: if the store's input
  figure includes the cache read (Codex does), subtract it. Reasoning goes into `output` unless it is already inside it.
- **A bare total** you cannot split goes in `unclassified_tokens`, never into `input`.
- **Never guess a count.** A line, row or file the reader cannot decode contributes nothing. A missing store is no records.
- **Dedup** by the product's own message id, inside the reader (a `HashSet` of ids), when two files or databases can
  hold one message. Two identical requests with no id are two requests.
- **Time:** an instant per record. Seconds vs milliseconds: `spend::logio::timestamp(value, milliseconds)`. A record with
  no real time is skipped, not dated at the epoch. A source that only knows a session's time sets `.aggregate(true)`.
- **No model name** in the store: use one placeholder id no price list matches (see `kimi.rs`); it is counted, not costed.
- Shared helpers: `spend::logio` (JSONL lines, `files`, JSON, counts), `spend::sqlite` (read-only open, `each`, `text`),
  `sources::{int, list, push_unique}`, `spend::titles::title_from`.

Open every SQLite file **read-only** (`sqlite::open`); the agent may be running. A wrong schema reads as no rows.

## 2. The registration line

In `sources/mod.rs`, add one line to `registry!`: the `SpendAgent` variant it becomes, the module (the file) and the
struct.

```rust
    Goose => goose::Goose,
```

That generates `pub mod goose;`, the `SpendAgent::Goose` variant, `SpendAgent::ALL` (the pane's order) and the lookup.
The cache (`agent-<n>-<raw>.json`), pricing, sessions, projects, the card and the recap come with it. If your change
makes an unchanged store read differently, bump `agent_cache::VERSION`.

If the product is also a `Provider` Pulse draws (OpenCode Go, Kimi Code, Grok), return it from `card_provider` and its
detailed card shows this source's spend; nothing else is needed (`spend_card_providers` tells the UI).

## 3. The icon

`icon()` is a file stem in `assets/icons/` (the set `ui/src/panel/Icon.tsx` loads). Reuse a mark that exists. If the
product has none, copy upstream's from `Sources/Pulse/Resources/<name>.svg` into `assets/icons/<name>.svg` (single colour,
`currentColor` is applied for you). If upstream has none either, return `None`: a borrowed mark names the wrong company.

## 4. Windows paths

- Start from upstream's macOS path and ask where the tool really writes on Windows: look for an environment override
  (`XDG_DATA_HOME`, `<TOOL>_HOME`) in its docs or source, and for whether it uses `%USERPROFILE%\.tool` (most
  Node and Python CLIs do, including OpenCode, which is `%USERPROFILE%\.local\share\opencode`) or `%APPDATA%` /
  `%LOCALAPPDATA%` (most Rust and Electron apps).
- If you cannot check it on a real PC, list every plausible candidate in `inputs` and mark each unverified:
  `// WINDOWS-PATH: unverified`. A candidate that does not exist costs one `stat` and shows nothing.
- Treat paths inside the data as text from another OS: a working directory may be `C:\Users\me\code` or `/Users/me/code`
  (`UsageProject::new` handles both), a percent-encoded folder name decodes to either.
- Never read credentials (`auth.json`, tokens, cookie stores); records only.

## 5. Tests

Next to the code in the same file (`#[cfg(test)] mod tests`), with synthetic data built in a `tempfile::tempdir()` used as
the home folder. Never read this PC's real stores in a test. The pattern, from `grok.rs`:

```rust
let home = tempfile::tempdir().unwrap();
// write the store's files under home in the shape the real one has (JSONL lines with serde_json::json!,
// or a SQLite file with spend::sqlite::tests::database(path, &["CREATE TABLE ...", "INSERT ..."]))
let cache = tempfile::tempdir().unwrap();
let ledger = read_agent(SpendAgent::Goose, &Sources::new(home.path()), cache.path(), &Calendar::utc(2), &prices);
assert_eq!(ledger.days[0].tally, TokenTally::new(input, cache_write, cache_read, output));
```

Cover at least: the four kinds land where upstream says; a row that is not a reply, has no time or has no counts is
skipped; a message present twice counts once; a store that is missing is an empty ledger; the project and title come
through; an environment override moves the store (`Sources::new(home).with_var("NAME", path)`); and, for money, a
session's buckets reaching "today" (copy `a_sessions_buckets_reach_today_and_only_todays_counts`). Port upstream's own
test for the reader from `Tests/PulseTests/` where one exists.

`cargo test -p pulse-core real_stores -- --ignored --nocapture` reads this PC's real stores, read-only, into a throwaway
cache and prints totals per source; use it once to see the reader work on real data.

## 6. Done

`cargo test -p pulse-core`, `cargo build -p pulse` (no new warnings), `npm run typecheck`, `npx vite build`. Then delete
the source's line from the TODO list in `spend/agent.rs`.
