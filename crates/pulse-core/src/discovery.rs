//! Presence-only discovery for the provider chooser: does a tool's folder exist?
//! Nothing is opened or read, so choosing services stays a decision made before
//! any credential is touched.

use std::path::Path;

use crate::Provider;

/// Providers whose tool left a folder behind, in `Provider::ALL` order.
pub fn detected(home: &Path, app_data: &Path, local_app_data: &Path) -> Vec<Provider> {
    Provider::ALL
        .iter()
        .copied()
        .filter(|p| candidates(*p, home, app_data, local_app_data).iter().any(|path| path.exists()))
        .collect()
}

fn candidates(
    provider: Provider,
    home: &Path,
    app_data: &Path,
    // Kept in the signature so a provider can look under Local without a new call shape.
    _local_app_data: &Path,
) -> Vec<std::path::PathBuf> {
    match provider {
        Provider::ClaudeCode => vec![home.join(".claude")],
        Provider::Codex => vec![home.join(".codex")],
        Provider::Gemini => vec![home.join(".gemini")],
        Provider::Cursor => vec![app_data.join("Cursor")],
        Provider::Windsurf => vec![app_data.join("Windsurf")],
        Provider::Kiro => vec![home.join(".kiro")],
        Provider::Antigravity => vec![app_data.join("Antigravity")],
        Provider::Grok => vec![home.join(".grok")],
        Provider::OpenCodeGo => vec![home.join(".local").join("share").join("opencode")],
        Provider::Factory => vec![home.join(".factory")],
        Provider::Amp => vec![home.join(".config").join("amp"), app_data.join("amp")],
        Provider::KimiCode => vec![home.join(".kimi")],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pulse-discovery-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn nothing_found_in_an_empty_home() {
        let root = scratch("empty");
        assert!(detected(&root, &root, &root).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn finds_folders_under_home_and_app_data() {
        let root = scratch("found");
        let (home, roaming, local) = (root.join("home"), root.join("roaming"), root.join("local"));
        for dir in [
            home.join(".claude"),
            home.join(".kimi"),
            home.join(".local").join("share").join("opencode"),
            roaming.join("Cursor"),
            roaming.join("amp"),
            local.clone(),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        let found = detected(&home, &roaming, &local);
        assert_eq!(
            found,
            vec![Provider::ClaudeCode, Provider::Cursor, Provider::OpenCodeGo, Provider::KimiCode, Provider::Amp]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_counts_and_amp_has_two_homes() {
        let root = scratch("file");
        let (home, roaming) = (root.join("home"), root.join("roaming"));
        fs::create_dir_all(home.join(".config").join("amp")).unwrap();
        fs::create_dir_all(&roaming).unwrap();
        fs::write(home.join(".codex"), b"").unwrap();
        let found = detected(&home, &roaming, &roaming);
        assert_eq!(found, vec![Provider::Codex, Provider::Amp]);
        let _ = fs::remove_dir_all(&root);
    }
}
