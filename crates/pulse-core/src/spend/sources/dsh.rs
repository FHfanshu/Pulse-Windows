// Ported from upstream Sources/Pulse/Usage/Readers/DSHUsageReader.swift.
//! DeepSeek Harness: ~/.dsh/sessions/<encoded-cwd>/<session>/session[.vN].jsonl[.zstd].
//! DSH_HOME replaces ~/.dsh. Verified on Windows with compressed session logs.
//! Assistant replies and compactions are additive calls. Reasoning is already inside output;
//! input, cache read and cache write are separate buckets. Forks skip seq < seedLength.
//! Compression follows the frame magic, not the suffix. Concatenated frames and a torn final
//! frame preserve complete JSONL events; corrupt/oversized compressed files report a limitation.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use zstd::stream::raw::{Decoder, InBuffer, Operation, OutBuffer};

use super::{SourceRead, SpendSource};
use crate::provider::Provider;
use crate::spend::{logio, record::AgentUsageRecord, tally::TokenTally, transcripts::Sources};

const LIMIT: usize = 64 * 1024 * 1024;
const MAGIC: &[u8] = &[0x28, 0xb5, 0x2f, 0xfd];

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
    fn card_provider(&self) -> Option<Provider> {
        Some(Provider::DeepSeek)
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["DSH_HOME"]
    }
    fn has_captured_validation(&self) -> bool {
        true
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        vec![sources
            .var("DSH_HOME")
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| sources.home.join(".dsh"))
            .join("sessions")]
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        self.read_records(roots).records
    }
    fn read_records(&self, roots: &[PathBuf]) -> SourceRead {
        let mut read = SourceRead::default();
        for file in logio::files(roots, &[], &[], &[]).into_iter().filter(|p| is_transcript(p)) {
            match transcript(&file) {
                Ok(bytes) => read.records.extend(events(&bytes, &file)),
                Err(_) => read.has_read_limitations = true,
            }
        }
        read
    }
}

fn is_transcript(path: &Path) -> bool {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let stem = name.strip_suffix(".zstd").unwrap_or(name);
    stem == "session.jsonl"
        || stem
            .strip_prefix("session.v")
            .and_then(|s| s.strip_suffix(".jsonl"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn transcript(file: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(file)?.take((LIMIT + 1) as u64).read_to_end(&mut bytes)?;
    decode(&bytes, LIMIT, LIMIT)
}

/// A zstd stream decoded under the same 64 MiB ceilings and window bound as a Harness transcript; bytes
/// without the frame magic pass through. None when the stream is corrupt or over a ceiling. Zed's
/// compressed thread rows share it.
pub(super) fn decode_zstd(bytes: &[u8]) -> Option<Vec<u8>> {
    decode(bytes, LIMIT, LIMIT).ok()
}

fn decode(bytes: &[u8], raw_limit: usize, decoded_limit: usize) -> io::Result<Vec<u8>> {
    let too_large = || io::Error::new(io::ErrorKind::InvalidData, "transcript exceeds decoding ceiling");
    if bytes.len() > raw_limit {
        return Err(too_large());
    }
    if !bytes.starts_with(MAGIC) {
        return if bytes.len() <= decoded_limit {
            Ok(bytes.to_vec())
        } else {
            Err(too_large())
        };
    }
    let mut decoder = Decoder::new()?;
    // Bound the decoder's own history buffer too, before accepting an untrusted frame header.
    decoder.set_parameter(zstd::stream::raw::DParameter::WindowLogMax(26))?;
    let mut input = InBuffer::around(bytes);
    let mut output = Vec::new();
    let mut chunk = vec![0; 256 * 1024];
    loop {
        let before = input.pos();
        let mut buffer = OutBuffer::around(chunk.as_mut_slice());
        let remaining = decoder.run(&mut input, &mut buffer)?;
        let written = buffer.pos();
        if output.len() + written > decoded_limit {
            return Err(too_large());
        }
        output.extend_from_slice(&chunk[..written]);
        if input.pos() == bytes.len() && (remaining == 0 || written < chunk.len()) {
            // A live writer may have left the last frame torn. Complete lines still count.
            return Ok(output);
        }
        if written == 0 && input.pos() == before {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "zstd decoder made no progress"));
        }
    }
}

// Decode only metadata. Message content and replay bodies may be much larger than the usage.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    seq: Option<Value>,
    #[serde(default)]
    time: Option<Value>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    seed_length: Option<Value>,
    #[serde(default)]
    data: Option<Data>,
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Data {
    usage: Option<Value>,
    message: Option<Message>,
    header: Option<Header>,
    compaction_id: Option<String>,
}
#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    source: Option<Source>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Source {
    model: Option<String>,
    provider: Option<String>,
    replay_state: Option<Replay>,
}
#[derive(Deserialize)]
struct Replay {
    response: Option<Response>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    response_model: Option<String>,
}
#[derive(Deserialize)]
struct Header {
    config: Option<Config>,
}
#[derive(Deserialize)]
struct Config {
    model: Option<String>,
    provider: Option<String>,
}

