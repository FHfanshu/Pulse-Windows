// Ported from upstream Sources/Pulse/Usage/UsageLedger.swift (title, text and link helpers).
//! What a conversation is called, taken from the words it opened with: one line, links
//! unwrapped, envelopes refused.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// The opening of the message Codex puts first in a session it runs to review another's
/// planned action.
pub const CODEX_REVIEW_OPENING: &str = "The following is the Codex agent history";

/// The longest title, in characters.
const TITLE_LIMIT: usize = 70;

static LINK_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"!?\[([^\]]+)\]\([^)]*\)", "$1"),
        (r"!?\[\]\(([^)\s]+)[^)]*\)", "$1"),
        (r"\[(https?://[^\]\s]+)\]", "$1"),
        (r"<(https?://[^>\s]+)>", "$1"),
    ]
    .into_iter()
    .map(|(pattern, template)| (Regex::new(pattern).expect("link pattern"), template))
    .collect()
});

static LINK_ONLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https?://\S+$").expect("link-only pattern"));
static SCHEME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https?://").expect("scheme pattern"));

/// A line break in the sense of `Character.isNewline`.
fn is_newline(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// The opening prompt, cut to something a row can hold.
///
/// Not the whole message: these are the user's own words and a row is one line; the point is to
/// tell one conversation from another, which the first few words do. Links are unwrapped, not
/// shown as markup: `[text](url)` reads as its text and `[url]` or `<url>` as the url. A message
/// that opens with a line that is only a link and carries words below it is named for the first
/// of those lines; a message that is only a link reads as `host/first-segment/...`.
pub fn title_from(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // A pasted file or a command envelope is not a title. A `<url>` is markdown's way to write a
    // link, not an envelope.
    if (trimmed.starts_with('<') && !trimmed.starts_with("<http")) || trimmed.starts_with("Caveat:") {
        return None;
    }

    let lines: Vec<String> = trimmed
        .split(is_newline)
        .map(|line| collapsing_whitespace(&unwrapping_links(line)))
        .filter(|line| !line.is_empty())
        .collect();
    let first = lines.first()?;

    let cleaned = if is_link_only(first) {
        // The first line of words below the link, else the link itself.
        lines.iter().find(|line| !is_link_only(line)).cloned().unwrap_or_else(|| compact_link(first))
    } else {
        lines.join(" ")
    };
    if cleaned.is_empty() {
        return None;
    }
    if cleaned.chars().count() <= TITLE_LIMIT {
        Some(cleaned)
    } else {
        let mut cut: String = cleaned.chars().take(TITLE_LIMIT - 1).collect();
        cut.push('…');
        Some(cut)
    }
}

fn unwrapping_links(line: &str) -> String {
    let mut result = line.to_string();
    for (pattern, template) in LINK_PATTERNS.iter() {
        result = pattern.replace_all(&result, *template).into_owned();
    }
    result
}

fn collapsing_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_link_only(line: &str) -> bool {
    LINK_ONLY.is_match(line)
}

/// `host/first-path-segment/...` for a link: no scheme, no brackets, no query. The ellipsis
/// marks that there is more path after the segment. A link that cannot be read is shown without
/// its scheme.
pub fn compact_link(link: &str) -> String {
    let stripped = || SCHEME.replace(link, "").into_owned();
    let Some((_, rest)) = link.split_once("://") else { return stripped() };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = if host_port.starts_with('[') {
        host_port.split_once(']').map_or(host_port, |(h, _)| h.trim_start_matches('['))
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };
    if host.is_empty() {
        return stripped();
    }
    let path = rest[authority_end..].split(['?', '#']).next().unwrap_or("");
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match segments.first() {
        None => host.to_string(),
        Some(first) => format!("{host}/{first}{}", if segments.len() > 1 { "/…" } else { "" }),
    }
}

/// The first run of text in a message body, which is a string in the simple case and an array
/// of typed parts in the rich one.
pub fn text_in(message: Option<&Value>) -> Option<String> {
    match message? {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .find(|text| !text.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

/// Whether a Codex user-role message is a review request rather than the user's words.
pub fn is_codex_review(content: Option<&Value>) -> bool {
    text_in(content).is_some_and(|text| text.trim().starts_with(CODEX_REVIEW_OPENING))
}

/// What a Codex user-role message says in the user's own words, as a title, or None when the
/// message is context Codex put there itself.
///
/// Codex writes more user-role messages than the user does. A rollout opens with `# AGENTS.md
/// instructions for ...` and an `<environment_context>` (or `<recommended_plugins>`), and the
/// app adds `<turn_aborted>`, `<subagent_notification>` and `<image>` envelopes later; the IDE
/// extension puts `# Context from my IDE setup:` before the request, which follows
/// `## My request for Codex:`. A session Codex runs to review another's planned action opens
/// with a message that says so. Envelopes start with `<`, which `title_from` already refuses.
pub fn codex_title(content: Option<&Value>) -> Option<String> {
    let text = text_in(content)?;
    let mut opening = text.trim();
    if opening.starts_with("# AGENTS.md instructions") || opening.starts_with(CODEX_REVIEW_OPENING) {
        return None;
    }
    if opening.starts_with("# Context from my IDE setup") || opening.starts_with("# Files mentioned by the user") {
        let marker = "## My request for Codex:";
        let at = opening.find(marker)?;
        opening = &opening[at + marker.len()..];
    }
    title_from(opening)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_title_is_one_line_of_the_opening_prompt() {
        assert_eq!(title_from("  Fix the ring  ").as_deref(), Some("Fix the ring"));
        assert_eq!(title_from("Fix the ring\nand the card").as_deref(), Some("Fix the ring and the card"));
        let long = "长".repeat(200);
        let cut = title_from(&long).unwrap();
        assert_eq!(cut.chars().count(), 70);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn an_envelope_is_not_a_title() {
        assert_eq!(title_from("<command-name>/init</command-name>"), None);
        assert_eq!(title_from("Caveat: The messages below were generated…"), None);
        assert_eq!(title_from("   "), None);
        assert_eq!(title_from(""), None);
    }

    #[test]
    fn a_message_body_is_a_string_or_a_list_of_typed_parts() {
        assert_eq!(text_in(Some(&json!("plain"))).as_deref(), Some("plain"));
        assert_eq!(text_in(Some(&json!([{"type": "text", "text": "rich"}]))).as_deref(), Some("rich"));
        // A tool result carries no text of its own and must not stop the search at the first part.
        assert_eq!(
            text_in(Some(&json!([{"type": "tool_result"}, {"type": "text", "text": "after"}]))).as_deref(),
            Some("after")
        );
        assert_eq!(text_in(Some(&json!([{"type": "tool_result"}]))), None);
        assert_eq!(text_in(None), None);
    }

    #[test]
    fn links_are_cleaned_out_of_a_title() {
        assert_eq!(title_from("see [the docs](https://example.com/a) now").as_deref(), Some("see the docs now"));
        assert_eq!(title_from("open [https://example.com/x] please").as_deref(), Some("open https://example.com/x please"));
        assert_eq!(title_from("open <https://example.com/x> please").as_deref(), Some("open https://example.com/x please"));
        // A first line that is only a link is named for the first line below it that is not one.
        assert_eq!(title_from("https://gist.github.com/me/abc\nreview this script").as_deref(), Some("review this script"));
        // A message that is only a link reads as host/first-segment/...
        assert_eq!(title_from("https://github.com/qunqin24/Pulse/pull/1?x=2").as_deref(), Some("github.com/qunqin24/…"));
        assert_eq!(title_from("https://example.com").as_deref(), Some("example.com"));
        assert_eq!(title_from("https://example.com/one").as_deref(), Some("example.com/one"));
    }

    #[test]
    fn codex_context_is_not_a_title() {
        let message = |text: &str| json!([{"type": "input_text", "text": text}]);
        assert_eq!(codex_title(Some(&message("# AGENTS.md instructions for /work\n..."))), None);
        assert_eq!(codex_title(Some(&message("<environment_context>"))), None);
        assert_eq!(codex_title(Some(&message("The following is the Codex agent history of a session"))), None);
        assert_eq!(
            codex_title(Some(&message("# Context from my IDE setup:\n- file\n## My request for Codex:\nFix it"))).as_deref(),
            Some("Fix it")
        );
        assert_eq!(codex_title(Some(&message("# Context from my IDE setup:\n- file"))), None);
        assert_eq!(codex_title(Some(&message("plain request"))).as_deref(), Some("plain request"));
        assert!(is_codex_review(Some(&message("  The following is the Codex agent history ..."))));
    }
}
