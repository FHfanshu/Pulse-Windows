// Ported from upstream Sources/Pulse/Usage/Readers/PrimeAgentSessionReader.swift
// and Sources/Pulse/Usage/Readers/SessionLogPaths.swift (prime-agent).
//! Prime Agent: the Pi transcript format, plus a parent/child accounting problem.
//!
//! `sessions` and `session-artifacts` under `~/.prime/agent`, read with the shared Pi parser in
//! [`super::pi`]. A parent assistant message can persist a **cumulative** `aggregateUsage` that
//! already includes a child invocation, and a `child_usage_attributed` record states which child
//! and how much. The child transcript is also scanned as its own usage, so adding the parent's
//! aggregate unchanged would count the child twice. The fix is not a guess: the parent message
//! whose own tally **equals** the stated aggregate is the one reduced, by exactly the stated child
//! usage, clamped at zero, and each child is consumed once.
//!
//! **When the child is not present, the parent keeps its aggregate.** Subtracting a child Pulse
//! cannot find would silently drop tokens nobody else accounts for.
//!
//! Attribution ids are unique only inside one session, and a fork copies records into a new file,
//! so an attribution is keyed by its id and the resolved fork-lineage root. A `parentSession`
//! cycle resolves to the lexicographically smallest path in the cycle.
//!
//! Where the store lives: `PRIME_AGENT_SESSION_DIR` (or `PRIME_AGENT_CODING_AGENT_SESSION_DIR`),
//! `PRIME_AGENT_CODING_AGENT_DIR`, else `%USERPROFILE%\.prime\agent`. A `sessionDir` in a
//! `settings.json` (the agent folder's, and each project's `.prime\agent\settings.json`) redirects
//! the sessions folder. Windows locations are unverified on a real PC.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::pi::{cwd_values, key, parse, push_unique_path, File, Message};
use super::SpendSource;
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::tally::TokenTally;
use crate::spend::transcripts::Sources;

pub struct PrimeAgent;

impl SpendSource for PrimeAgent {
    fn raw(&self) -> &'static str {
        "primeAgent"
    }
    fn source_id(&self) -> &'static str {
        "prime-agent"
    }
    fn display_name(&self) -> &'static str {
        "Prime Agent"
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["PRIME_AGENT_SESSION_DIR", "PRIME_AGENT_CODING_AGENT_SESSION_DIR", "PRIME_AGENT_CODING_AGENT_DIR"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let home = &sources.home;
        let mut roots: Vec<PathBuf> = if let Some(dir) =
            sources.var("PRIME_AGENT_SESSION_DIR").or_else(|| sources.var("PRIME_AGENT_CODING_AGENT_SESSION_DIR"))
        {
            vec![dir.clone(), parent_of(&dir).join("session-artifacts")]
        } else if let Some(dir) = sources.var("PRIME_AGENT_CODING_AGENT_DIR") {
            vec![dir.join("sessions"), dir.join("session-artifacts")]
        } else {
            // WINDOWS-PATH: unverified (`%USERPROFILE%\.prime` is the macOS layout carried over)
            let base = home.join(".prime").join("agent");
            vec![base.join("sessions"), base.join("session-artifacts")]
        };

        if let Some(first) = roots.first().cloned() {
            if let Some(redirected) = settings_session_dir(&parent_of(&first).join("settings.json"), home) {
                roots.extend(redirected);
            }
        }
        for cwd in cwd_values(&roots) {
            let project = PathBuf::from(cwd).join(".prime").join("agent").join("settings.json");
            if let Some(redirected) = settings_session_dir(&project, home) {
                roots.extend(redirected);
            }
        }

        let mut unique = Vec::new();
        for root in roots {
            push_unique_path(&mut unique, root);
        }
        unique
    }
    fn records(&self, roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        records(roots)
    }
}

fn parent_of(path: &Path) -> PathBuf {
    path.parent().map(Path::to_path_buf).unwrap_or_else(|| path.to_path_buf())
}

/// A stated path, with `~` and `~/...` resolved against the home folder. Empty is absent.
fn stated_path(raw: &str, home: &Path) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "~" {
        return Some(home.to_path_buf());
    }
    if let Some(rest) = trimmed.strip_prefix("~/").or_else(|| trimmed.strip_prefix("~\\")) {
        return Some(home.join(rest));
    }
    Some(PathBuf::from(trimmed))
}

