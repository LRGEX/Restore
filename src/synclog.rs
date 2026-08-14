use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn log_path() -> PathBuf {
    crate::config::data_dir().join("sync.log")
}

pub fn timestamp() -> String {
    use chrono::Local;
    Local::now().format("%Y-%m-%d %I:%M:%S %p").to_string()
}

pub fn write(msg: &str) {
    let path = log_path();
    let line = format!("{} {}\r\n", timestamp(), msg);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
    // L-6: mtime-guarded trim — two processes passing 2050 lines near-simultaneously
    // would last-writer-wins and lose ~50 lines. Capture mtime before the read;
    // write the trim only if nothing appended in between (next append retries).
    let mtime_before = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    if let Ok(data) = std::fs::read_to_string(&path) {
        let lines: Vec<&str> = data.lines().collect();
        if lines.len() > 2050 {
            let mtime_now = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            if mtime_before.is_some() && mtime_before == mtime_now {
                let trimmed = lines[lines.len()-2000..].join("\r\n");
                let _ = std::fs::write(&path, trimmed);
            }
        }
    }
}

pub fn write_progress(msg: &str) {
    let path = crate::config::data_dir().join("sync-progress.txt");
    let pid = std::process::id();
    let line = format!("{}|{}", pid, msg);
    let _ = std::fs::write(&path, line);
}

pub fn read_progress() -> String {
    let raw = std::fs::read_to_string(crate::config::data_dir().join("sync-progress.txt"))
        .unwrap_or_default();
    let raw = raw.trim();
    if raw.is_empty() { return String::new(); }

    // Format: PID|message — check BOTH PID liveness AND file mtime (PID reuse backstop)
    if let Some((pid_str, msg)) = raw.split_once('|') {
        if let Ok(pid) = pid_str.parse::<u32>() {
            let path = crate::config::data_dir().join("sync-progress.txt");
            // mtime backstop: clear if file untouched >10 min (defeats PID reuse)
            let stale = std::fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| t.elapsed().unwrap_or_default().as_secs() > 600)
                .unwrap_or(true);
            if stale {
                let _ = std::fs::write(&path, "");
                return String::new();
            }
            if is_pid_alive(pid) {
                return msg.to_string();
            } else {
                let _ = std::fs::write(&path, "");
                return String::new();
            }
        }
    }
    String::new()
}

pub fn is_pid_alive(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows_sys::Win32::Foundation::CloseHandle;
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // L-3: ACCESS_DENIED means the process EXISTS (e.g. elevated LRGEX
            // while we run unelevated) — treating it as dead made the sweep
            // delete a live elevated writer's temp.
            return windows_sys::Win32::Foundation::GetLastError() == 5; // ERROR_ACCESS_DENIED
        }
        CloseHandle(handle);
        true
    }
}

pub fn read_tail(n: usize) -> String {
    match std::fs::read_to_string(log_path()) {
        Ok(data) => {
            let lines: Vec<&str> = data.lines().collect();
            let start = if lines.len() > n { lines.len() - n } else { 0 };
            lines[start..].join("\r\n")
        }
        Err(_) => "No sync log yet.".into(),
    }
}

// ==================== LIVE PROGRESS (heartbeat) ====================
// Structured progress for the GUI: written every 500ms regardless of file count,
// so the UI never looks frozen during a slow parallel read.

pub fn status_path() -> PathBuf {
    crate::config::data_dir().join("sync-progress.json")
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Clone)]
pub struct Progress {
    inner: Arc<ProgressInner>,
}

struct ProgressInner {
    phase: AtomicUsize,        // 0=walk 1=compress 2=flush 3=done 4=error
    files_done: AtomicUsize,
    files_total: AtomicUsize,
    bytes_done: AtomicU64,
    bytes_total: AtomicU64,
    started: Instant,
    stop: AtomicBool,
    label: Mutex<String>,
    path: PathBuf,
}

