// Ported from upstream Sources/Pulse/Usage/Readers/DSHUsageReader.swift and DSHZstdDecoder.swift.
//! DeepSeek Harness (`dsh`) transcripts.
//!
//! `~/.dsh/sessions/<encoded-cwd>/<session-id>/session.jsonl[.zstd]` holds one event per line:
//! `session`, `request/header`, `user/message`, `assistant/message` and `compaction/summary`. Assistant
//! replies and compaction summaries are both real provider calls and are counted additively.
//!
//! **The suffix is physical; the magic is the truth.** A transcript is decoded as zstd only when its
//! first four bytes are the zstd frame magic, never because it ends in `.zstd`; a plain file is already
//! JSONL. A zstd stream may hold several frames. A torn trailing frame (the file ends mid-frame, as a
//! live writer leaves it) keeps the decodable prefix; a frame that reports corrupt data, or a stream
//! over the 64 MiB raw or decoded ceiling, makes the whole file contribute nothing. The decoder is the
//! pure-Rust `ruzstd` crate, so no system library is needed.
//!
//! **Reasoning is a subset of output, so it is not added.** `reasoningTokens` is contained in
//! `outputTokens`, and the reported output is kept whole.
//!
//! **A forked prefix is not this session's work.** Events whose `seq` is below the session header's
//! `seedLength` were inherited verbatim and are skipped. The identity is deliberately not
//! session-scoped, so a call copied into a fork collapses with its original.
//!
//! Where it lives on Windows: `DSH_HOME` when set, else `%USERPROFILE%\.dsh\sessions`.
//! WINDOWS-PATH: unverified.

use std::io::Read;
use std::path::PathBuf;

use chrono::DateTime;
use ruzstd::decoding::{FrameDecoder, StreamingDecoder};
use serde_json::Value;

use super::{push_unique, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

/// The four bytes that begin a zstd frame; the suffix alone is not trusted.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// The raw input and the decoded output are each bounded at 64 MiB.
const MAX_BYTES: usize = 64 * 1024 * 1024;

pub struct Dsh;

impl SpendSource for Dsh {
    fn raw(&self) -> &'static str {
        "dsh"
    }
    fn source_id(&self) -> &'static str {
        "dsh"
    }
    fn display_name(&self) -> &'static str {
        "DeepSeek Harness"
    }
    fn icon(&self) -> Option<&'static str> {
        Some("deepseek")
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["DSH_HOME"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        // WINDOWS-PATH: unverified (the macOS layout carried over)
        let base = sources.var("DSH_HOME").unwrap_or_else(|| sources.home.join(".dsh"));
        push_unique(&mut roots, base.join("sessions"));
        roots
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        let mut records = Vec::new();
        for file in logio::files(roots, &[], &[], &[]) {
            let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !(name.starts_with("session.") && name.contains(".jsonl")) {
                continue;
            }
            let Some(bytes) = std::fs::read(&file).ok().and_then(jsonl_bytes) else { continue };
            records.extend(transcript_records(&bytes, &file));
        }
        records
    }
}

/// The transcript's bytes as JSONL: a plain file as it is, a zstd stream decoded. None when the stream
/// is corrupt or over a ceiling, so such a file contributes nothing and is never read as a smaller
/// figure.
pub(super) fn jsonl_bytes(raw: Vec<u8>) -> Option<Vec<u8>> {
    if raw.len() < ZSTD_MAGIC.len() || raw[..ZSTD_MAGIC.len()] != ZSTD_MAGIC {
        return Some(raw);
    }
    if raw.len() > MAX_BYTES {
        return None;
    }
    let mut out = Vec::new();
    let mut source: &[u8] = &raw;
    let mut frame = FrameDecoder::new();
    while !source.is_empty() {
        let before = source.len();
        let mut decoder = match StreamingDecoder::new_with_decoder(&mut source, &mut frame) {
            Ok(decoder) => decoder,
            // A header cut off at the end of the file is a torn tail; anything else is corrupt.
            Err(_) => return if source.is_empty() { Some(out) } else { None },
        };
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            match decoder.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    out.extend_from_slice(&chunk[..n]);
                    if out.len() > MAX_BYTES {
                        return None;
                    }
                }
                Err(_) => return if decoder.get_ref().is_empty() { Some(out) } else { None },
            }
        }
        drop(decoder);
        if source.len() == before {
            break;
        }
    }
    Some(out)
}

