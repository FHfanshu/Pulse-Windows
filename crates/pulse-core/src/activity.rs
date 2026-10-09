// Simplified from upstream Usage/AgentActivity.swift: only the "last write" half, which is what
// the adaptive refresh reads (`signals.lastAgentActivity = activity.lastWrite`).
//! When a local agent last wrote to its transcripts. Claude Code keeps them under
//! `~/.claude/projects/<project>/*.jsonl`, Codex under `~/.codex/sessions/YYYY/MM/DD/*.jsonl`.
//! Only modification times are read, never contents.

use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};

fn newest_in(dir: &Path, depth: usize, newest: &mut Option<SystemTime>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            if depth > 0 {
                newest_in(&entry.path(), depth - 1, newest);
            }
        } else if entry.path().extension().is_some_and(|e| e == "jsonl") {
            if let Ok(modified) = meta.modified() {
                if newest.is_none_or(|n| modified > n) {
                    *newest = Some(modified);
                }
            }
        }
    }
}

/// The newest transcript write by Claude Code or Codex.
pub fn last_agent_write(home: &Path, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let mut newest: Option<SystemTime> = None;
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR").map(Into::into).unwrap_or_else(|| home.join(".claude"));
    newest_in(&claude.join("projects"), 2, &mut newest);
    // Codex files by day: today's and yesterday's folders are the only ones being written.
    let sessions = std::env::var_os("CODEX_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".codex")).join("sessions");
    for day in [now, now - Duration::days(1)] {
        let local = day.with_timezone(&chrono::Local);
        newest_in(&sessions.join(local.format("%Y/%m/%d").to_string()), 0, &mut newest);
    }
    newest.map(DateTime::<Utc>::from)
}

/// Modification time of a file, for noticing that a local source changed.
pub fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_newest_transcript() {
        let dir = std::env::temp_dir().join(format!("pulse-activity-{}", std::process::id()));
        let project = dir.join(".claude/projects/p");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("a.jsonl"), "{}").unwrap();
        std::fs::write(project.join("notes.txt"), "x").unwrap();
        let found = last_agent_write(&dir, Utc::now());
        assert!(found.is_some_and(|t| (Utc::now() - t).num_seconds() < 60));
        assert!(last_agent_write(&dir.join("nowhere"), Utc::now()).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
