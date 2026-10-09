# Handoff: redo PR #3 on current main

PR #3 (`codex/fix-local-session-stats`, head `e9a0135`, "Fix local session details and speed up
OpenCode statistics") was branched from `2d2bb78`. Main has since gained spend-source batch E,
which breaks it. Redo it as a fresh branch from `origin/main` (do not rebase the old branch in
place; it lives in another agent's checkout). Read `docs/agent-brief-common.md` first.

## Why it cannot merge as is

1. **Build break.** The PR replaces `ruzstd` with the `zstd` crate in `crates/pulse-core/Cargo.toml`
   and deletes `dsh::jsonl_bytes`. On main, `spend/sources/zed.rs` (`payload`, line ~120) calls
   `dsh::jsonl_bytes` (made `pub(super)` on main) and its module doc names `ruzstd`. The textual
   merge of `Cargo.toml` is clean, so the break only shows at compile time.
2. **Conflicts** in `spend/sources/dsh.rs` (main only changed `jsonl_bytes` visibility) and
   `spend/sources/tests.rs` (main writes the cache name via `agent_cache::VERSION` and swapped the
   "no card" probe from `Cursor` to `Zai`, since Cursor now has a card source).
3. **Cache version.** Both sides bumped `agent_cache::VERSION` 1 -> 2 independently, so the merge
   shows no conflict but caches written by main's v2 would be reused although the Harness and
   OpenCode readers now produce different output. Set it to **3**.

## What to carry over (all of it was reviewed and is good)

- `sources/mod.rs`: `SourceRead { records, has_read_limitations }` and the default
  `SpendSource::read_records`; `read_agent` uses it and sets `ledger.has_read_limitations`.
- `ledger.rs`: `has_read_limitations` field (serde default), propagated by `Ledger::adding`.
- `src-tauri/src/spend_ipc.rs`: `has_read_limitations` from the snapshot's ledgers instead of `false`.
- `record.rs`: per-UTC-quarter cache of `(slot key, day)` in `build_ledger`, plus the DST test in
  `tests_record.rs` (`repeated_quarters_keep_day_and_session_totals_across_midnight_and_dst`).
- `sources/opencode.rs`: `UsageMessage` visitor that keeps only usage/identity fields, the SQL
  `USAGE_DATA` projection with `json_valid`, borrowed `message_data`, and its two tests.
- `sources/dsh.rs`: the rewrite (typed `Event` parsing, `is_transcript`, bounded `decode` with
  `WindowLogMax(26)`, `read_records` reporting undecodable files, `card_provider = DeepSeek`,
  `has_captured_validation`, `DSH_HOME` only when absolute) and its five tests.
- `sources/zcode.rs`: session join only when both columns exist, `title`/`slug` into the record,
  verified path comment, and its test.
- `examples/spend-read-bench.rs`.

## What to change while redoing it

- **One zstd decoder.** Keep the `zstd` crate (Codex validated it against real Harness files) and
  drop `ruzstd`. Expose the bounded decoder from `dsh.rs` as `pub(super) fn decode_zstd(bytes:
  &[u8]) -> Option<Vec<u8>>` (or similar, keeping the 64 MiB ceilings) and switch `zed.rs` to it;
  update `zed.rs`'s module doc. Zed's tests must still pass, including the compressed-row one.
  Consider whether an undecodable Zed row should also set `has_read_limitations` (Zed would then
  override `read_records`); nice to have, not required.
- `agent_cache::VERSION = 3`; `sources/tests.rs` keeps main's `VERSION`-based file name and the
  `Zai` probe.
- Run `cargo fmt` on the new tests (several are written without spaces after commas).
- Check `zstd` builds in CI (`.github/workflows/ci.yml`, windows-2022 / MSVC); it compiles C.

## Done when

- `cargo test -p pulse-core` and `cargo clippy -p pulse-core --all-targets` pass from the
  workspace root (expect 1000+ tests; two spend-cache tests are known to flake under full
  parallelism, `unchanged_files_are_served_from_the_cache_not_reparsed` and
  `unchanged_rescans_reprice_without_rewriting_edits_and_deletions_persist`; rerun them alone).
- `cargo check -p pulse` (the Tauri app) passes.
- `ruzstd` is gone from `Cargo.toml` and `Cargo.lock`.
- A new PR against `main` that says it supersedes #3; close #3 afterwards.