/// One transcript's increments. `file`'s folder names the session when the file names none.
fn transcript_records(bytes: &[u8], file: &std::path::Path) -> Vec<AgentUsageRecord> {
    let folder = file.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut records = Vec::new();
    let mut session_id: Option<String> = None;
    let mut workspace: Option<String> = None;
    let mut seed_length: i64 = 0;
    let mut header_provider: Option<String> = None;
    let mut header_model: Option<String> = None;

    for line in bytes.split(|b| *b == b'\n') {
        let Ok(event @ Value::Object(_)) = serde_json::from_slice::<Value>(line) else { continue };
        let Some(kind) = logio::text(event.get("type")) else { continue };
        let seq = logio::count(event.get("seq")).unwrap_or(0);
        let data = event.get("data").filter(|d| d.is_object());

        match kind.as_str() {
            "session" => {
                if session_id.is_none() {
                    session_id = logio::text(event.get("id"));
                }
                if workspace.is_none() {
                    workspace = logio::text(event.get("cwd"));
                }
                if seed_length == 0 {
                    seed_length = logio::count(event.get("seedLength")).unwrap_or(0);
                }
            }
            "request/header" => {
                let config = data.and_then(|d| d.get("header")).and_then(|h| h.get("config"));
                if let Some(provider) = logio::text(config.and_then(|c| c.get("provider"))) {
                    header_provider = Some(provider);
                }
                if let Some(model) = logio::text(config.and_then(|c| c.get("model"))) {
                    header_model = Some(model);
                }
            }
            "assistant/message" | "compaction/summary" => {
                // Inherited from a fork's parent, not this session's work.
                if seq < seed_length {
                    continue;
                }
                let Some(usage) = data.and_then(|d| d.get("usage")).filter(|u| u.is_object()) else { continue };
                let message = data.and_then(|d| d.get("message")).filter(|m| m.is_object());
                let source = message.and_then(|m| m.get("source")).filter(|s| s.is_object());
                let response = source.and_then(|s| s.get("replayState")).and_then(|r| r.get("response")).filter(|r| r.is_object());

                let model = logio::text(response.and_then(|r| r.get("responseModel")))
                    .or_else(|| logio::text(source.and_then(|s| s.get("model"))))
                    .or_else(|| header_model.clone());
                let Some(model) = model else { continue };
                let provider = logio::text(source.and_then(|s| s.get("provider"))).or_else(|| header_provider.clone());

                // A zero time is not a moment anyone measured.
                let Some(milliseconds) = logio::count(event.get("time")).filter(|ms| *ms > 0) else { continue };
                let Some(at) = DateTime::from_timestamp_millis(milliseconds) else { continue };

                let count = |key: &str| logio::count(usage.get(key)).unwrap_or(0);
                let reasoning = count("reasoningTokens");
                // `reasoningTokens` is a subset of `outputTokens`, which already counts it once.
                let tally = TokenTally::new(count("inputTokens"), count("cacheWriteTokens"), count("cacheReadTokens"), count("outputTokens"));

                let identity = if kind == "compaction/summary" {
                    logio::text(data.and_then(|d| d.get("compactionId")))
                        .map(|id| format!("summary:cmp:{id}"))
                        .unwrap_or_else(|| format!("seq:{seq}"))
                } else if let Some(id) = logio::text(message.and_then(|m| m.get("id"))) {
                    format!("msg:{id}")
                } else {
                    format!("assistant:seq:{seq}")
                };
                let provider_name = provider.clone().unwrap_or_default();
                let key = format!(
                    "dsh:{identity}:{milliseconds}:{provider_name}:{model}:{}:{}:{}:{}:{reasoning}",
                    tally.input, tally.output, tally.cache_read, tally.cache_write
                );
                if tally.total() <= 0 {
                    continue;
                }

                let resolved = session_id.clone().unwrap_or_else(|| folder.clone());
                let mut record = AgentUsageRecord::new(at, &model, tally).session(&resolved);
                record.session_name = provider;
                record.project = workspace.clone();
                record.deduplication_id = Some(key);
                records.push(record);
            }
            _ => {}
        }
    }
    records
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::summary::SpendSummary;
    use crate::spend::transcripts::Sources as Home;

    const MS: i64 = 1_789_381_230_000;

    /// A zstd stream holding one raw block per frame: a single-segment frame of `data`, padded to the
    /// 256 bytes the two-byte content size needs.
    fn zstd_frame(data: &[u8]) -> Vec<u8> {
        let mut padded = data.to_vec();
        while padded.len() < 256 {
            padded.push(b'\n');
        }
        let len = padded.len();
        let mut out = vec![0x28, 0xB5, 0x2F, 0xFD, 0x60];
        out.extend(((len - 256) as u16).to_le_bytes());
        // Last block, raw type, size in bits 3 and up.
        out.extend(&(((len as u32) << 3) | 1).to_le_bytes()[..3]);
        out.extend(padded);
        out
    }

    fn lines(values: &[Value]) -> Vec<u8> {
        values.iter().map(|v| format!("{v}\n")).collect::<String>().into_bytes()
    }

    fn reply(seq: i64, id: &str, usage: Value) -> Value {
        json!({"type": "assistant/message", "seq": seq, "time": MS, "data": {"usage": usage,
            "message": {"id": id, "source": {"model": "priced", "provider": "deepseek", "replayState": {"response": {"responseModel": "priced"}}}}}})
    }

    fn header() -> Value {
        json!({"type": "session", "seq": 0, "id": "dsh-1", "cwd": "C:\\Users\\me\\Code\\Pulse", "seedLength": 0})
    }

    fn write(home: &std::path::Path, folder: &str, name: &str, bytes: &[u8]) -> PathBuf {
        let dir = home.join(".dsh").join("sessions").join("encoded").join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(name);
        std::fs::write(&file, bytes).unwrap();
        file
    }

    #[test]
    fn the_four_kinds_land_where_upstream_says_and_reasoning_is_not_added_again() {
        let home = tempfile::tempdir().unwrap();
        let body = lines(&[header(), reply(1, "m1", json!({"inputTokens": 10, "cacheWriteTokens": 2, "cacheReadTokens": 30, "outputTokens": 40, "reasoningTokens": 9}))]);
        write(home.path(), "s1", "session.jsonl", &body);
        let records = Dsh.records(&Dsh.inputs(&Home::new(home.path())));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally, TokenTally::new(10, 2, 30, 40));
        assert!(!records[0].is_partial);
        assert_eq!(records[0].project.as_deref(), Some("C:\\Users\\me\\Code\\Pulse"));
        assert_eq!(records[0].session_name.as_deref(), Some("deepseek"));
        assert_eq!(records[0].session_id.as_deref(), Some("dsh-1"));
    }

    #[test]
    fn a_forked_prefix_is_skipped_and_a_compaction_summary_is_a_call() {
        let home = tempfile::tempdir().unwrap();
        let mut seeded = header();
        seeded["seedLength"] = json!(2);
        let summary = json!({"type": "compaction/summary", "seq": 3, "time": MS, "data": {"compactionId": "c9",
            "usage": {"inputTokens": 5, "outputTokens": 1}, "message": {"source": {"model": "priced"}}}});
        let body = lines(&[seeded, reply(1, "inherited", json!({"inputTokens": 999})), reply(2, "own", json!({"inputTokens": 7, "outputTokens": 1})), summary]);
        write(home.path(), "s2", "session.jsonl", &body);
        let records = Dsh.records(&Dsh.inputs(&Home::new(home.path())));
        let tallies: Vec<TokenTally> = records.iter().map(|r| r.tally.clone()).collect();
        assert_eq!(tallies, vec![TokenTally::new(7, 0, 0, 1), TokenTally::new(5, 0, 0, 1)]);
        assert_eq!(records[1].deduplication_id.as_deref().map(|d| d.contains("summary:cmp:c9")), Some(true));
    }

    #[test]
    fn a_message_present_in_two_copies_counts_once_and_a_row_with_no_time_or_model_is_skipped() {
        let home = tempfile::tempdir().unwrap();
        let untimed = json!({"type": "assistant/message", "seq": 2, "data": {"usage": {"inputTokens": 50}, "message": {"id": "x"}}});
        let body = lines(&[header(), reply(1, "dup", json!({"inputTokens": 4, "outputTokens": 1})), untimed]);
        write(home.path(), "s3", "session.jsonl", &body);
        write(home.path(), "s3", "session.v2.jsonl", &body);
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Dsh, &Home::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert_eq!(ledger.days.iter().map(|d| d.tokens).sum::<i64>(), 5);
    }

    #[test]
    fn a_zstd_transcript_is_decoded_by_its_magic_and_a_torn_tail_keeps_its_prefix() {
        let home = tempfile::tempdir().unwrap();
        let first = lines(&[header(), reply(1, "z1", json!({"inputTokens": 3, "outputTokens": 2}))]);
        let second = lines(&[reply(2, "z2", json!({"inputTokens": 100, "outputTokens": 0}))]);
        let mut stream = zstd_frame(&first);
        let torn = zstd_frame(&second);
        stream.extend_from_slice(&torn[..torn.len() - 40]);
        // The suffix does not decide it: the magic does.
        write(home.path(), "s4", "session.jsonl.zstd", &stream);
        let records = Dsh.records(&Dsh.inputs(&Home::new(home.path())));
        assert_eq!(records.iter().map(|r| r.tally.total()).sum::<i64>(), 5);
    }

    #[test]
    fn a_corrupt_zstd_file_contributes_nothing_and_a_full_stream_of_two_frames_counts_both() {
        let home = tempfile::tempdir().unwrap();
        let mut garbage = vec![0x28, 0xB5, 0x2F, 0xFD];
        garbage.extend(b"not a zstd frame at all, just noise");
        write(home.path(), "bad", "session.jsonl.zstd", &garbage);
        let mut both = zstd_frame(&lines(&[header(), reply(1, "a", json!({"inputTokens": 1, "outputTokens": 1}))]));
        both.extend(zstd_frame(&lines(&[reply(2, "b", json!({"inputTokens": 2, "outputTokens": 2}))])));
        write(home.path(), "good", "session.jsonl.zstd", &both);
        let records = Dsh.records(&Dsh.inputs(&Home::new(home.path())));
        assert_eq!(records.iter().map(|r| r.tally.total()).sum::<i64>(), 6);
    }

    #[test]
    fn a_missing_store_is_empty_and_the_home_override_moves_it() {
        let empty = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Dsh, &Home::new(empty.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());

        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let dir = elsewhere.path().join("sessions").join("e").join("s");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.jsonl"), lines(&[header(), reply(1, "o", json!({"inputTokens": 6}))])).unwrap();
        let sources = Home::new(home.path()).with_var("DSH_HOME", elsewhere.path());
        assert_eq!(Dsh.records(&Dsh.inputs(&sources)).len(), 1);
    }

    #[test]
    fn a_session_buckets_reach_today_and_only_todays_counts() {
        use chrono::{Duration, Utc};
        let home = tempfile::tempdir().unwrap();
        let calendar = Calendar::local();
        let now = Utc::now();
        let ms = |day: DateTime<Utc>| (calendar.start_of_day(day) + Duration::hours(12)).timestamp_millis();
        let at = |seq: i64, when: i64, input: i64| json!({"type": "assistant/message", "seq": seq, "time": when, "data": {"usage": {"inputTokens": input}, "message": {"id": format!("m{seq}"), "source": {"model": "priced"}}}});
        let yesterday = calendar.add_days(calendar.start_of_day(now), -1);
        write(home.path(), "s5", "session.jsonl", &lines(&[header(), at(1, ms(now), 100), at(2, ms(yesterday), 900)]));
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Dsh, &Home::new(home.path()), cache.path(), &calendar, &PriceTable::new());
        let summary = SpendSummary::of(&std::collections::HashMap::from([(SpendAgent::Dsh, ledger)]), Some(1), now, &calendar);
        assert_eq!(summary.tokens, 100);
        assert_eq!(summary.sessions[0].session.unpriced_tokens, 100);
    }
}
