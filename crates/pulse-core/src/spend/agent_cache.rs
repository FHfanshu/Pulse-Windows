// Ported from upstream Sources/Pulse/Usage/AgentCache.swift.
//! One agent's finished ledger, kept between launches.
//!
//! The whole ledger rather than its inputs. The transcript reader caches per-file token counts and
//! prices them afresh each time, because a price change should not mean rescanning hundreds of
//! megabytes. The record sources (a database, a folder of logs) are read from several places
//! rather than file by file, so the cache keeps the finished ledger, and its validity is settled
//! by the store's real inputs and the price table, not by the root's own size and date.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::agent::SpendAgent;
use super::ledger::Ledger;
use super::prices::PriceTable;

/// The number in `agent-<n>-<agent>.json`. Part of the contract: also versions the readers'
/// semantics (a valid old shape can hold totals from old pricing or dedup rules), so bump it
/// whenever a reader's output changes for an unchanged store.
pub const VERSION: u32 = 2;

/// Whether the stores' real inputs, and the money behind their cost, are the same as when the
/// ledger was kept.
///
/// Not the store's own size and date. Half these stores are directories, and appending a line to
/// a log inside one leaves the directory's own stamp untouched. A database has the mirror-image
/// problem: SQLite writes the WAL and the `.db` file only moves at a checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    /// A digest of every file the ledger was read out of, across every root. For a directory it
    /// is a recursive walk by path, size and modification time; for a database it is the `.db`
    /// plus the `-wal` and `-journal` beside it.
    pub source: String,
    /// A digest of the price table. Money is part of what is kept, so a table that changed has to
    /// invalidate it, including the empty table an offline first run would otherwise freeze at
    /// $0.00 for ever, since an unchanged store would never be read again.
    pub prices: String,
}

impl Stamp {
    /// The stamp a set of roots and the price table produce now.
    pub fn of(inputs: &[PathBuf], prices: &PriceTable) -> Stamp {
        Stamp { source: source_fingerprint(inputs), prices: price_fingerprint(prices) }
    }
}

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn line(path: &Path, size: u64, modified: Option<std::time::SystemTime>) -> String {
    let seconds = modified.and_then(|m| m.duration_since(UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64());
    format!("{}\t{size}\t{seconds}", path.display())
}

/// A digest of every input across a store's roots.
///
/// The roots are ordered and deduplicated first, and each contributes its own identity line as
/// well as the fingerprint of its real inputs: two empty stores would digest alike otherwise, and
/// the set of roots is part of what the store is. Hidden files and folders are included (these
/// stores are often dot-directories); `-shm` is left out, because it is shared memory rather than
/// data and merely opening a database can touch it, which would make a read invalidate the cache
/// it was about to validate.
pub fn source_fingerprint(inputs: &[PathBuf]) -> String {
    let ordered: BTreeSet<&PathBuf> = inputs.iter().collect();
    let mut lines: Vec<String> = Vec::new();
    for root in ordered {
        lines.push(format!("root\t{}", root.display()));
        let Ok(metadata) = std::fs::metadata(root) else {
            lines.push(format!("missing\t{}", root.display()));
            continue;
        };
        if metadata.is_dir() {
            let mut pending = vec![root.clone()];
            while let Some(directory) = pending.pop() {
                let Ok(entries) = std::fs::read_dir(&directory) else { continue };
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Ok(kind) = entry.file_type() else { continue };
                    if kind.is_dir() {
                        pending.push(path);
                    } else if kind.is_file() && !path.to_string_lossy().ends_with("-shm") {
                        // The file's own metadata, not the directory entry's: NTFS refreshes an
                        // entry's size and time lazily.
                        if let Ok(meta) = std::fs::metadata(&path) {
                            lines.push(line(&path, meta.len(), meta.modified().ok()));
                        }
                    }
                }
            }
        } else {
            // The database itself, then the sidecars where SQLite keeps what it has not folded in
            // yet. The full path rather than the file's own name: two roots can each hold a file
            // called `opencode.db`, and they are not the same store.
            for suffix in ["", "-wal", "-journal"] {
                let path = PathBuf::from(format!("{}{suffix}", root.display()));
                if let Ok(meta) = std::fs::metadata(&path) {
                    lines.push(line(&path, meta.len(), meta.modified().ok()));
                }
            }
        }
    }
    // Sorted so the walk's order cannot make one unchanged store look like two different ones.
    lines.sort();
    digest(&lines.join("\n"))
}

/// A digest of the price table's rates and names. Content, not the file's date: a daily refresh
/// that fetched the same table must not throw away every agent's cached ledger, and a table with
/// even one changed rate, or a renamed model, must.
pub fn price_fingerprint(prices: &PriceTable) -> String {
    if prices.is_empty() {
        return "empty".to_string();
    }
    let mut keys: Vec<&String> = prices.keys().collect();
    keys.sort();
    let canonical: Vec<String> =
        keys.iter().map(|key| format!("{key}\t{}", serde_json::to_string(&prices[*key]).unwrap_or_default())).collect();
    digest(&canonical.join("\n"))
}