fn events(bytes: &[u8], file: &Path) -> Vec<AgentUsageRecord> {
    let mut records = Vec::new();
    let mut session = None;
    let mut workspace = None;
    let mut seed = 0;
    let mut header_model = None;
    let mut header_provider = None;
    for line in bytes.split(|b| *b == b'\n') {
        let Ok(event) = serde_json::from_slice::<Event>(line) else {
            continue;
        };
        let seq = logio::count(event.seq.as_ref()).unwrap_or(0);
        match event.kind.as_str() {
            "session" => {
                session = session.or(event.id);
                workspace = workspace.or(event.cwd);
                if seed == 0 {
                    seed = logio::count(event.seed_length.as_ref()).unwrap_or(0);
                }
            }
            "request/header" => {
                if let Some(config) = event.data.and_then(|d| d.header).and_then(|h| h.config) {
                    header_model = config.model.or(header_model);
                    header_provider = config.provider.or(header_provider);
                }
            }
            "assistant/message" | "compaction/summary" if seq >= seed => {
                let Some(data) = event.data else { continue };
                let Some(usage) = data.usage.as_ref().and_then(Value::as_object) else {
                    continue;
                };
                let source = data.message.as_ref().and_then(|m| m.source.as_ref());
                let model = source
                    .and_then(|s| s.replay_state.as_ref())
                    .and_then(|r| r.response.as_ref())
                    .and_then(|r| r.response_model.as_deref())
                    .or_else(|| source.and_then(|s| s.model.as_deref()))
                    .or(header_model.as_deref());
                let Some(model) = model.filter(|m| !m.trim().is_empty()) else {
                    continue;
                };
                let provider = source.and_then(|s| s.provider.as_deref()).or(header_provider.as_deref());
                let Some(ms) = logio::count(event.time.as_ref()).filter(|t| *t > 0) else {
                    continue;
                };
                let Some(at) = chrono::DateTime::from_timestamp_millis(ms) else {
                    continue;
                };
                let count = |key: &str| logio::count(usage.get(key)).unwrap_or(0);
                let tally = TokenTally::new(
                    count("inputTokens"),
                    count("cacheWriteTokens"),
                    count("cacheReadTokens"),
                    count("outputTokens"),
                );
                if tally.input == 0 && tally.output == 0 && tally.cache_read == 0 && tally.cache_write == 0 {
                    continue;
                }
                let identity = if event.kind == "compaction/summary" {
                    data.compaction_id
                        .map(|id| format!("summary:cmp:{id}"))
                        .unwrap_or_else(|| format!("seq:{seq}"))
                } else {
                    data.message
                        .as_ref()
                        .and_then(|m| m.id.as_ref())
                        .map(|id| format!("msg:{id}"))
                        .unwrap_or_else(|| format!("assistant:seq:{seq}"))
                };
                let key = format!(
                    "dsh:{identity}:{ms}:{}:{model}:{}:{}:{}:{}:{}",
                    provider.unwrap_or(""),
                    tally.input,
                    tally.output,
                    tally.cache_read,
                    tally.cache_write,
                    count("reasoningTokens")
                );
                let fallback = file.parent().and_then(Path::file_name).and_then(|s| s.to_str()).unwrap_or("");
                let mut record =
                    AgentUsageRecord::new(at, model, tally).session(session.as_deref().unwrap_or(fallback));
                record.session_name = provider.map(str::to_string);
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
    use super::*;
    use crate::spend::{
        sources::{read_agent, SpendAgent},
        Calendar, PriceTable,
    };
    use serde_json::json;

    const AT: i64 = 1_789_344_000_000;

    fn header(id: &str, seed: i64) -> Value {
        json!({"type":"session","id":id,"cwd":"C:\\code\\Pulse","seedLength":seed})
    }
    fn config() -> Value {
        json!({"type":"request/header","data":{"header":{"config":{"model":"fallback","provider":"deepseek"}}}})
    }
    fn reply(seq: i64, id: &str) -> Value {
        json!({"type":"assistant/message","seq":seq,"time":AT+seq,
            "data":{"usage":{"inputTokens":10,"outputTokens":20,"cacheReadTokens":30,"cacheWriteTokens":40,"reasoningTokens":5},
                "message":{"id":id,"content":[{"type":"text","text":"ignored body"}],
                    "source":{"provider":"deepseek","model":"alias","replayState":{"response":{"responseModel":"reported"}}}}}})
    }
    fn text(events: &[Value]) -> Vec<u8> {
        let mut bytes = events.iter().map(Value::to_string).collect::<Vec<_>>().join("\n").into_bytes();
        bytes.push(b'\n');
        bytes
    }
    fn write(home: &Path, session: &str, name: &str, bytes: &[u8]) -> PathBuf {
        let path = home.join(".dsh/sessions/project").join(session).join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn replies_and_compactions_count_cache_and_reasoning_once_with_header_fallback() {
        let summary = json!({"type":"compaction/summary","seq":2,"time":AT+2,
            "data":{"compactionId":"compact","usage":{"inputTokens":7,"outputTokens":3,"reasoningTokens":2}}});
        let file = Path::new("project/session/session.jsonl");
        let records = events(&text(&[header("s", 0), config(), reply(1, "m"), summary]), file);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].tally, TokenTally::new(10, 40, 30, 20));
        assert_eq!(records[0].model, "reported");
        assert_eq!(records[0].project.as_deref(), Some("C:\\code\\Pulse"));
        assert_eq!(records[0].session_id.as_deref(), Some("s"));
        assert_eq!(records[1].model, "fallback");
        assert_eq!(records[1].tally, TokenTally::new(7, 0, 0, 3));
        let ledger = crate::spend::record::build_ledger(
            &records,
            &PriceTable::new(),
            "dsh",
            None,
            &Calendar::utc(2),
            Default::default(),
        );
        assert_eq!(ledger.all_time().tokens, 110);
        assert_eq!(ledger.sessions.len(), 1);
        assert_eq!(ledger.sessions[0].tokens, 110);
        assert_eq!(ledger.sessions[0].days[0].tokens, 110);
        assert!(!ledger.slots.is_empty());
    }

    #[test]
    fn compressed_copies_and_forked_prefixes_count_once_and_backups_are_not_inputs() {
        let home = tempfile::tempdir().unwrap();
        let original = text(&[header("s", 0), reply(1, "original")]);
        write(home.path(), "s", "session.jsonl", &original);
        // Plain JSONL despite the compression suffix; frame magic decides.
        write(home.path(), "s", "session.jsonl.zstd", &original);
        let fork = text(&[header("fork", 2), reply(1, "original"), reply(2, "new")]);
        write(home.path(), "fork", "session.v4.jsonl.zstd", &zstd::stream::encode_all(fork.as_slice(), 1).unwrap());
        write(home.path(), "s", "session.jsonl.zstd.bak", &text(&[header("backup", 0), reply(9, "backup")]));
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(
            SpendAgent::Dsh,
            &Sources::new(home.path()),
            cache.path(),
            &Calendar::utc(2),
            &PriceTable::new(),
        );
        assert_eq!(ledger.all_time().tokens, 200);
        assert_eq!(ledger.sessions.len(), 2);
        assert!(!ledger.has_read_limitations);
        let card = crate::spend::read_card_ledger_with(
            Provider::DeepSeek,
            &Sources::new(home.path()),
            cache.path(),
            &Calendar::utc(2),
            &PriceTable::new(),
            chrono::Utc::now(),
        )
        .unwrap();
        assert_eq!(card.all_time().tokens, 200);
        assert_eq!(
            ledger,
            read_agent(
                SpendAgent::Dsh,
                &Sources::new(home.path()),
                cache.path(),
                &Calendar::utc(2),
                &PriceTable::new()
            )
        );
    }

    #[test]
    fn concatenated_frames_and_a_torn_tail_keep_complete_lines() {
        let a = text(&[header("s", 0), reply(1, "a")]);
        let b = text(&[reply(2, "b")]);
        let mut compressed = zstd::stream::encode_all(a.as_slice(), 1).unwrap();
        compressed.extend(zstd::stream::encode_all(b.as_slice(), 1).unwrap());
        assert_eq!(decode(&compressed, LIMIT, LIMIT).unwrap(), [a.clone(), b].concat());
        let tail = zstd::stream::encode_all(b"{\"type\":\"assistant/message\",\"seq\":3".as_slice(), 1).unwrap();
        compressed.extend_from_slice(&tail[..tail.len() - 2]);
        let decoded = decode(&compressed, LIMIT, LIMIT).unwrap();
        assert_eq!(events(&decoded, Path::new("s/session.jsonl")).len(), 2);
        // More than a decoder output chunk must not be lost when the input buffer runs out.
        let large = vec![b'x'; 600_000];
        let packed = zstd::stream::encode_all(large.as_slice(), 1).unwrap();
        assert_eq!(decode(&packed, LIMIT, LIMIT).unwrap(), large);
    }

    #[test]
    fn decode_bounds_and_corruption_are_reported_even_when_other_files_read_and_when_cached() {
        let packed = zstd::stream::encode_all(vec![b'x'; 1024].as_slice(), 1).unwrap();
        assert!(decode(&packed, packed.len() - 1, 2048).is_err());
        assert!(decode(&packed, 2048, 1023).is_err());
        assert!(decode(b"plain", 4, 10).is_err());
        let mut encoder = zstd::stream::Encoder::new(Vec::new(), 1).unwrap();
        encoder.include_checksum(true).unwrap();
        std::io::Write::write_all(&mut encoder, b"complete transcript").unwrap();
        let mut checked = encoder.finish().unwrap();
        *checked.last_mut().unwrap() ^= 1;
        assert!(decode(&checked, LIMIT, LIMIT).is_err(), "a bad checksum is corruption, not a torn tail");
        let home = tempfile::tempdir().unwrap();
        write(home.path(), "ok", "session.jsonl", &text(&[header("ok", 0), reply(1, "ok")]));
        write(home.path(), "bad", "session.jsonl.zstd", &[MAGIC, b"bad frame"].concat());
        let sources = Sources::new(home.path());
        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Dsh, &sources, cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert_eq!(ledger.all_time().tokens, 100);
        assert!(ledger.has_read_limitations);
        assert_eq!(read_agent(SpendAgent::Dsh, &sources, cache.path(), &Calendar::utc(2), &PriceTable::new()), ledger);
        write(home.path(), "bad", "session.jsonl.zstd", &text(&[header("now-readable", 0), reply(2, "fixed")]));
        let fixed = read_agent(SpendAgent::Dsh, &sources, cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert_eq!(fixed.all_time().tokens, 200);
        assert!(!fixed.has_read_limitations);
    }

    #[test]
    fn malformed_unreported_and_untimed_events_contribute_nothing_and_overrides_are_honoured() {
        let home = tempfile::tempdir().unwrap();
        let custom = tempfile::tempdir().unwrap();
        let file = write(
            custom.path(),
            "s",
            "session.jsonl",
            &text(&[
                header("s", 0),
                json!({"type":"user/message","seq":1,"time":AT,"data":{"usage":{"inputTokens":999}}}),
                json!({"type":"assistant/message","seq":2,"time":AT,"data":{"usage":{"inputTokens":9}}}), // no model
                json!({"type":"assistant/message","seq":3,"data":{"message":{"source":{"model":"x"}},"usage":{"inputTokens":9}}}), // no time
                json!({"type":"assistant/message","seq":4,"time":AT,"data":{"message":{"source":{"model":"x"}}}}), // no usage
                reply(5, "good"),
            ]),
        );
        let sources = Sources::new(home.path()).with_var("DSH_HOME", custom.path().join(".dsh"));
        assert!(SpendAgent::Dsh.inputs(&sources)[0].exists());
        assert!(!SpendAgent::Dsh.is_present(&Sources::new(home.path())));
        let records = Dsh.records(std::slice::from_ref(&file));
        assert_eq!(records.len(), 1);
        assert!(events(b"malformed\n[]\n", &file).is_empty());
        assert!(Dsh.records(&SpendAgent::Dsh.inputs(&Sources::new(home.path()))).is_empty());
        assert_eq!(SpendAgent::Dsh.card_provider(), Some(Provider::DeepSeek));
        assert_eq!(SpendAgent::Dsh.icon_resource(), Some("deepseek"));
    }
}

#[cfg(test)]
mod existing_tests {
    use chrono::DateTime;
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
        let body = lines(&[
            header(),
            reply(
                1,
                "m1",
                json!({"inputTokens": 10, "cacheWriteTokens": 2, "cacheReadTokens": 30, "outputTokens": 40, "reasoningTokens": 9}),
            ),
        ]);
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
        let body = lines(&[
            seeded,
            reply(1, "inherited", json!({"inputTokens": 999})),
            reply(2, "own", json!({"inputTokens": 7, "outputTokens": 1})),
            summary,
        ]);
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
        let ledger =
            read_agent(SpendAgent::Dsh, &Home::new(home.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
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
        let ledger =
            read_agent(SpendAgent::Dsh, &Home::new(empty.path()), cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());

        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let dir = elsewhere.path().join("sessions").join("e").join("s");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.jsonl"), lines(&[header(), reply(1, "o", json!({"inputTokens": 6}))]))
            .unwrap();
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
        let summary =
            SpendSummary::of(&std::collections::HashMap::from([(SpendAgent::Dsh, ledger)]), Some(1), now, &calendar);
        assert_eq!(summary.tokens, 100);
        assert_eq!(summary.sessions[0].session.unpriced_tokens, 100);
    }
}
