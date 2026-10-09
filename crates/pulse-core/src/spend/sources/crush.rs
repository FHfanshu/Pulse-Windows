// Ported from upstream Sources/Pulse/Usage/Readers/CrushReader.swift.
//! Crush's project registry and the per-project databases it references.
//!
//! Crush keeps a `projects.json` registry whose `projects[]` entries name a working `path` and a
//! `data_dir`. The database is `<data_dir>/crush.db` when `data_dir` is absolute, else
//! `<path>/<data_dir>/crush.db`.
//!
//! **There are no token records here, by design.** The only figure Crush reports is a session
//! `cost` in dollars, and a cost is not a token count: turning it back into tokens would invent a
//! count nobody measured. `records` is therefore empty. The inputs are still declared, so the stores
//! are watched and the source is recognised rather than silently absent.
//!
//! Where it lives on Windows: `$CRUSH_GLOBAL_DATA`, `$XDG_DATA_HOME\crush`, `%LOCALAPPDATA%\crush`
//! and `%USERPROFILE%\AppData\Local\crush` are all looked at. Unverified on a PC.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{push_unique, SpendSource};
use crate::spend::logio;
use crate::spend::record::AgentUsageRecord;
use crate::spend::transcripts::Sources;

pub struct Crush;

impl SpendSource for Crush {
    fn raw(&self) -> &'static str {
        "crush"
    }
    fn source_id(&self) -> &'static str {
        "crush"
    }
    fn display_name(&self) -> &'static str {
        "Crush"
    }
    fn env_vars(&self) -> &'static [&'static str] {
        &["CRUSH_GLOBAL_DATA", "XDG_DATA_HOME", "LOCALAPPDATA"]
    }
    fn inputs(&self, sources: &Sources) -> Vec<PathBuf> {
        let mut registries = Vec::new();
        // WINDOWS-PATH: unverified (the global data override, when the variable is set)
        if let Some(global) = sources.var("CRUSH_GLOBAL_DATA") {
            registries.push(global.join("projects.json"));
        }
        registries.push(sources.xdg_data_home().join("crush").join("projects.json"));
        // WINDOWS-PATH: unverified
        registries.push(sources.local_app_data().join("crush").join("projects.json"));

        let mut roots = Vec::new();
        for registry in registries {
            // The registry is itself an input: adding or removing a project changes it without
            // touching any database's stamp.
            push_unique(&mut roots, registry.clone());
            for database in databases(&registry) {
                push_unique(&mut roots, database);
            }
        }
        roots
    }

    /// No records: Crush reports cost, not tokens. See the module note.
    fn records(&self, _roots: &[PathBuf]) -> Vec<AgentUsageRecord> {
        Vec::new()
    }
}

/// The database each project in a registry names, with `..` resolved.
fn databases(registry: &Path) -> Vec<PathBuf> {
    let Some(json) = logio::json(registry) else { return Vec::new() };
    let Some(projects) = json.get("projects").and_then(Value::as_array) else { return Vec::new() };
    projects
        .iter()
        .filter_map(|project| {
            let path = project.get("path").and_then(Value::as_str).filter(|p| !p.trim().is_empty())?;
            let data_dir = project.get("data_dir").and_then(Value::as_str).filter(|d| !d.trim().is_empty())?;
            let directory = if data_dir.starts_with('/') || Path::new(data_dir).is_absolute() {
                PathBuf::from(data_dir)
            } else {
                PathBuf::from(path).join(data_dir)
            };
            Some(normalize(&directory.join("crush.db")))
        })
        .collect()
}

/// `path` with `.` and `..` components resolved lexically, as Foundation's standardizing does.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend::calendar::Calendar;
    use crate::spend::prices::PriceTable;
    use crate::spend::sources::{read_agent, SpendAgent};
    use crate::spend::sqlite::tests::database;

    #[test]
    fn the_registry_and_the_databases_it_names_are_inputs_and_no_tokens_are_read() {
        let home = tempfile::tempdir().unwrap();
        let folder = home.path().join(".local").join("share").join("crush");
        std::fs::create_dir_all(&folder).unwrap();
        let project = home.path().join("work").join("app");
        let registry = serde_json::json!({"projects": [
            {"path": project.to_string_lossy(), "data_dir": ".crush"},
            {"path": "/elsewhere", "data_dir": "/var/crush"}
        ]});
        std::fs::write(folder.join("projects.json"), registry.to_string()).unwrap();
        // A database with rows: cost only, so still no record.
        let data = project.join(".crush");
        std::fs::create_dir_all(&data).unwrap();
        database(
            &data.join("crush.db"),
            &["CREATE TABLE sessions (id TEXT, cost REAL, prompt_tokens INTEGER, completion_tokens INTEGER)", "INSERT INTO sessions VALUES ('s', 1.5, 100, 20)"],
        );

        let sources = Sources::new(home.path());
        let roots = Crush.inputs(&sources);
        assert!(roots.contains(&folder.join("projects.json")));
        assert!(roots.contains(&data.join("crush.db")));
        assert!(roots.iter().any(|r| r.ends_with("var/crush/crush.db") || r.ends_with("var\\crush\\crush.db")));
        assert!(Crush.records(&roots).is_empty());

        let cache = tempfile::tempdir().unwrap();
        let ledger = read_agent(SpendAgent::Crush, &sources, cache.path(), &Calendar::utc(2), &PriceTable::new());
        assert!(ledger.days.is_empty());
    }

    #[test]
    fn a_missing_registry_names_its_own_path_and_no_databases() {
        let home = tempfile::tempdir().unwrap();
        let roots = Crush.inputs(&Sources::new(home.path()));
        assert!(roots.iter().all(|r| r.file_name().is_some_and(|n| n == "projects.json")));
        assert!(!roots.is_empty());
    }
}
