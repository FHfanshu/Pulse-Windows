// Ported from upstream Sources/Pulse/Usage/TranscriptArchive.swift.
//! What Pulse keeps of a Claude Code or Codex transcript after the CLI has deleted it.
//!
//! **The transcripts are the only record, and the CLIs prune them.** Claude Code deletes
//! sessions older than `cleanupPeriodDays`; Codex users clear `~/.codex/sessions`. The reader
//! used to drop a file's cache entry the scan it went missing, and its days left Token spend and
//! the recaps with it: a month's total could shrink a month later.
//!
//! **One entry per transcript, kept the scan it disappears**, holding what that file *counted*
//! in that scan: its quarter-hours by model after the cross-file dedupe, its reply timings, and
//! what the session row needs (title, directory, review mark). Nothing is kept for a file Pulse
//! never scanned while it existed.
//!
//! **A kept file still claims its replies.** A resumed or forked conversation opens with a copy
//! of the old one's history. The kept file counted those replies, so the ids it counted (and,
//! for Codex, the running totals a fork replays) are kept beside its figures and are claimed
//! before any live file is read. **Claims are never let go**: a file holding the copies can be
//! away for a scan (a moved folder, a drive not mounted) or come back from a backup, and a claim
//! dropped meanwhile would count its copies again for good. They are kept as 64-bit digests
//! (`claim_digest`), eight bytes a reply.
//!
//! **Its own file, unnumbered** (`archive-ledger-<provider>.json`). The `ledger-<n>-*` cache is
//! thrown away whenever its number goes up, and can be, because it is rebuilt from the
//! transcripts; this cannot be rebuilt from anything, so it must never be superseded by
//! renaming. A change of shape bumps `format` and must read the older one, not discard it.

use std::collections::HashMap;
use std::path::Path;

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::ledger::Buckets;
use super::transcripts::Timings;
use crate::provider::Provider;

/// The shape this build writes and reads. A file written in a later shape is left untouched.
pub const CURRENT_FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptArchive {
    #[serde(default = "current_format")]
    pub format: u32,
    /// By the path the transcript had.
    #[serde(default)]
    pub files: HashMap<String, Kept>,
}

fn current_format() -> u32 {
    CURRENT_FORMAT
}

impl Default for TranscriptArchive {
    fn default() -> Self {
        Self { format: CURRENT_FORMAT, files: HashMap::new() }
    }
}

/// Every field optional on the way in, so a field added later does not make the whole archive
/// unreadable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kept {
    /// What the file counted, by quarter-hour key and then raw model id.
    #[serde(default)]
    pub days: Buckets,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timings: Option<Timings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_review: Option<bool>,
    /// Digests of the reply ids this file counted (`pack`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claims: Option<String>,
    /// Digests of the Codex running totals this file reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totals: Option<String>,
    /// When the file was found gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<DateTime<Utc>>,
}

impl TranscriptArchive {
    pub fn file_name(provider: Provider) -> String {
        format!("archive-ledger-{}.json", provider.raw())
    }

    /// The archive, an empty one when there is none yet, or **None when a file is there and
    /// cannot be read** (damaged, or written by a later build). None means nothing may be moved
    /// into it and it must not be overwritten.
    pub fn load(provider: Provider, directory: &Path) -> Option<TranscriptArchive> {
        let path = directory.join(Self::file_name(provider));
        if !path.exists() {
            return Some(TranscriptArchive::default());
        }
        let archive: TranscriptArchive = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        (archive.format <= CURRENT_FORMAT).then_some(archive)
    }

    /// Whether the archive is on disk. A caller drops the cache entries it just kept only when
    /// this is true.
    pub fn save(&self, provider: Provider, directory: &Path) -> bool {
        let Ok(bytes) = serde_json::to_vec(self) else { return false };
        write_durably(&directory.join(Self::file_name(provider)), &bytes)
    }
}

/// Writes through a temporary file and a rename, and says whether the file is in place.
pub(crate) fn write_durably(path: &Path, bytes: &[u8]) -> bool {
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    if std::fs::write(&temp, bytes).is_err() {
        return false;
    }
    if std::fs::rename(&temp, path).is_err() {
        let _ = std::fs::remove_file(&temp);
        return false;
    }
    true
}

/// A stable 64-bit digest of a claim (FNV-1a over its UTF-8). Rust's own hasher is seeded per
/// process, so it cannot be written down.
pub fn claim_digest(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Eight little-endian bytes each, in base 64. None for none.
pub fn pack(digests: &[u64]) -> Option<String> {
    if digests.is_empty() {
        return None;
    }
    let bytes: Vec<u8> = digests.iter().flat_map(|d| d.to_le_bytes()).collect();
    Some(base64::engine::general_purpose::STANDARD.encode(bytes))
}

pub fn unpack(packed: Option<&str>) -> Vec<u64> {
    let Some(bytes) = packed.and_then(|p| base64::engine::general_purpose::STANDARD.decode(p).ok()) else {
        return Vec::new();
    };
    bytes.chunks_exact(8).map(|chunk| u64::from_le_bytes(chunk.try_into().expect("eight bytes"))).collect()
}

/// Whether a path lies inside one of the folders, as written or as the link it may be resolves
/// (a `~/.claude` linked elsewhere is walked at its target).
pub fn is_inside(path: &str, roots: &[std::path::PathBuf]) -> bool {
    let path = Path::new(path);
    roots.iter().any(|root| {
        path.starts_with(root) || std::fs::canonicalize(root).is_ok_and(|real| path.starts_with(real))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_are_fnv1a_and_pack_round_trips() {
        // FNV-1a 64 of "a", a published test vector.
        assert_eq!(claim_digest("a"), 0xaf63_dc4c_8601_ec8c);
        let digests = vec![1, u64::MAX, claim_digest("msg_01")];
        assert_eq!(unpack(pack(&digests).as_deref()), digests);
        assert_eq!(pack(&[]), None);
        assert!(unpack(None).is_empty());
        assert!(unpack(Some("not base64!")).is_empty());
    }

    #[test]
    fn an_unreadable_or_later_archive_loads_as_none_and_a_missing_one_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(TranscriptArchive::load(Provider::ClaudeCode, dir.path()), Some(TranscriptArchive::default()));
        let path = dir.path().join(TranscriptArchive::file_name(Provider::ClaudeCode));
        std::fs::write(&path, b"{ damaged").unwrap();
        assert_eq!(TranscriptArchive::load(Provider::ClaudeCode, dir.path()), None);
        std::fs::write(&path, br#"{"format":2,"files":{}}"#).unwrap();
        assert_eq!(TranscriptArchive::load(Provider::ClaudeCode, dir.path()), None);
        // An older file missing fields still reads.
        std::fs::write(&path, br#"{"files":{"a":{"days":{}}}}"#).unwrap();
        assert_eq!(TranscriptArchive::load(Provider::ClaudeCode, dir.path()).unwrap().files.len(), 1);
    }
}
