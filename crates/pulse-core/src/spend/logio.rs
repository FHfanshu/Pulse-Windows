// Ported from upstream Sources/Pulse/Usage/AgentLogIO.swift (files, JSON, numbers, time).
//! The shared read-only helpers every record reader uses, so two readers cannot disagree about
//! what "malformed" or "missing" means. The SQLite helpers (`AgentSQLite`) belong to the readers
//! that need them and are not ported yet.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::loglines::LineReader;

/// Every regular file under the roots, deduplicated, in a stable order.
///
/// A root may be a directory (walked, hidden logs included) or a single file. `-shm` files are
/// shared memory, not data, and are never a source. With `extensions` or `names` given, a file
/// matches when either does; with neither, every file does. A directory directly under a root
/// whose name is in `excluding_root_directories` is skipped whole.
pub fn files(roots: &[PathBuf], extensions: &[&str], names: &[&str], excluding_root_directories: &[&str]) -> Vec<PathBuf> {
    let wants_filter = !extensions.is_empty() || !names.is_empty();
    let matches = |path: &Path| -> bool {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.ends_with("-shm") {
            return false;
        }
        if !wants_filter {
            return true;
        }
        let extension = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        (!extensions.is_empty() && extensions.contains(&extension.as_str())) || (!names.is_empty() && names.contains(&name.as_str()))
    };

    let mut ordered: Vec<&PathBuf> = roots.iter().collect();
    ordered.sort();
    ordered.dedup();

    let mut candidates: Vec<PathBuf> = Vec::new();
    for root in ordered {
        let Ok(metadata) = std::fs::metadata(root) else { continue };
        if metadata.is_file() {
            if matches(root) {
                candidates.push(root.clone());
            }
            continue;
        }
        let mut pending = vec![root.clone()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_dir() {
                    let excluded = directory == *root
                        && excluding_root_directories.iter().any(|name| entry.file_name().to_string_lossy() == *name);
                    if !excluded {
                        pending.push(path);
                    }
                } else if kind.is_file() && matches(&path) {
                    candidates.push(path);
                }
            }
        }
    }

    // Aliases of one file (a symlink, a junction) count once.
    let mut resolved: Vec<(PathBuf, PathBuf)> =
        candidates.into_iter().map(|path| (std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()), path)).collect();
    resolved.sort();
    let mut seen = HashSet::new();
    resolved.into_iter().filter(|(canonical, _)| seen.insert(canonical.clone())).map(|(_, path)| path).collect()
}

/// A whole JSON file, or None when it is missing or malformed: never a partial value.
pub fn json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// The JSON objects of a JSONL file, one at a time. A line that is not an object (malformed,
/// an array, a scalar) is skipped.
pub fn json_lines(path: &Path) -> impl Iterator<Item = Value> {
    let mut lines = std::fs::File::open(path).ok().map(LineReader::new);
    std::iter::from_fn(move || {
        let lines = lines.as_mut()?;
        while let Some(line) = lines.next_line() {
            if let Ok(value @ Value::Object(_)) = serde_json::from_slice::<Value>(line) {
                return Some(value);
            }
        }
        None
    })
}

/// A string with its surrounding whitespace removed; None when nothing is left or it is not a
/// string.
pub fn text(value: Option<&Value>) -> Option<String> {
    let trimmed = value?.as_str()?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// A count: a whole, finite, non-negative number that fits, as a number or a numeric string.
/// Never a boolean, a fraction or an overflow; missing is not zero.
pub fn count(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return (i >= 0).then_some(i);
            }
            if n.as_u64().is_some() {
                return None; // above i64::MAX
            }
            let f = n.as_f64()?;
            (f.is_finite() && f >= 0.0 && f.fract() == 0.0 && f < 9.223_372_036_854_776e18).then_some(f as i64)
        }
        Value::String(s) => s.trim().parse::<i64>().ok().filter(|v| *v >= 0),
        _ => None,
    }
}