/// A settings file's `sessionDir`, resolved to a sessions folder and its sibling artifacts folder.
///
/// `null` (or no key) means the default; an empty string means the settings file's own folder.
/// A missing or unreadable file is the default too: nothing is redirected on a guess.
fn settings_session_dir(settings: &Path, home: &Path) -> Option<Vec<PathBuf>> {
    let object = logio::json(settings)?;
    let text = match object.get("sessionDir")? {
        Value::String(text) => text,
        _ => return None,
    };
    let sessions = if text.trim().is_empty() {
        parent_of(settings)
    } else {
        stated_path(text, home)?
    };
    Some(vec![sessions.clone(), parent_of(&sessions).join("session-artifacts")])
}

/// The records of a Prime store, with the parent/child reconciliation applied.
fn records(roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
    let files: Vec<File> = logio::files(roots, &["jsonl"], &[], &[]).iter().map(|p| parse(p)).collect();

    let mut parent_of_file: HashMap<String, String> = HashMap::new();
    for file in &files {
        if let Some(parent) = &file.header.parent_session {
            parent_of_file.insert(file.path.clone(), key(Path::new(parent)));
        }
    }

    // The top of a fork lineage, collapsing a cycle to its smallest path.
    let lineage_root = |start: &str| -> String {
        let mut chain: Vec<String> = Vec::new();
        let mut current = start.to_string();
        loop {
            if let Some(index) = chain.iter().position(|c| *c == current) {
                return chain[index..].iter().min().cloned().unwrap_or(current);
            }
            chain.push(current.clone());
            match parent_of_file.get(&current) {
                Some(parent) => current = parent.clone(),
                None => return current,
            }
        }
    };

    let is_descendant = |child: &str, ancestor: &str| -> bool {
        let mut current = Some(child.to_string());
        let mut visited: HashSet<String> = HashSet::new();
        while let Some(path) = current {
            if path == ancestor {
                return true;
            }
            if !visited.insert(path.clone()) {
                return false;
            }
            current = parent_of_file.get(&path).cloned();
        }
        false
    };

    // Every child transcript's total, for matching a stated child usage. A fork's `parentSession`
    // names the file it copied (not a child); only a positive `rlmDepth` is a child.
    let child_totals: Vec<(String, TokenTally)> = files
        .iter()
        .filter(|f| f.header.rlm_depth.unwrap_or(0) > 0)
        .map(|f| (f.path.clone(), f.messages.iter().fold(TokenTally::default(), |sum, m| sum + m.tally.clone())))
        .collect();

    let mut consumed_children: HashSet<String> = HashSet::new();
    let mut seen_attributions: HashSet<String> = HashSet::new();
    // Keyed by the parent message's stable identity, so a fork copy of the same message is reduced identically.
    let mut reductions: HashMap<String, TokenTally> = HashMap::new();

    for file in &files {
        for attribution in &file.attributions {
            let (Some(id), Some(target)) = (&attribution.id, &attribution.target_id) else { continue };
            let lineage = lineage_root(&file.path);
            let attribution_key = format!("{lineage}#{id}");
            if !seen_attributions.insert(attribution_key) {
                continue;
            }
            if attribution.aggregate_usage.total() <= 0 || attribution.child_usage.total() <= 0 {
                continue;
            }
            let Some(message) = file
                .messages
                .iter()
                .find(|m| m.id.as_deref() == Some(target.as_str()) && m.tally == attribution.aggregate_usage)
            else {
                continue;
            };
            let Some(index) = child_totals.iter().position(|(path, tally)| {
                !consumed_children.contains(path) && *tally == attribution.child_usage && is_descendant(path, &lineage)
            }) else {
                continue;
            };
            consumed_children.insert(child_totals[index].0.clone());

            let entry = reductions.entry(deduplication_id(message)).or_default();
            *entry = entry.clone() + attribution.child_usage.clone();
        }
    }

    let mut records = Vec::new();
    let mut incomplete = false;
    for file in &files {
        let session_id = if file.is_valid { file.header.id.clone() } else { None };
        for message in &file.messages {
            let mut tally = message.tally.clone();
            if let Some(subtract) = reductions.get(&deduplication_id(message)) {
                tally = TokenTally::new(
                    (tally.input - subtract.input).max(0),
                    (tally.cache_write - subtract.cache_write).max(0),
                    (tally.cache_read - subtract.cache_read).max(0),
                    (tally.output - subtract.output).max(0),
                );
            }
            if tally.total() <= 0 && message.unclassified <= 0 {
                continue;
            }
            let (Some(session_id), Some(timestamp), Some(model)) = (session_id.as_ref(), message.timestamp, message.model.as_ref()) else {
                incomplete = true;
                continue;
            };
            let mut record = AgentUsageRecord::new(timestamp, model, tally).session(session_id).unclassified(message.unclassified);
            record.project = file.header.cwd.clone();
            record.deduplication_id = Some(deduplication_id(message));
            records.push(record);
        }
    }
    if incomplete {
        for record in &mut records {
            record.is_partial = true;
        }
    }
    records
}

