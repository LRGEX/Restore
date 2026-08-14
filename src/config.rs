use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct Junction {
    #[serde(default)]
    pub source_path: String,
    #[serde(default)]
    pub auto_restore: bool,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub is_game: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct Config {
    #[serde(default, rename = "Junctions")]
    pub junctions: Vec<Junction>,
    #[serde(default = "default_interval", rename = "SyncIntervalMinutes")]
    pub sync_interval_minutes: i32,
    #[serde(default, rename = "ExcludedNames")]
    pub excluded_names: Vec<String>,
    #[serde(default = "default_max_versions", rename = "MaxVersions")]
    pub max_versions: i32,
}

fn default_interval() -> i32 { 1440 }
fn default_max_versions() -> i32 { 2 }

impl Default for Config {
    fn default() -> Self {
        Config {
            junctions: vec![],
            sync_interval_minutes: 1440,
            excluded_names: vec![],
            max_versions: 2,
        }
    }
}

pub fn script_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    exe.parent().unwrap_or(std::path::Path::new(".")).to_path_buf()
}

/// Internal state directory — all config/log/marker files live here.
/// Hidden folder (.lrgex), auto-created on first access.
pub fn data_dir() -> PathBuf {
    let d = script_dir().join(".lrgex");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// One-time migration: move scattered root files into .lrgex/ subfolder.
pub fn migrate_to_data_dir() {
    let dd = data_dir();
    let sd = script_dir();
    let moves = [
        ("junction-config.json", "junction-config.json"),
        ("sync.log", "sync.log"),
        ("sync-progress.txt", "sync-progress.txt"),
        ("sync-status.json", "sync-status.json"),
        (".lrgex-home", "home"),
        (".legacy-tasks-cleaned", "legacy-cleaned"),
        (".migration-pending", "migration-pending"),
    ];
    for (old, new) in &moves {
        let old_path = sd.join(old);
        let new_path = dd.join(new);
        if old_path.exists() && !new_path.exists() {
            let _ = std::fs::rename(&old_path, &new_path);
        }
    }
}

pub fn config_path() -> PathBuf {
    data_dir().join("junction-config.json")
}

/// Save config with path contraction (absolute → portable).
pub fn save_config(cfg: &Config) -> bool {
    // L5/M-T2: returns success AND logs on failure — a silently-dropped
    // junction (full disk, OneDrive lock) was invisible to the user.
    let path = config_path();
    let mut cfg = cfg.clone();
    for j in &mut cfg.junctions {
        j.source_path = crate::pathutil::contract(&j.source_path);
    }
    match serde_json::to_string_pretty(&cfg) {
        Ok(data) => {
            if std::fs::write(&path, data).is_ok() {
                true
            } else {
                crate::synclog::write("[CONFIG-FAIL] could not write junction-config.json — change NOT saved");
                false
            }
        }
        Err(_) => {
            crate::synclog::write("[CONFIG-FAIL] could not serialize config — change NOT saved");
            false
        }
    }
}

/// Load config with path expansion and healing.
pub fn load_config() -> Config {
    let path = config_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(data) => data.strip_prefix('\u{feff}').unwrap_or(&data).to_string(),
        Err(_) => return Config::default(),
    };

    let mut cfg: Config = serde_json::from_str(&raw).unwrap_or_default();
    let mut needs_save = false;
    let user_profile = std::env::var("USERPROFILE").unwrap_or_default();
    let current_user = user_profile.rsplit(std::path::MAIN_SEPARATOR).next().unwrap_or("").to_lowercase();

    for j in &mut cfg.junctions {
        j.source_path = crate::pathutil::expand(&j.source_path);
        let lower = j.source_path.to_lowercase();
        let sep = std::path::MAIN_SEPARATOR;
        // L4: heal on the ACTUAL system drive (%SystemDrive%, not hardcoded c:) —
        // Windows installed on D:/E: previously never healed after a reinstall.
        let sys_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let prefix = format!("{}{}users{}", sys_drive.to_lowercase(), sep, sep);
        if lower.starts_with(&prefix) && !current_user.is_empty() {
            let after_prefix = &j.source_path[prefix.len()..];
            if let Some(bs) = after_prefix.find(sep) {
                let old_user = after_prefix[..bs].to_lowercase();
                if old_user != current_user && old_user != "public" {
                    let suffix = &after_prefix[bs..];
                    let healed = format!("{}{}", user_profile, suffix);
                    crate::synclog::write(&format!("  [HEAL] {} -> {}", j.source_path, healed));
                    j.source_path = healed;
                    needs_save = true;
                }
            }
        }
    }

    let has_absolute = raw.contains("SourcePath") && raw.contains(":\\");
    if needs_save || has_absolute {
        // Idempotent migrate: only write if contraction actually changes the content.
        // (Otherwise un-contractable paths like E:\ re-save on every load_config call,
        //  flipping the file mtime and looping any mtime-watching caller.)
        let mut contracted = cfg.clone();
        for j in &mut contracted.junctions {
            j.source_path = crate::pathutil::contract(&j.source_path);
        }
        if let Ok(new_raw) = serde_json::to_string_pretty(&contracted) {
            if new_raw != raw {
                crate::synclog::write("  [MIGRATE] Saving portable config");
                let _ = std::fs::write(&path, new_raw);
            }
        }
    }

    cfg
}

