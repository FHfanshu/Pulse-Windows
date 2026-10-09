// Ported from upstream Sources/Pulse/Usage/Readers/EditorLogSupport.swift.
//! Decoding helpers shared by the editor and agent readers in `sources/` (Group B and the
//! other JSON-log families that read the same way).
//!
//! Deliberately narrow: it decodes a value a product actually wrote and never turns a length, a
//! duration, a cost or a missing field into a token count. A missing key is `None`; a record is
//! only built when the store named a model and a real timestamp.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::logio;
use super::tally::TokenTally;

/// A JSON object, as the readers hold one.
pub type Object = Map<String, Value>;

/// Parses a JSON object out of text a log field carries as a string. Malformed or non-object text
/// is `None`, never a partial object.
pub fn json_object(text: &str) -> Option<Object> {
    serde_json::from_str::<Value>(text).ok()?.as_object().cloned()
}

/// The first balanced `{...}` in `text`, unescaped, as an object. A log line can put a usage
/// object in the middle of a sentence and keep talking; the first balanced object is read without
/// guessing where the line ends.
pub fn brace_object(text: &str) -> Option<Object> {
    let start = text.find('{')?;
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, c) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                return json_object(&text[start..start + offset + c.len_utf8()]);
            }
        }
    }
    None
}

/// The first key that carries a whole count, preferring a positive one. A present zero is
/// returned when no key is positive, so "reported as zero" is not confused with "not reported".
pub fn first_count(object: &Object, keys: &[&str]) -> Option<i64> {
    let mut zero = None;
    for key in keys {
        let Some(value) = logio::count(object.get(*key)) else { continue };
        if value > 0 {
            return Some(value);
        }
        zero.get_or_insert(value);
    }
    zero
}

/// A whole count, with a missing or malformed value read as zero. Only for a bucket where zero and
/// absent mean the same thing to the arithmetic downstream.
pub fn int(value: Option<&Value>) -> i64 {
    logio::count(value).unwrap_or(0)
}

/// A model id with a gateway's provider prefix removed (`provider/model` becomes `model`). The
/// price table is keyed by the bare id models.dev publishes. `raw` is already trimmed text.
pub fn model_id(raw: Option<String>) -> Option<String> {
    let raw = raw?;
    match raw.rfind('/') {
        Some(slash) if slash + 1 < raw.len() => Some(raw[slash + 1..].to_string()),
        _ => Some(raw),
    }
}

/// An instant from an epoch value whose unit the schema fixes per branch: milliseconds above the
/// threshold, seconds below it (WorkBuddy's `updated_at`). The threshold is the schema's.
pub fn auto_epoch(value: Option<i64>) -> Option<DateTime<Utc>> {
    let value = value.filter(|v| *v > 0)?;
    if value > 10_000_000_000 {
        DateTime::from_timestamp_millis(value)
    } else {
        DateTime::from_timestamp(value, 0)
    }
}

/// A naive local `YYYY/MM/DD HH:MM:SS[.fff]` (or dashed) prefix, which is what the Tencent
/// extension log writes. The wall-clock string is the timestamp, read in this PC's zone; nothing
/// is filled from a file's date or the clock.
pub fn naive_timestamp(line: &str) -> Option<DateTime<Utc>> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    for format in ["%Y/%m/%d %H:%M:%S%.3f", "%Y/%m/%d %H:%M:%S", "%Y-%m-%d %H:%M:%S%.3f", "%Y-%m-%d %H:%M:%S"] {
        // The length of the format's rendered text, in characters, is the prefix that must match.
        let length = format_length(format);
        let candidate: String = trimmed.chars().take(length).collect();
        if candidate.chars().count() != length {
            continue;
        }
        if let Ok(naive) = NaiveDateTime::parse_from_str(&candidate, format) {
            return local_to_utc(naive);
        }
    }
    None
}

/// The character length of a `naive_timestamp` format's text (`%Y/%m/%d %H:%M:%S%.3f` is 23).
fn format_length(format: &str) -> usize {
    if format.ends_with("%.3f") {
        23
    } else {
        19
    }
}

fn local_to_utc(naive: NaiveDateTime) -> Option<DateTime<Utc>> {
    Local.from_local_datetime(&naive).earliest().map(|at| at.with_timezone(&Utc))
}

/// A deterministic content digest, used only as a fragment identity for a whole-file mirror.
pub fn digest(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// One usage object reduced to Pulse's four disjoint buckets, plus a total the store reported that
/// the named kinds do not add up to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageParts {
    pub tally: TokenTally,
    pub unclassified: i64,
}

/// The counts a store reported for one usage object, each `None` when its key was absent.
#[derive(Debug, Clone, Default)]
pub struct Reported {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cache_read: Option<i64>,
    pub cache_write: Option<i64>,
    pub reasoning: Option<i64>,
    /// The store's own total, when it carries one.
    pub total: Option<i64>,
    /// A cache-miss field the store already reports cache-exclusive; it wins over `input`.
    pub exclusive_input: Option<i64>,
    /// Set when the store's `input` never includes the cache (the default is "may include").
    pub input_excludes_cache: bool,
}