/// An instant from an ISO-8601 string (fractions and offsets honoured) or a number in seconds
/// (milliseconds when the caller says so). A numeric string is a number; a boolean is not a time.
pub fn timestamp(value: Option<&Value>, milliseconds: bool) -> Option<DateTime<Utc>> {
    fn from_number(number: f64, milliseconds: bool) -> Option<DateTime<Utc>> {
        if !number.is_finite() {
            return None;
        }
        let seconds = if milliseconds { number / 1000.0 } else { number };
        if !seconds.is_finite() || seconds.abs() > 1.0e11 {
            return None;
        }
        DateTime::<Utc>::from_timestamp_micros((seconds * 1_000_000.0).round() as i64)
    }
    match value? {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            if let Ok(number) = trimmed.parse::<f64>() {
                return from_number(number, milliseconds);
            }
            DateTime::parse_from_rfc3339(trimmed).ok().map(|d| d.with_timezone(&Utc))
        }
        Value::Number(n) => from_number(n.as_f64()?, milliseconds),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_count_is_a_whole_finite_in_range_number_never_a_boolean_fraction_or_overflow() {
        assert_eq!(count(Some(&json!(42))), Some(42));
        assert_eq!(count(Some(&json!(2.0))), Some(2));
        assert_eq!(count(Some(&json!(true))), None);
        assert_eq!(count(Some(&json!(2.5))), None);
        assert_eq!(count(Some(&json!(-1))), None);
        assert_eq!(count(Some(&json!(u64::MAX))), None);
        assert_eq!(count(Some(&json!(f64::MAX))), None);
        // A numeric string is a count only as a whole number.
        assert_eq!(count(Some(&json!("42"))), Some(42));
        assert_eq!(count(Some(&json!("-1"))), None);
        assert_eq!(count(Some(&json!("2.5"))), None);
        assert_eq!(count(Some(&json!("nope"))), None);
        // Missing is not zero.
        assert_eq!(count(None), None);
    }

    #[test]
    fn timestamps_read_fractions_and_offsets_numbers_take_the_unit_the_caller_states() {
        let secs = |v: &Value, ms| timestamp(Some(v), ms).map(|d| d.timestamp_micros() as f64 / 1e6);
        assert_eq!(secs(&json!("1970-01-01T00:00:00Z"), false), Some(0.0));
        assert_eq!(secs(&json!("1970-01-01T00:00:00.500Z"), false), Some(0.5));
        // A stated offset is honoured, not ignored.
        assert_eq!(secs(&json!("1970-01-01T08:00:00+08:00"), false), Some(0.0));
        // Seconds by default, milliseconds only when asked.
        assert_eq!(secs(&json!(1000), false), Some(1000.0));
        assert_eq!(secs(&json!(1000), true), Some(1.0));
        assert_eq!(secs(&json!("1000"), false), Some(1000.0));
        assert_eq!(secs(&json!(true), false), None);
        assert_eq!(secs(&json!("not a date"), false), None);
        assert_eq!(timestamp(None, false), None);
    }

    #[test]
    fn malformed_json_is_none_or_a_skipped_line_never_a_partial_value() {
        let root = tempfile::tempdir().unwrap();
        let good = root.path().join("good.json");
        std::fs::write(&good, r#"{"model":"x","count":3}"#).unwrap();
        assert_eq!(json(&good).unwrap()["model"], "x");
        let bad = root.path().join("bad.json");
        std::fs::write(&bad, "{not json").unwrap();
        assert!(json(&bad).is_none());
        assert!(json(&root.path().join("missing.json")).is_none());

        let lines = root.path().join("lines.jsonl");
        std::fs::write(&lines, "{\"a\":1}\nthis is not json\n{\"b\":2}\n[1,2,3]\n\n{\"c\":3}\n").unwrap();
        let rows: Vec<Value> = json_lines(&lines).collect();
        assert_eq!(rows.len(), 3);
        let seen: Vec<i64> = rows.iter().filter_map(|r| r["a"].as_i64().or(r["b"].as_i64()).or(r["c"].as_i64())).collect();
        assert_eq!(seen, [1, 2, 3]);
        assert_eq!(json_lines(&root.path().join("missing.jsonl")).count(), 0);

        assert_eq!(text(Some(&json!("  hi  "))).as_deref(), Some("hi"));
        assert_eq!(text(Some(&json!("   "))), None);
        assert_eq!(text(Some(&json!(5))), None);
    }

    #[test]
    fn files_include_hidden_logs_survive_duplicate_roots_and_exclude_shm() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().to_path_buf();
        std::fs::create_dir_all(root.join(".logs")).unwrap();
        std::fs::write(root.join(".logs/capture.jsonl"), "{}\n").unwrap();
        std::fs::write(root.join("state.json"), "{}\n").unwrap();
        std::fs::write(root.join("x.db-shm"), "shared").unwrap();
        let names = |files: Vec<PathBuf>| {
            let mut names: Vec<String> = files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
            names.sort();
            names
        };

        // Both the hidden log and the visible state file are found; the shared memory index is
        // not a source.
        let all = files(std::slice::from_ref(&root), &[], &[], &[]);
        assert_eq!(names(all.clone()), ["capture.jsonl", "state.json"]);
        // A duplicate root is walked once.
        assert_eq!(files(&[root.clone(), root.clone()], &[], &[], &[]).len(), all.len());
        // Filters: either matches when both are given.
        assert_eq!(names(files(std::slice::from_ref(&root), &["jsonl"], &[], &[])), ["capture.jsonl"]);
        assert_eq!(names(files(std::slice::from_ref(&root), &["jsonl"], &["state.json"], &[])), ["capture.jsonl", "state.json"]);
        assert_eq!(names(files(std::slice::from_ref(&root), &[], &["state.json"], &[])), ["state.json"]);
        // A single file root is honoured directly.
        assert_eq!(names(files(&[root.join("state.json")], &[], &[], &[])), ["state.json"]);
        // A directory directly under the root can be left out.
        assert_eq!(names(files(std::slice::from_ref(&root), &[], &[], &[".logs"])), ["state.json"]);
    }
}