impl Progress {
    pub fn new(label: &str) -> Self {
        Progress {
            inner: Arc::new(ProgressInner {
                phase: AtomicUsize::new(0),
                files_done: AtomicUsize::new(0),
                files_total: AtomicUsize::new(0),
                bytes_done: AtomicU64::new(0),
                bytes_total: AtomicU64::new(0),
                started: Instant::now(),
                stop: AtomicBool::new(false),
                label: Mutex::new(label.to_string()),
                path: status_path(),
            }),
        }
    }

    pub fn set_phase(&self, p: usize) { self.inner.phase.store(p, Ordering::Relaxed); }
    pub fn set_totals(&self, files: usize, bytes: u64) {
        self.inner.files_total.store(files, Ordering::Relaxed);
        self.inner.bytes_total.store(bytes, Ordering::Relaxed);
    }
    /// Call from INSIDE the parallel read — one per file, as soon as it's read.
    pub fn tick(&self, bytes: u64) {
        self.inner.files_done.fetch_add(1, Ordering::Relaxed);
        self.inner.bytes_done.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Tick bytes only (no file count) — for streaming progress within a single large file.
    pub fn tick_bytes(&self, bytes: u64) {
        self.inner.bytes_done.fetch_add(bytes, Ordering::Relaxed);
    }

    fn snapshot_json(&self, pid: u32) -> String {
        let done = self.inner.files_done.load(Ordering::Relaxed);
        let total = self.inner.files_total.load(Ordering::Relaxed);
        let bdone = self.inner.bytes_done.load(Ordering::Relaxed);
        let btotal = self.inner.bytes_total.load(Ordering::Relaxed);
        let elapsed = self.inner.started.elapsed().as_secs_f64().max(0.001);
        let rate = bdone as f64 / elapsed;
        let eta = if rate > 1.0 && btotal > bdone { ((btotal - bdone) as f64 / rate) as u64 } else { 0 };
        let label = self.inner.label.lock().map(|g| g.clone()).unwrap_or_default();
        format!(
            "{{\"heartbeat\":{},\"pid\":{},\"phase\":{},\"label\":{},\"files_done\":{},\"files_total\":{},\"bytes_done\":{},\"bytes_total\":{},\"elapsed\":{:.1},\"rate\":{:.1},\"eta\":{}}}",
            now_secs(), pid,
            self.inner.phase.load(Ordering::Relaxed),
            serde_json::to_string(&label).unwrap_or_else(|_| "\"\"".into()),
            done, total, bdone, btotal, elapsed, rate, eta
        )
    }

    /// Spawns a writer that updates sync-progress.json every 500ms regardless of progress.
    pub fn spawn_writer(&self) -> std::thread::JoinHandle<()> {
        let me = self.clone();
        std::thread::spawn(move || {
            while !me.inner.stop.load(Ordering::Relaxed) {
                me.write_snapshot();
                std::thread::sleep(Duration::from_millis(500));
            }
        })
    }

    fn write_snapshot(&self) {
        let json = self.snapshot_json(std::process::id());
        let tmp = self.inner.path.with_extension("tmp");
        if std::fs::write(&tmp, json.as_bytes()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.inner.path);
        }
    }

    pub fn finish(&self, phase: usize) {
        self.set_phase(phase);
        self.write_snapshot();
        self.inner.stop.store(true, Ordering::Relaxed);
    }
}

/// Parsed status for the GUI reader.
#[derive(serde::Deserialize)]
pub struct Status {
    pub heartbeat: u64,
    pub pid: u32,
    pub phase: usize,
    pub label: String,
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub elapsed: f64,
    pub rate: f64,
    pub eta: u64,
}

pub fn read_status() -> Option<Status> {
    let raw = std::fs::read_to_string(status_path()).ok()?;
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw); // strip BOM
    serde_json::from_str::<Status>(raw).ok()
}

pub fn clear_status() {
    let _ = std::fs::write(status_path(), r#"{"phase":3}"#);
}
