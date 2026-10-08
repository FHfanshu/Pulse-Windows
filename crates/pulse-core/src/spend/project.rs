// Ported from upstream Sources/Pulse/Usage/UsageProject.swift and SessionLabel.swift.
//! Project identity survives reading and caching; its short name is only a label.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectIdentity {
    Directory(String),
    Label(String),
    /// A store's project folder when the working directory is not known.
    Source(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageProject {
    pub identity: ProjectIdentity,
    pub name: String,
}

impl<'de> Deserialize<'de> for UsageProject {
    /// Ledgers cached before `repository_of` existed hold worktree paths; they are folded in as
    /// they are read rather than waiting for a rescan.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            identity: ProjectIdentity,
            name: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        if let ProjectIdentity::Directory(path) = &raw.identity {
            let repository = UsageProject::repository_of(path);
            if repository != *path {
                let name = last_component(&repository);
                return Ok(UsageProject { identity: ProjectIdentity::Directory(repository), name });
            }
        }
        Ok(UsageProject { identity: raw.identity, name: raw.name })
    }
}

fn last_component(path: &str) -> String {
    path.split('/').rfind(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| "/".to_string())
}

/// `X:\dir`, `X:/dir` or `\\server\share`: a Windows directory the transcripts state.
fn windows_directory(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let drive = bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/');
    let unc = value.starts_with("\\\\");
    if !drive && !unc {
        return None;
    }
    let joined = value.replace('\\', "/").split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/");
    Some(if unc { format!("//{joined}") } else { joined })
}