/// Turns one product's usage keys into disjoint buckets.
///
/// **Inclusion is decided only by the store's own arithmetic.** When the total equals `input +
/// output` and not the sum of every named kind, the two are inclusive and the cache overlap is
/// removed from input. When the total equals the sum of every named kind, reasoning is a bucket of
/// its own. Otherwise the values are kept as reported.
///
/// **Reasoning is folded into output only when proven separate.** Otherwise the reported output
/// is kept whole and reasoning is not counted a second time.
///
/// **A bare total is not input.** When the object names no kind, the tally stays empty and the
/// total becomes `unclassified`; a total larger than the named kinds leaves the remainder
/// unclassified.
pub fn combine(reported: &Reported) -> UsageParts {
    let cache_read = reported.cache_read.unwrap_or(0);
    let cache_write = reported.cache_write.unwrap_or(0);
    let reasoning = reported.reasoning.unwrap_or(0);
    let names_a_kind = reported.input.is_some()
        || reported.output.is_some()
        || reported.cache_read.is_some()
        || reported.cache_write.is_some()
        || reported.reasoning.is_some()
        || reported.exclusive_input.is_some();

    if !names_a_kind {
        if let Some(total) = reported.total.filter(|t| *t > 0) {
            return UsageParts { tally: TokenTally::default(), unclassified: total };
        }
    }

    let reported_input = reported.input.unwrap_or(0);
    let raw_output = reported.output.unwrap_or(0);
    let base_input = reported.exclusive_input.unwrap_or(reported_input);
    let disjoint = base_input + raw_output + cache_read + cache_write + reasoning;

    let inclusive = reported.exclusive_input.is_none()
        && !reported.input_excludes_cache
        && reported.total.is_some_and(|total| total == reported_input + raw_output && total != disjoint);

    let fresh_input = if inclusive { (reported_input - cache_read - cache_write).max(0) } else { base_input };

    let unclassified = match reported.total {
        Some(total) if total > disjoint => total - disjoint,
        _ => 0,
    };

    // Reasoning is a separate kind only when the store's total counts it separately.
    let reasoning_is_separate = reported.total.is_some_and(|total| total == disjoint);
    let output = if reasoning_is_separate { raw_output + reasoning } else { raw_output };

    UsageParts {
        tally: TokenTally { input: fresh_input, cache_write, cache_read, output, ..TokenTally::default() },
        unclassified,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parts(reported: Reported) -> (TokenTally, i64) {
        let p = combine(&reported);
        (p.tally, p.unclassified)
    }

    #[test]
    fn a_total_decides_whether_the_cache_is_inside_input_and_reasoning_inside_output() {
        // Total = input + output: the input includes the cache, so it is taken out.
        let inclusive = Reported { input: Some(100), output: Some(10), cache_read: Some(30), total: Some(110), ..Default::default() };
        assert_eq!(parts(inclusive), (TokenTally::new(70, 0, 30, 10), 0));

        // Total = every kind: the four are disjoint and reasoning is its own bucket.
        let disjoint = Reported { input: Some(100), output: Some(10), cache_read: Some(30), reasoning: Some(5), total: Some(145), ..Default::default() };
        assert_eq!(parts(disjoint), (TokenTally::new(100, 0, 30, 15), 0));

        // A total that fits neither shape keeps the reported split and the remainder unclassified.
        let odd = Reported { input: Some(100), output: Some(10), total: Some(200), ..Default::default() };
        assert_eq!(parts(odd), (TokenTally::new(100, 0, 0, 10), 90));
    }

    #[test]
    fn a_bare_total_is_unclassified_never_input() {
        let bare = Reported { total: Some(500), ..Default::default() };
        assert_eq!(parts(bare), (TokenTally::default(), 500));
    }

    #[test]
    fn a_model_loses_its_gateway_prefix_and_a_naive_time_is_read_locally() {
        assert_eq!(model_id(Some("openai/gpt-5".into())).as_deref(), Some("gpt-5"));
        assert_eq!(model_id(Some("plain".into())).as_deref(), Some("plain"));
        assert_eq!(model_id(Some("trailing/".into())).as_deref(), Some("trailing/"));
        assert_eq!(model_id(None), None);

        let at = naive_timestamp("2026/01/02 03:04:05.678 [AgentReporter] usage: {}").unwrap();
        assert_eq!(at.with_timezone(&Local).format("%Y/%m/%d %H:%M:%S%.3f").to_string(), "2026/01/02 03:04:05.678");
        assert!(naive_timestamp("no time here").is_none());
    }

    #[test]
    fn a_brace_object_is_the_first_balanced_one_and_text_objects_must_be_whole() {
        let found = brace_object(r#"usage: {"a":1,"b":"x}y"} trailing {"c":2}"#).unwrap();
        assert_eq!(found["a"], json!(1));
        assert_eq!(found["b"], json!("x}y"));
        assert!(brace_object(r#"{"unfinished":1"#).is_none());
        assert!(json_object("[1,2]").is_none());
        assert_eq!(int(Some(&json!("7"))), 7);
        assert_eq!(int(Some(&json!(-3))), 0);
    }
}