/// A fork copy must fold onto its original, so the key is session-independent.
fn deduplication_id(message: &Message) -> String {
    if let Some(response) = &message.response_id {
        return format!("prime-agent:response:{response}");
    }
    let milliseconds = message.timestamp.map(|t| (t.timestamp_micros() as f64 / 1000.0).round() as i64).unwrap_or(0);
    let tally = &message.tally;
    format!(
        "prime-agent:message:{}:{milliseconds}:{}:{}:{}:{}:{}:{}",
        message.id.clone().unwrap_or_default(),
        message.provider.clone().unwrap_or_default(),
        message.model.clone().unwrap_or_default(),
        tally.input,
        tally.output,
        tally.cache_read,
        tally.cache_write,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use crate::spend::calendar::Calendar;
    use crate::spend::prices::{ModelPrice, PriceTable};
    use crate::spend::sources::pi::tests::write;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::transcripts::Sources;

    fn prices() -> PriceTable {
        HashMap::from([("gpt-5".to_string(), ModelPrice::new(1.0, 10.0, Some(0.1), Some(1.0), Some("GPT-5")))])
    }

    fn total(sources: &Sources) -> i64 {
        let cache = tempfile::tempdir().unwrap();
        read_agent(SpendAgent::PrimeAgent, sources, cache.path(), &Calendar::utc(2), &prices()).days.iter().map(|d| d.tokens).sum()
    }

    /// A parent whose aggregate (150) includes a child of 50, and the child's own transcript.
    fn parent_and_child(root: &Path, child_present: bool) {
        let parent = root.join("sessions/parent.jsonl");
        // Forward slashes keep the Windows path valid inside the JSON string.
        let parent_path = parent.to_string_lossy().replace('\\', "/");
        write(
            &parent,
            &format!(
                "{{\"type\":\"session\",\"id\":\"p\",\"timestamp\":\"2026-01-02T03:05:00Z\",\"cwd\":\"/w\"}}\n\
                 {{\"type\":\"message\",\"id\":\"m1\",\"timestamp\":\"2026-01-02T03:05:00Z\",\"message\":{{\"role\":\"assistant\",\"model\":\"gpt-5\",\"usage\":{{\"input\":150,\"output\":0}}}}}}\n\
                 {{\"type\":\"child_usage_attributed\",\"id\":\"a1\",\"targetId\":\"m1\",\"childUsage\":{{\"input\":50,\"output\":0}},\"aggregateUsage\":{{\"input\":150,\"output\":0}}}}"
            ),
        );
        if child_present {
            write(
                &root.join("sessions/child.jsonl"),
                &format!(
                    "{{\"type\":\"session\",\"id\":\"c\",\"timestamp\":\"2026-01-02T03:05:00Z\",\"cwd\":\"/w\",\"parentSession\":\"{parent_path}\",\"rlmDepth\":1}}\n\
                     {{\"type\":\"message\",\"id\":\"c1\",\"timestamp\":\"2026-01-02T03:05:01Z\",\"message\":{{\"role\":\"assistant\",\"model\":\"gpt-5\",\"usage\":{{\"input\":50,\"output\":0}}}}}}"
                ),
            );
        }
    }

    #[test]
    fn a_parent_aggregate_is_reduced_by_the_child_once_and_the_child_is_not_counted_twice() {
        let home = tempfile::tempdir().unwrap();
        parent_and_child(&home.path().join(".prime/agent"), true);
        // 150 - 50 for the parent, plus the child's own 50.
        assert_eq!(total(&Sources::new(home.path())), 150);
    }

    #[test]
    fn an_absent_child_leaves_the_parent_aggregate_alone() {
        let home = tempfile::tempdir().unwrap();
        parent_and_child(&home.path().join(".prime/agent"), false);
        assert_eq!(total(&Sources::new(home.path())), 150);
    }

    #[test]
    fn a_settings_session_dir_redirects_the_store() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        write(
            &home.path().join(".prime/agent/settings.json"),
            &format!("{{\"sessionDir\":\"{}\"}}", elsewhere.path().to_string_lossy().replace('\\', "/")),
        );
        write(
            &elsewhere.path().join("a.jsonl"),
            "{\"type\":\"session\",\"id\":\"s\",\"timestamp\":\"2026-01-02T03:05:00Z\",\"cwd\":\"/w\"}\n\
             {\"type\":\"message\",\"id\":\"m\",\"timestamp\":\"2026-01-02T03:05:00Z\",\"message\":{\"role\":\"assistant\",\"model\":\"gpt-5\",\"usage\":{\"input\":9}}}",
        );
        assert_eq!(total(&Sources::new(home.path())), 9);
    }
}
