//! Ported from upstream SpendReadPerformanceTests (synthetic, no user data): a Codex rollout
//! mixing irrelevant 8 KiB tool-output rows with cumulative usage events must keep the exact
//! deltas while only streaming the file.

use std::io::Write;
use std::path::Path;

use super::calendar::Calendar;
use super::tally::TokenTally;
use super::transcripts::parse_codex_file;

fn write_big_codex(file: &Path, filler_rows: usize, events: usize) {
    let mut out = std::io::BufWriter::new(std::fs::File::create(file).unwrap());
    let body = "x".repeat(8 * 1024);
    writeln!(out, r#"{{"payload":{{"cwd":"/work/big","model":"gpt-5"}}}}"#).unwrap();
    let per_event = filler_rows / events.max(1);
    let mut total = 0;
    for event in 0..events {
        for _ in 0..per_event {
            writeln!(
                out,
                r#"{{"timestamp":"2026-01-02T09:00:00Z","type":"response_item","payload":{{"type":"function_call_output","output":"{body}"}}}}"#
            )
            .unwrap();
        }
        total += 100;
        writeln!(
            out,
            r#"{{"timestamp":"2026-01-02T09:{:02}:00Z","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{total},"cached_input_tokens":{},"output_tokens":{}}}}}}}}}"#,
            event % 60,
            total / 2,
            event + 1
        )
        .unwrap();
    }
}

fn total(scanned: &super::transcripts::Scanned) -> TokenTally {
    scanned.all_days().values().flat_map(|m| m.values().cloned()).sum()
}

#[test]
fn a_large_codex_rollout_is_streamed_and_its_deltas_are_exact() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("big.jsonl");
    write_big_codex(&file, 2_400, 200);
    assert!(std::fs::metadata(&file).unwrap().len() > 16 * 1024 * 1024);
    let started = std::time::Instant::now();
    let scanned = parse_codex_file(&file, &Calendar::local());
    // The last reading: 20,000 input of which 10,000 cached, 200 output.
    assert_eq!(total(&scanned), TokenTally::new(10_000, 0, 10_000, 200));
    assert_eq!(scanned.cwd.as_deref(), Some("/work/big"));
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
}

#[test]
#[ignore = "multi-hundred-megabyte synthetic read; run with --ignored --release"]
fn a_huge_codex_rollout_reads_in_bounded_time() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("huge.jsonl");
    write_big_codex(&file, 120_000, 1_024);
    let started = std::time::Instant::now();
    let scanned = parse_codex_file(&file, &Calendar::local());
    eprintln!("{} MB in {:?}", std::fs::metadata(&file).unwrap().len() / 1_000_000, started.elapsed());
    assert_eq!(total(&scanned).output, 1_024);
}
