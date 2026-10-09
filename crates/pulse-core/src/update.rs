// Ported from upstream App/AppUpdate.swift (what the rest of the app asks of the updater).
//! The state of Pulse's own updates: whether a check can happen, whether one is running, whether
//! a newer version was found, and how far an install has got.
//!
//! Upstream hands all of this to Sparkle, which owns the schedule and the windows it puts up, and
//! keeps only `newer` / `isChecking` / `didFail` for the menu bar and the About pane. Windows has
//! no Sparkle: the Tauri updater does the downloading and the installer does the swapping, so what
//! Sparkle's window shows (progress, failure) is kept here too and drawn by the About pane.
//! Pure state and transitions, so the rules can be pinned; the shell in `update_ipc` drives them.

use std::time::Duration;

use serde::Serialize;

/// Upstream `SUScheduledCheckInterval` (7200): "Every two hours."
pub const CHECK_INTERVAL: Duration = Duration::from_secs(2 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    #[default]
    Idle,
    Downloading,
    /// The installer has the files; Pulse closes and starts again by itself.
    Installing,
    /// The last install attempt did not complete. The newer version is still on offer.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    /// This build's version.
    pub current: String,
    /// Upstream `canCheck`: false for a build that is not an installed release ("Built from source").
    pub can_check: bool,
    pub checking: bool,
    /// The last check couldn't reach the feed ("no update" and "no answer" must not look alike).
    pub failed: bool,
    /// Set once a check has found something newer.
    pub newer: Option<String>,
    pub stage: Stage,
    pub downloaded: u64,
    pub total: Option<u64>,
}

impl UpdateState {
    pub fn new(current: &str, can_check: bool) -> Self {
        Self { current: current.to_string(), can_check, ..Self::default() }
    }

    /// A check or an install is under way: another check would only abort it.
    pub fn busy(&self) -> bool {
        self.checking || matches!(self.stage, Stage::Downloading | Stage::Installing)
    }

    /// Start a check. False when it can't happen now (nothing to check with, or one is under way).
    pub fn begin_check(&mut self) -> bool {
        if !self.can_check || self.busy() {
            return false;
        }
        self.checking = true;
        self.failed = false;
        // A new look at the feed leaves an earlier install failure behind; the version stays on offer.
        if self.stage == Stage::Failed {
            self.stage = Stage::Idle;
        }
        true
    }

    /// A check got an answer (`Ok`: the newer version, if any) or couldn't reach the feed.
    pub fn finish_check(&mut self, result: Result<Option<String>, ()>) {
        self.checking = false;
        match result {
            Ok(newer) => {
                self.failed = false;
                self.newer = newer;
            }
            // Upstream leaves `newer` as it was: a version found earlier is still there.
            Err(()) => self.failed = true,
        }
    }

    /// Start installing the version on offer. False when there is none, or an install is under way.
    pub fn begin_install(&mut self) -> bool {
        if self.newer.is_none() || self.busy() {
            return false;
        }
        self.stage = Stage::Downloading;
        self.downloaded = 0;
        self.total = None;
        true
    }

    pub fn downloaded_chunk(&mut self, chunk: u64, total: Option<u64>) {
        self.downloaded = self.downloaded.saturating_add(chunk);
        self.total = total;
    }

    pub fn installing(&mut self) {
        self.stage = Stage::Installing;
    }

    pub fn install_failed(&mut self) {
        self.stage = Stage::Failed;
    }

    /// 0.0 to 1.0, or None while the size is not known.
    pub fn fraction(&self) -> Option<f64> {
        let total = self.total.filter(|t| *t > 0)?;
        Some((self.downloaded as f64 / total as f64).clamp(0.0, 1.0))
    }
}

/// Whether the build in hand may check for updates: an installed release, or (so the About pane
/// can be tried against a local feed) any build told which feed to read.
pub fn can_check(is_debug_build: bool, feed_override: Option<&str>) -> bool {
    !is_debug_build || feed_override.is_some_and(|f| !f.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> UpdateState {
        UpdateState::new("0.1.0", true)
    }

    #[test]
    fn a_build_that_cannot_check_never_starts_one() {
        let mut s = UpdateState::new("0.1.0", false);
        assert!(!s.begin_check());
        assert!(!s.checking);
    }

    #[test]
    fn a_check_runs_alone_and_reports_what_it_found() {
        let mut s = state();
        assert!(s.begin_check());
        assert!(!s.begin_check(), "a second check would only abort the first");
        s.finish_check(Ok(Some("0.1.1".into())));
        assert!(!s.checking && !s.failed);
        assert_eq!(s.newer.as_deref(), Some("0.1.1"));
        assert!(s.begin_check());
        s.finish_check(Ok(None));
        assert_eq!(s.newer, None, "an answer of no update replaces the earlier one");
    }

    #[test]
    fn a_failed_check_says_so_and_keeps_what_was_known() {
        let mut s = state();
        s.begin_check();
        s.finish_check(Ok(Some("0.2.0".into())));
        s.begin_check();
        s.finish_check(Err(()));
        assert!(s.failed);
        assert_eq!(s.newer.as_deref(), Some("0.2.0"));
        s.begin_check();
        assert!(!s.failed, "starting again clears the failure");
    }

    #[test]
    fn installing_needs_a_newer_version_and_blocks_checks() {
        let mut s = state();
        assert!(!s.begin_install());
        s.begin_check();
        s.finish_check(Ok(Some("0.1.1".into())));
        assert!(s.begin_install());
        assert!(!s.begin_install());
        assert!(!s.begin_check());
        s.downloaded_chunk(250, Some(1000));
        s.downloaded_chunk(250, Some(1000));
        assert_eq!(s.fraction(), Some(0.5));
        s.installing();
        assert!(s.busy());
    }

    #[test]
    fn a_failed_install_can_be_tried_again() {
        let mut s = state();
        s.begin_check();
        s.finish_check(Ok(Some("0.1.1".into())));
        s.begin_install();
        s.downloaded_chunk(10, None);
        assert_eq!(s.fraction(), None);
        s.install_failed();
        assert!(!s.busy());
        assert_eq!(s.newer.as_deref(), Some("0.1.1"));
        assert!(s.begin_check());
        assert_eq!(s.stage, Stage::Idle, "a new check clears the failed install");
        s.finish_check(Ok(Some("0.1.1".into())));
        assert!(s.begin_install());
        assert_eq!(s.downloaded, 0, "a new attempt starts from nothing");
    }

    #[test]
    fn the_fraction_stays_within_bounds() {
        let mut s = state();
        s.total = Some(0);
        assert_eq!(s.fraction(), None);
        s.total = Some(100);
        s.downloaded = 500;
        assert_eq!(s.fraction(), Some(1.0));
    }

    #[test]
    fn only_an_installed_release_checks_unless_a_feed_is_given() {
        assert!(can_check(false, None));
        assert!(!can_check(true, None));
        assert!(!can_check(true, Some("  ")));
        assert!(can_check(true, Some("http://localhost:8080/latest.json")));
    }

    #[test]
    fn the_schedule_is_every_two_hours() {
        assert_eq!(CHECK_INTERVAL.as_secs(), 7200);
    }
}
