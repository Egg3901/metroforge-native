//! Single-instance guard: a second launch focuses the existing window
//! instead of spawning another client (and another sidecar).
//!
//! On Windows this uses a named mutex (`Local\MetroForge.SingleInstance`).
//! Elsewhere the check is a graceful no-op so Linux/macOS keep their
//! existing multi-instance behavior (useful for CI / dual-monitor debug).

/// Returns `true` when this process should continue starting the game.
/// Returns `false` when another instance already owns the mutex (and has
/// been asked to come to the foreground); the caller should exit.
pub fn ensure_single_instance() -> bool {
    ensure_single_instance_impl()
}

#[cfg(windows)]
fn ensure_single_instance_impl() -> bool {
    windows_impl::try_acquire()
}

#[cfg(not(windows))]
fn ensure_single_instance_impl() -> bool {
    true
}

#[cfg(windows)]
mod windows_impl {
    use std::sync::atomic::{AtomicIsize, Ordering};

    // Keep the mutex handle alive for the process lifetime. Leaking via a
    // static is intentional: Drop would release the mutex and allow a
    // second instance mid-run.
    static MUTEX_HANDLE: AtomicIsize = AtomicIsize::new(0);

    const MUTEX_NAME: &str = "Local\\MetroForge.SingleInstance";
    const WINDOW_TITLE: &str = "MetroForge";

    pub fn try_acquire() -> bool {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{
            CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE,
        };
        use windows_sys::Win32::System::Threading::CreateMutexW;

        let wide: Vec<u16> = std::ffi::OsStr::new(MUTEX_NAME)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // SAFETY: CreateMutexW with a valid null-terminated name; we only
        // store the handle and never double-close it.
        let handle: HANDLE = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
        if handle.is_null() {
            tracing::warn!(
                "mf-game: CreateMutexW failed; continuing without single-instance guard"
            );
            return true;
        }

        let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already {
            unsafe {
                CloseHandle(handle);
            }
            focus_existing_window();
            return false;
        }

        MUTEX_HANDLE.store(handle as isize, Ordering::SeqCst);
        true
    }

    fn focus_existing_window() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
        };

        let title: Vec<u16> = std::ffi::OsStr::new(WINDOW_TITLE)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // SAFETY: FindWindowW with a valid title; HWND is only passed to
        // documented user32 focus/restore helpers.
        let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if hwnd == 0 {
            tracing::warn!(
                "mf-game: another instance holds the single-instance mutex but no '{WINDOW_TITLE}' window was found"
            );
            return;
        }
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd);
        }
        tracing::info!("mf-game: focused existing MetroForge window; exiting duplicate launch");
    }
}
