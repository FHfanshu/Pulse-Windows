// Ported from upstream App/LoginItem.swift (the decision of what the login item should be).
//! Start at login: what the `HKCU\…\Run\Pulse` value should say and what to do about what it says.
//!
//! The registry reads and writes are in the shell (`win::login_item`); the decisions are here so
//! they can be pinned without touching the registry.

use std::path::Path;

/// Added to the Run command so a start at login is silent: no provider chooser, no Settings, just
/// the panel and the tray icon.
pub const AUTOSTART_ARGUMENT: &str = "--autostart";

/// The Run value for `exe`: quoted, because install paths contain spaces.
pub fn command(exe: &Path) -> String {
    format!("\"{}\" {AUTOSTART_ARGUMENT}", exe.display())
}

/// What the shell does about the login item at launch or when the switch changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    Nothing,
    /// Write the Run value (missing, or pointing somewhere else).
    Write,
    /// Remove the Run value.
    Remove,
    /// The user turned Pulse off in Windows (Settings > Apps > Startup, or Task Manager): the
    /// switch follows, and the value is left for Windows to keep as the user set it.
    SwitchOff,
}

/// Decide. `current` is the Run value as it stands, `wanted` is [`command`] for this exe,
/// `disabled_in_windows` is the Startup apps toggle (see [`startup_disabled`]), and `follow_windows`
/// is whether that toggle may switch the setting off (at launch, not when the user has just
/// flipped the switch themselves).
pub fn plan(enabled: bool, current: Option<&str>, wanted: &str, disabled_in_windows: bool, follow_windows: bool) -> Plan {
    match (enabled, current) {
        (false, None) => Plan::Nothing,
        (false, Some(_)) => Plan::Remove,
        (true, Some(_)) if disabled_in_windows && follow_windows => Plan::SwitchOff,
        (true, Some(c)) if same(c, wanted) => Plan::Nothing,
        // Missing, or left by another build (a development build, an earlier install folder, a
        // version before the `--autostart` argument): point it at this exe.
        (true, _) => Plan::Write,
    }
}

fn same(current: &str, wanted: &str) -> bool {
    // Windows paths are case-insensitive.
    current.trim().eq_ignore_ascii_case(wanted.trim())
}

/// The data of `…\Explorer\StartupApproved\Run\Pulse`, which Windows' Startup apps page and Task
/// Manager write instead of touching the Run value. Its first byte is 2 or 6 for enabled and 3 for
/// disabled (the following eight are the time it was switched); the low bit is the tell. A value
/// that is missing, or too short to mean anything, is enabled.
pub fn startup_disabled(approved: Option<&[u8]>) -> bool {
    approved.and_then(|b| b.first()).is_some_and(|b| b & 1 == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const EXE: &str = r"C:\Users\me\AppData\Local\Pulse\pulse.exe";

    fn wanted() -> String {
        command(&PathBuf::from(EXE))
    }

    #[test]
    fn the_command_is_quoted_and_silent() {
        assert_eq!(wanted(), r#""C:\Users\me\AppData\Local\Pulse\pulse.exe" --autostart"#);
        assert_eq!(
            command(Path::new(r"C:\Program Files\Pulse\pulse.exe")),
            r#""C:\Program Files\Pulse\pulse.exe" --autostart"#
        );
    }

    #[test]
    fn switched_on_and_missing_writes_it() {
        assert_eq!(plan(true, None, &wanted(), false, true), Plan::Write);
    }

    #[test]
    fn switched_on_and_right_leaves_it_alone_whatever_the_case() {
        assert_eq!(plan(true, Some(&wanted()), &wanted(), false, true), Plan::Nothing);
        assert_eq!(plan(true, Some(&wanted().to_uppercase()), &wanted(), false, true), Plan::Nothing);
    }

    #[test]
    fn a_value_left_by_another_build_is_pointed_here() {
        let dev = r#""D:\code\Pulse\target\debug\pulse.exe" --autostart"#;
        assert_eq!(plan(true, Some(dev), &wanted(), false, true), Plan::Write);
        // The value an earlier release wrote, without the argument.
        let old = format!("\"{EXE}\"");
        assert_eq!(plan(true, Some(&old), &wanted(), false, true), Plan::Write);
    }

    #[test]
    fn switched_off_removes_what_is_there() {
        assert_eq!(plan(false, Some(&wanted()), &wanted(), false, true), Plan::Remove);
        assert_eq!(plan(false, None, &wanted(), false, true), Plan::Nothing);
    }

    #[test]
    fn windows_turning_it_off_turns_the_switch_off_at_launch() {
        assert_eq!(plan(true, Some(&wanted()), &wanted(), true, true), Plan::SwitchOff);
        // Even when the value is stale: the user's choice stands, and nothing is written over it.
        assert_eq!(plan(true, Some("old"), &wanted(), true, true), Plan::SwitchOff);
    }

    #[test]
    fn a_switch_the_user_has_just_flipped_wins_over_a_stale_windows_toggle() {
        assert_eq!(plan(true, Some(&wanted()), &wanted(), true, false), Plan::Nothing);
        assert_eq!(plan(true, None, &wanted(), true, false), Plan::Write);
    }

    #[test]
    fn startup_approved_bytes() {
        let enabled = [2u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let enabled_alt = [6u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let disabled = [3u8, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8];
        assert!(!startup_disabled(None));
        assert!(!startup_disabled(Some(&[])));
        assert!(!startup_disabled(Some(&enabled)));
        assert!(!startup_disabled(Some(&enabled_alt)));
        assert!(startup_disabled(Some(&disabled)));
    }
}
