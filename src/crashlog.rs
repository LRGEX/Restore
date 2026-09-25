//! Crash forensics — "never suffer a mystery crash again".
//!
//! The app uses the Windows GUI subsystem (no console), so Rust's runtime
//! messages ("thread has overflowed its stack", abort details) were printed
//! into an invisible void — during the v1.6.1 stack-overflow hunt, the killer
//! clue existed on stderr the entire time but nobody could see it.
//!
//! This module redirects the process stderr into `.lrgex/stderr.log` so every
//! runtime-level failure leaves a written trace, and stamps the app version
//! into every sync cycle so logs are build-attributable forever.

/// Redirect process stderr (C runtime + Rust std) to `<data_dir>/stderr.log`.
/// Startup-only, before threads spawn — the safe window for handle redirection.
pub fn capture_stderr() {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_APPEND_DATA, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_ALWAYS,
        };
        use std::os::windows::ffi::OsStrExt;

        let path = crate::config::data_dir().join("stderr.log");
        // Fresh log per launch (crash traces stay until the NEXT launch).
        let _ = std::fs::write(&path, format!(
            "=== LRGEX Restore v{} launched at {} ===\r\n",
            env!("CARGO_PKG_VERSION"),
            crate::synclog::timestamp()
        ));
        let wide: Vec<u16> = std::ffi::OsStr::new(&path)
            .encode_wide().chain(std::iter::once(0)).collect();
        unsafe {
            let handle = CreateFileW(
                wide.as_ptr(),
                FILE_APPEND_DATA,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_ALWAYS,
                0,
                0 as HANDLE,
            );
            if handle != -1isize as HANDLE {
                SetStdHandle(STD_ERROR_HANDLE, handle);
            }
        }
    }
}

/// One-line header for every sync cycle — build-attributable logs forever.
pub fn version_stamp() -> String {
    format!("LRGEX Restore v{}", env!("CARGO_PKG_VERSION"))
}
