use serde::{Serialize, Deserialize};
use std::path::PathBuf;
use std::os::windows::process::CommandExt;

#[derive(Serialize, Deserialize)]
pub struct SyncStatus {
    pub last_sync: String,
    pub ok: i32,
    pub fail: i32,
    pub restored: i32,
    pub restored_names: String,
}

pub struct HealthResult {
    pub status: String,
    pub label: String,
    pub reason: String,
}

pub fn status_path() -> PathBuf {
    crate::config::data_dir().join("sync-status.json")
}

pub fn write_status(ok: i32, fail: i32, restored: i32, names: &[String]) {
    let s = SyncStatus {
        last_sync: crate::synclog::timestamp(),
        ok, fail, restored,
        restored_names: names.join(", "),
    };
    if let Ok(data) = serde_json::to_string(&s) {
        // L6: atomic write (temp + rename) — a torn sync-status.json made the
        // health lamp read stale/default forever.
        let tmp = status_path().with_extension("json.tmp");
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(&tmp, status_path());
        }
    }
}

pub fn task_exists() -> bool {
    let output = std::process::Command::new("schtasks.exe")
        .args(&["/Query", "/TN", "LRGEX-Restore-Rust", "/FO", "LIST"])
        .creation_flags(0x08000000u32)
        .output();
    match output {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}

/// Is the task currently running? (M8: locale-INDEPENDENT — check the task's
/// running PID via XML-free query instead of parsing localized /V text.)
fn task_running() -> bool {
    // PowerShell resolves the locale-independent LastTaskResult/State via COM:
    // faster and reliable on non-English Windows. Fall back to the old parse.
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command",
            "(Get-ScheduledTask -TaskName 'LRGEX-Restore-Rust' -ErrorAction SilentlyContinue).State -eq 'Running'"])
        .creation_flags(0x08000000u32)
        .output();
    if let Ok(out) = output {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
            return text == "true";
        }
    }
    // Fallback: legacy localized parse (English Windows) — best effort.
    let output = std::process::Command::new("schtasks.exe")
        .args(&["/Query", "/TN", "LRGEX-Restore-Rust", "/FO", "LIST", "/V"])
        .creation_flags(0x08000000u32)
        .output();
    if let Ok(out) = output {
        if !out.status.success() { return false; }
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let t = line.trim();
            if t.to_lowercase().starts_with("status:") {
                return t.contains("Running");
            }
        }
    }
    false
}

fn friendly_time(ts: &str) -> String {
    // stored format: "2026-08-04 06:41:23 AM" -> "6:41 AM"
    let parts: Vec<&str> = ts.split_whitespace().collect();
    if parts.len() >= 3 {
        let mut segs = parts[1].split(':');
        if let (Some(h), Some(m)) = (segs.next(), segs.next()) {
            let h = h.trim_start_matches('0');
            let h = if h.is_empty() { "12" } else { h };
            return format!("{}:{} {}", h, m, parts[2]);
        }
    }
    ts.to_string()
}

pub fn get_health() -> HealthResult {
    // 1. Task not registered = RED (sync will not run automatically)
    if !task_exists() {
        return HealthResult {
            status: "RED".into(),
            label: "Protection off • will resume on next launch".into(),
            reason: String::new(),
        };
    }

    // 2. Task running right now = AMBER
    if task_running() {
        return HealthResult {
            status: "AMBER".into(),
            label: "Protecting your folders…".into(),
            reason: String::new(),
        };
    }

    // 3. Read last sync status
    match std::fs::read_to_string(status_path()) {
        Ok(data) => {
            if let Ok(s) = serde_json::from_str::<SyncStatus>(&data) {
                if s.fail > 0 {
                    return HealthResult {
                        status: "RED".into(),
                        label: format!("⚠ Needs attention • {} {} failed", s.fail, if s.fail == 1 { "folder" } else { "folders" }).into(),
                        reason: String::new(),
                    };
                }
                if s.restored > 0 {
                    return HealthResult {
                        status: "GREEN".into(),
                        label: format!("✔ Restored {} {} • auto-restored: {}", s.restored, if s.restored == 1 { "folder" } else { "folders" }, s.restored_names).into(),
                        reason: String::new(),
                    };
                }
                return HealthResult {
                    status: "GREEN".into(),
                    label: format!("✔ {} {} protected • {}", s.ok, if s.ok == 1 { "folder" } else { "folders" }, friendly_time(&s.last_sync)).into(),
                    reason: String::new(),
                };
            }
        }
        Err(_) => {}
    }

    // 4. Task registered but hasn't run yet = AMBER (not green!)
    HealthResult {
        status: "AMBER".into(),
        label: "Starting up — preparing your first backup".into(),
        reason: String::new(),
    }
}
