//! Windows difference: restore presentation after remote access, display or compositor transitions.
//! A layered WebView can disappear while IsWindowVisible still returns true. Session messages only
//! request recovery; the shell checks the current settings and full-screen policy before showing it.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::WebviewWindow;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    WM_DISPLAYCHANGE, WM_DWMCOMPOSITIONCHANGED, WM_NCDESTROY, WM_POWERBROADCAST,
    WM_WTSSESSION_CHANGE, PBT_APMRESUMEAUTOMATIC, WTS_CONSOLE_CONNECT, WTS_REMOTE_CONNECT,
    WTS_SESSION_UNLOCK,
};

static INSTALLED: AtomicBool = AtomicBool::new(false);
static QUEUED: AtomicBool = AtomicBool::new(false);
static RESTORE: AtomicBool = AtomicBool::new(false);
const SUBCLASS: usize = 1;
// Defined by Winuser.h, but absent from windows 0.58's metadata.
const WTS_SESSION_DESKTOP_READY: u32 = 0xF;

/// Called by the shell watcher. Retry registration if Terminal Services was not ready at startup.
pub fn watch(window: &WebviewWindow) {
    if INSTALLED.load(Ordering::Acquire) || QUEUED.swap(true, Ordering::AcqRel) {
        return;
    }
    let window = window.clone();
    let dispatch = window.clone();
    if dispatch.run_on_main_thread(move || {
        if let Some(hwnd) = crate::win::hwnd(&window) {
            INSTALLED.store(install(hwnd), Ordering::Release);
        }
        QUEUED.store(false, Ordering::Release);
    }).is_err() {
        QUEUED.store(false, Ordering::Release);
    }
}

/// UI thread only: subclass helpers cannot be used across threads.
fn install(hwnd: HWND) -> bool {
    unsafe {
        if !SetWindowSubclass(hwnd, Some(session_proc), SUBCLASS, 0).as_bool() {
            return false;
        }
        if WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION).is_err() {
            // Retain display/compositor notifications even without Terminal Services. The watcher
            // retries WTS registration, replacing this same subclass rather than adding another.
            return false;
        }
    }
    true
}

pub fn take_restore_request() -> bool {
    RESTORE.swap(false, Ordering::AcqRel)
}

unsafe extern "system" fn session_proc(
    hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize,
) -> LRESULT {
    let session_ready = msg == WM_WTSSESSION_CHANGE && matches!(
        wparam.0 as u32,
        WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT | WTS_SESSION_UNLOCK | WTS_SESSION_DESKTOP_READY
    );
    // Streaming tools need not create a WTS session. Their virtual displays / compositor switches,
    // and resuming a streamed host, must recover presentation through the same path.
    if session_ready || matches!(msg, WM_DISPLAYCHANGE | WM_DWMCOMPOSITIONCHANGED)
        || (msg == WM_POWERBROADCAST && wparam.0 as u32 == PBT_APMRESUMEAUTOMATIC)
    {
        // No Tauri calls or panel locks inside the window procedure.
        RESTORE.store(true, Ordering::Release);
    } else if msg == WM_NCDESTROY {
        let _ = WTSUnRegisterSessionNotification(hwnd);
        let _ = RemoveWindowSubclass(hwnd, Some(session_proc), SUBCLASS);
        INSTALLED.store(false, Ordering::Release);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SendMessageW, WTS_REMOTE_DISCONNECT, WTS_SESSION_LOCK,
        WM_SIZE, WS_EX_TOOLWINDOW, WS_POPUP,
    };

    #[test]
    fn session_messages_request_recovery_even_for_a_hidden_window() {
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW, w!("STATIC"), w!("PulseSessionTest"), WS_POPUP,
                0, 0, 1, 1, None, None, None, None,
            ).expect("isolated session test window");
            struct Window(HWND);
            impl Drop for Window {
                fn drop(&mut self) { let _ = unsafe { DestroyWindow(self.0) }; }
            }
            let _window = Window(hwnd);
            // Exercise production installation, including display recovery when WTS is unavailable.
            let _ = install(hwnd);
            take_restore_request();
            SendMessageW(hwnd, WM_SIZE, WPARAM(0), LPARAM(0));
            assert!(!take_restore_request(), "our own placement must not start another recovery");
            for event in [WTS_REMOTE_DISCONNECT, WTS_SESSION_LOCK] {
                SendMessageW(hwnd, WM_WTSSESSION_CHANGE, WPARAM(event as usize), LPARAM(0));
                assert!(!take_restore_request(), "disconnect/lock must not show the panel");
            }
            for event in [WTS_REMOTE_CONNECT, WTS_CONSOLE_CONNECT, WTS_SESSION_UNLOCK, WTS_SESSION_DESKTOP_READY] {
                SendMessageW(hwnd, WM_WTSSESSION_CHANGE, WPARAM(event as usize), LPARAM(0));
                assert!(take_restore_request(), "reconnect must request presentation recovery");
                assert!(!take_restore_request(), "each request is consumed once");
                assert!(!windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd).as_bool(),
                    "the message handler must leave visibility to the shell's policy");
            }
            for (msg, event) in [
                (WM_DISPLAYCHANGE, 32),
                (WM_DWMCOMPOSITIONCHANGED, 0),
                (WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC),
            ] {
                SendMessageW(hwnd, msg, WPARAM(event as usize), LPARAM(0));
                assert!(take_restore_request(), "streaming/display transitions also request recovery");
            }
        }
    }
}