#[derive(Serialize, Deserialize)]
struct Saved {
    version: u32,
    stamp: Stamp,
    ledger: Ledger,
}

pub fn file_name(agent: SpendAgent) -> String {
    format!("agent-{VERSION}-{}.json", agent.raw())
}

/// The kept ledger and the stamp it was read under, or None when there is none or it is of
/// another version.
pub fn load(agent: SpendAgent, directory: &Path) -> Option<(Stamp, Ledger)> {
    let bytes = std::fs::read(directory.join(file_name(agent))).ok()?;
    let saved: Saved = serde_json::from_slice(&bytes).ok()?;
    (saved.version == VERSION).then_some((saved.stamp, saved.ledger))
}

pub fn save(agent: SpendAgent, directory: &Path, stamp: &Stamp, ledger: &Ledger) {
    if std::fs::create_dir_all(directory).is_err() {
        return;
    }
    let saved = Saved { version: VERSION, stamp: stamp.clone(), ledger: ledger.clone() };
    if let Ok(bytes) = serde_json::to_vec(&saved) {
        super::prices::write_atomically(&directory.join(file_name(agent)), &bytes);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::super::prices::ModelPrice;
    use super::*;

    #[test]
    fn a_log_appended_inside_a_folder_moves_the_fingerprint_and_the_shm_index_does_not() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("sessions");
        std::fs::create_dir_all(store.join("a")).unwrap();
        std::fs::write(store.join("a").join("wire.jsonl"), "{}\n").unwrap();
        let before = source_fingerprint(std::slice::from_ref(&store));
        assert_eq!(before, source_fingerprint(&[store.clone(), store.clone()]), "a duplicate root is one root");

        std::fs::write(store.join("a").join("x.db-shm"), "shared memory").unwrap();
        assert_eq!(before, source_fingerprint(std::slice::from_ref(&store)));

        std::fs::write(store.join("a").join("wire.jsonl"), "{}\n{}\n").unwrap();
        assert_ne!(before, source_fingerprint(std::slice::from_ref(&store)));
    }

    #[test]
    fn a_database_is_stamped_with_its_wal_and_a_missing_root_is_its_own_line() {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("opencode.db");
        std::fs::write(&database, "db").unwrap();
        let before = source_fingerprint(std::slice::from_ref(&database));
        std::fs::write(root.path().join("opencode.db-wal"), "pending writes").unwrap();
        assert_ne!(before, source_fingerprint(std::slice::from_ref(&database)));

        let absent = root.path().join("absent.db");
        assert_ne!(source_fingerprint(&[absent.clone()]), source_fingerprint(&[]));
        assert_ne!(source_fingerprint(&[absent.clone()]), source_fingerprint(&[root.path().join("other.db")]));
    }

    #[test]
    fn the_price_stamp_follows_content_not_order_or_date() {
        let rate = |input| ModelPrice::new(input, 2.0, None, None, Some("Name"));
        let one: PriceTable = HashMap::from([("a".to_string(), rate(1.0)), ("b".to_string(), rate(1.0))]);
        let same: PriceTable = HashMap::from([("b".to_string(), rate(1.0)), ("a".to_string(), rate(1.0))]);
        let changed: PriceTable = HashMap::from([("a".to_string(), rate(1.5)), ("b".to_string(), rate(1.0))]);
        assert_eq!(price_fingerprint(&one), price_fingerprint(&same));
        assert_ne!(price_fingerprint(&one), price_fingerprint(&changed));
        assert_eq!(price_fingerprint(&PriceTable::new()), "empty");
    }

    #[test]
    fn a_saved_ledger_reads_back_exactly_and_another_version_is_ignored() {
        let root = tempfile::tempdir().unwrap();
        let stamp = Stamp { source: "s".into(), prices: "p".into() };
        let ledger = Ledger::empty();
        save(SpendAgent::OpenCode, root.path(), &stamp, &ledger);
        let (read_stamp, read_ledger) = load(SpendAgent::OpenCode, root.path()).unwrap();
        assert_eq!((read_stamp, read_ledger), (stamp, ledger));
        assert!(load(SpendAgent::Grok, root.path()).is_none());

        std::fs::write(root.path().join(file_name(SpendAgent::Grok)), r#"{"version":0,"stamp":{"source":"","prices":""},"ledger":{"days":[],"earliest":null,"unpricedModels":[],"modelNames":{},"slots":[]}}"#)
            .unwrap();
        assert!(load(SpendAgent::Grok, root.path()).is_none());
    }
}