pub fn is_home() -> bool {
    // Check WITHOUT creating .lrgex (prevents stray folder on Desktop/Downloads).
    script_dir().join(".lrgex").join("home").exists()
}

const REG_PATH: &str = r"SOFTWARE\LRGEX\Restore";

pub fn canonical_home() -> Option<PathBuf> {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(REG_PATH)
        .ok()
        .and_then(|k| k.get_value::<String, _>("HomePath").ok())
        .map(PathBuf::from)
}

pub fn set_canonical_home(path: &Path) {
    if let Ok(key) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .create_subkey(REG_PATH) {
        let _ = key.0.set_value("HomePath", &path.to_string_lossy().to_string());
    }
}

pub fn clear_canonical_home() {
    if let Ok(parent) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey_with_flags(r"SOFTWARE\LRGEX", winreg::enums::KEY_WRITE) {
        let _ = parent.delete_subkey_all("Restore");
    }
}

#[allow(dead_code)]
pub fn pair_cloud_path(source: &str) -> PathBuf {
    let leaf = std::path::Path::new(source)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    script_dir().join(&leaf)
}

pub fn trash_base() -> PathBuf {
    script_dir().join("_versions")
}

pub fn ensure_versions_setup() {
    let versions = trash_base();
    if !versions.exists() {
        let old = script_dir().join("_trash");
        if old.exists() {
            let _ = std::fs::rename(&old, &versions);
        }
    }
    if versions.exists() {
        use std::os::windows::fs::MetadataExt;
        let already_set = std::fs::metadata(&versions)
            .map(|m| m.file_attributes() & 0x6 == 0x6)
            .unwrap_or(false);
        if !already_set {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("attrib")
                .args(["+H", "+S", versions.to_str().unwrap_or("")])
                .creation_flags(0x08000000u32)
                .spawn();
        }
    }
}

// ==================== PAIR IDENTITY (C2) ====================
// Backup identity is leaf + short hash of the full (lowercased) source path.
// Two junctions named "Saves" in different games are DIFFERENT pairs — the old
// leaf-only keys made them overwrite each other's backups and restore the
// wrong game's data into the other's folder.
fn fnv1a_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.to_lowercase().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", (h & 0xFFFF_FFFF) as u32) // 8 hex chars — display-friendly
}