impl UsageProject {
    /// A project from a working directory or a label. `None` for a blank value and for a scratch
    /// folder a tool made for a chat that has no project.
    pub fn new(value: Option<&str>) -> Option<UsageProject> {
        let value = value?;
        if value.trim().is_empty() {
            return None;
        }
        let directory = if value.starts_with('/') {
            Some(format!("/{}", value.split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/")))
        } else {
            windows_directory(value)
        };
        if let Some(path) = directory {
            if Self::is_scratch_directory(&value.replace('\\', "/")) {
                return None;
            }
            // Normalise separators only. Resolving symlinks or `..` would make historical
            // attribution depend on the current filesystem.
            let path = Self::repository_of(&path);
            let name = last_component(&path);
            return Some(UsageProject { identity: ProjectIdentity::Directory(path), name });
        }
        // A workspace URI that is not a local path (VS Code's Remote-SSH
        // `vscode-remote://ssh-remote%2Bhost/home/me/proj`) is still one project, named by its
        // last folder; the whole URI stays its identity, so the same folder on two hosts is not
        // merged.
        let name = if value.contains("://") { uri_last_component(value).unwrap_or_else(|| value.to_string()) } else { value.to_string() };
        Some(UsageProject { identity: ProjectIdentity::Label(value.to_string()), name })
    }

    pub fn source(source: &str, name: &str) -> UsageProject {
        UsageProject { identity: ProjectIdentity::Source(source.to_string()), name: name.to_string() }
    }

    pub fn path(&self) -> Option<&str> {
        match &self.identity {
            ProjectIdentity::Directory(path) => Some(path),
            _ => None,
        }
    }

    /// A worktree an agent works in is its repository's work. Claude Code puts each one at
    /// `<repo>/.claude/worktrees/<name>`, and named by its last folder every subagent's run was
    /// a project of its own beside the repository it was done for. A string rule: nothing on
    /// disk is consulted.
    pub fn repository_of(path: &str) -> String {
        match path.find("/.claude/worktrees/") {
            Some(at) if at > 0 => path[..at].to_string(),
            _ => path.to_string(),
        }
    }

    /// A scratch folder a tool makes is no project: Codex Desktop's
    /// `~/Documents/Codex/<yyyy-mm-dd>/<slug of the first prompt>` for a chat with none, and
    /// DeepSeek Harness's `.../deepseek-harness/default-workspace`. A pure string rule.
    pub fn is_scratch_directory(path: &str) -> bool {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        for index in 0..parts.len() {
            if index + 1 < parts.len() && parts[index] == "deepseek-harness" && parts[index + 1] == "default-workspace" {
                return true;
            }
            // The slug is the last of the four, so it must be there.
            if index + 3 < parts.len() && parts[index] == "Documents" && parts[index + 1] == "Codex" && is_date(parts[index + 2]) {
                return true;
            }
        }
        false
    }

    /// A project read from a cache, or None when it names a scratch folder.
    pub fn unless_scratch(self) -> Option<UsageProject> {
        match self.path() {
            Some(path) if Self::is_scratch_directory(path) => None,
            _ => Some(self),
        }
    }

    /// Extend only ambiguous directory names, using the shortest distinct suffix.
    pub fn display_names(projects: &HashSet<UsageProject>) -> HashMap<UsageProject, String> {
        let mut groups: HashMap<&str, Vec<&UsageProject>> = HashMap::new();
        for project in projects {
            groups.entry(project.name.as_str()).or_default().push(project);
        }
        let mut names = HashMap::new();
        for group in groups.values() {
            for project in group {
                names.insert((*project).clone(), Self::display_name(project, group));
            }
        }
        names
    }

    pub fn display_name(project: &UsageProject, among: &[&UsageProject]) -> String {
        let peers: Vec<&&UsageProject> = among.iter().filter(|p| ***p != *project && p.name == project.name).collect();
        if peers.is_empty() {
            return project.name.clone();
        }
        let Some(path) = project.path() else {
            if let ProjectIdentity::Source(source) = &project.identity {
                return source.clone();
            }
            return project.name.clone();
        };
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let suffix = |parts: &[&str], count: usize| -> String { parts[parts.len().saturating_sub(count)..].join("/") };
        for count in 2..=parts.len().max(2) {
            let mine = suffix(&parts, count);
            let distinct = peers.iter().all(|peer| match peer.path() {
                None => mine != peer.name,
                Some(other) => {
                    let other_parts: Vec<&str> = other.split('/').filter(|s| !s.is_empty()).collect();
                    suffix(&other_parts, count) != mine
                }
            });
            if distinct {
                return mine;
            }
        }
        path.to_string()
    }
}

fn is_date(part: &str) -> bool {
    let bytes = part.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(i, b)| if i == 4 || i == 7 { *b == b'-' } else { b.is_ascii_digit() })
}

/// The last path segment of a URI, percent-decoded; None when it has none.
fn uri_last_component(uri: &str) -> Option<String> {
    let after_scheme = uri.split_once("://")?.1;
    let without_query = after_scheme.split(['?', '#']).next().unwrap_or(after_scheme);
    let path = without_query.split_once('/')?.1;
    let segment = path.split('/').rfind(|s| !s.is_empty())?;
    Some(percent_decode(segment))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Some(value) = text.get(index + 1..index + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What a session is called wherever one is listed: the conversation's own title where it has
/// one; a review session Codex ran by itself says so; else the project it ran in; else
/// "Untitled conversation". Never the transcript's file name.
pub struct SessionLabel;

impl SessionLabel {
    /// English keys, as upstream; the UI localises them.
    pub const CODEX_REVIEW: &'static str = "Codex review";
    pub const UNTITLED: &'static str = "Untitled conversation";

    pub fn text(title: Option<&str>, is_review: bool, project: Option<&str>) -> String {
        if let Some(title) = title {
            return title.to_string();
        }
        if is_review {
            return Self::CODEX_REVIEW.to_string();
        }
        if let Some(project) = project {
            return project.to_string();
        }
        Self::UNTITLED.to_string()
    }

    /// Whether the label is the session's own name rather than its project, which is when a
    /// row's subtitle should add the project, so it is never said twice.
    pub fn names_itself(title: Option<&str>, is_review: bool) -> bool {
        title.is_some() || is_review
    }
}