/// L2: true when two source paths refer to the SAME folder — compares the
/// EXPANDED, case-folded, trailing-slash-trimmed forms, so contracted config
/// paths (%APPDATA%\Foo) match picker-expanded ones (C:\Users\X\...\Foo).
pub fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| crate::pathutil::expand(p).trim_end_matches(['\\', '/']).to_lowercase();
    norm(a) == norm(b)
}
/// Hash base is the CONTRACTED form (pathutil::contract) — stable across
/// username changes (the app's core purpose). contract() is idempotent on
/// already-contracted input (contracting a contracted path is identity).
pub fn pair_key(source: &str) -> String {
    let contracted = crate::pathutil::contract(source);
    let leaf = match Path::new(&contracted).file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => "pair".into(),
    };
    format!("{}_{}", leaf, fnv1a_hex(&contracted))
}

/// One-time migration: if the old leaf-only dir exists and the new keyed dir
/// does not, rename DIR + rename the archive/sidecar FILES inside (they keep
/// their old <leaf>.* names — the new code expects <key>.*). Idempotent — a
/// no-op once migrated. Runs from sync AND restore paths (fresh reinstall may
/// only ever run restore, so both entry points must migrate).
pub fn migrate_pair_key(source: &str) {
    let leaf = match Path::new(source).file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => return,
    };
    let key = pair_key(source);
    if key == leaf { return; }
    let old_dir = script_dir().join("backup").join(&leaf);
    let new_dir = script_dir().join("backup").join(&key);
    if old_dir.is_dir() && !new_dir.exists() {
        if std::fs::rename(&old_dir, &new_dir).is_ok() {
            // Rename files inside: <leaf>.tar.zst -> <key>.tar.zst, same for .size
            let old_arch = new_dir.join(format!("{}.tar.zst", leaf));
            let new_arch = new_dir.join(format!("{}.tar.zst", key));
            if old_arch.exists() { let _ = std::fs::rename(&old_arch, &new_arch); }
            let old_side = new_dir.join(format!("{}.tar.zst.size", leaf));
            let new_side = new_dir.join(format!("{}.tar.zst.size", key));
            if old_side.exists() { let _ = std::fs::rename(&old_side, &new_side); }
        }
    }
    // Same for versions
    let old_v = trash_base().join(&leaf);
    let new_v = trash_base().join(&key);
    if old_v.is_dir() && !new_v.exists() {
        let _ = std::fs::rename(&old_v, &new_v);
    }
}

// C-1: names the app itself owns under script_dir() — a legacy raw-folder can
// NEVER be one of these, and a user source named "backup" must not make the
// migration path compress-and-delete the app's own backup store.
const RESERVED_LEAVES: &[&str] = &["backup", "_versions", ".lrgex"];

/// C-1: resolves the pre-C2 raw backup folder for a source, SAFELY.
/// Returns None for reserved names (the app's own store) and for folders that
/// contain the app's own structure (i.e. the home itself).
pub fn legacy_raw_folder(source: &str) -> Option<PathBuf> {
    let leaf = Path::new(source).file_name()?.to_string_lossy().to_string();
    if RESERVED_LEAVES.iter().any(|r| r.eq_ignore_ascii_case(&leaf)) {
        return None; // never touch our own store via the legacy path
    }
    let p = script_dir().join(&leaf);
    // A genuine pre-C2 raw backup never contains our own store dirs.
    if p.join("backup").is_dir() || p.join(".lrgex").is_dir() || p.join("_versions").is_dir() {
        return None;
    }
    if p.is_dir() { Some(p) } else { None }
}

pub fn trash_path_for(source: &str) -> PathBuf {
    trash_base().join(pair_key(source))
}

pub fn backup_dir_for(source: &str) -> PathBuf {
    script_dir().join("backup").join(pair_key(source))
}

pub fn backup_file_for(source: &str) -> PathBuf {
    let key = pair_key(source);
    script_dir().join("backup").join(&key).join(format!("{}.tar.zst", key))
}

pub fn sidecar_for(source: &str) -> PathBuf {
    let key = pair_key(source);
    script_dir().join("backup").join(&key).join(format!("{}.tar.zst.size", key))
}
