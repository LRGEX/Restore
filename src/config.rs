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
    /// v1.7: stable volume GUID of the junction's drive ("{GUID}") — recorded
    /// so a drive-LETTER reshuffle after a format can be healed automatically.
    /// None in pre-v1.7 configs (legacy relative-path fallback applies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_id: Option<String>,
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
/// INTERVAL-GUARD marker: written ONLY at successful completion of a full sync
/// cycle. A scheduled -sync that finds a completed sync younger than the
/// configured interval exits early — protects a 1-minute interval from firing
/// while/after a 4-minute mega-folder backup (the lock already serializes
/// concurrent runs; this stops the pile-up of waiting task instances).
/// Deliberately NOT sync-status.json — that file is rewritten constantly by
/// progress writes and manual/GUI operations; a marker from it would suppress
/// scheduled syncs after every manual backup.
pub fn last_sync_marker_path() -> PathBuf {
    data_dir().join("last-sync-complete")
}

pub fn write_last_sync_marker() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let _ = std::fs::write(last_sync_marker_path(), secs.to_string());
}

/// True if a completed sync is younger than `interval_minutes` (grace 1 min).
pub fn sync_completed_recently(interval_minutes: i64) -> bool {
    let age = std::fs::read_to_string(last_sync_marker_path())
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|then| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|now| now.as_secs().saturating_sub(then))
                .unwrap_or(u64::MAX)
        })
        .unwrap_or(u64::MAX);
    age < interval_minutes.max(1) as u64 * 60 + 60
}

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
        // v1.7: record the volume GUID from the EXPANDED path before contracting.
        // Only overwrite on SUCCESS — a temporarily-unplugged drive must not
        // wipe the stored GUID (it's needed exactly when the drive returns
        // with a different letter).
        let expanded = crate::pathutil::expand(&j.source_path);
        if let Some(letter) = drive_letter(&expanded) {
            if let Some(g) = volume_guid(&letter) {
                j.volume_id = Some(g);
            }
        }
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

        // v1.7 DRIVE HEALING: drive letters reshuffle after a format (E:→D:).
        // Volume GUIDs don't. Heal + re-key the backup so it follows the path.
        {
            let stored = j.volume_id.clone();
            if let Some(healed) = heal_drive_letter(&j.source_path, stored.as_deref()) {
                rekey_backup(&j.source_path, &healed);
                crate::synclog::write(&format!(
                    "  [HEAL-DRIVE] {} -> {}", j.source_path, healed));
                j.source_path = healed;
                needs_save = true;
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
/// ROOT-CAUSE GUARD (the Saved Games incident): every string that becomes a
/// Windows path component passes through here. Replaces illegal chars
/// (<>:"/\|?* and control chars), strips trailing dots/spaces, and neutralizes
/// reserved device names (CON, NUL, COM1…). A token leak like
/// %KNOWNFOLDER:SavedGames% (colon!) can never produce an invalid path again.
pub fn safe_filename(name: &str) -> String {
    let mut s: String = name.chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    while s.ends_with('.') || s.ends_with(' ') { s.pop(); }
    // Reserved device names — bare or with an extension stem
    let lower = s.to_lowercase();
    let stem = lower.split('.').next().unwrap_or("");
    let reserved = matches!(stem, "con" | "prn" | "aux" | "nul")
        || (stem.len() >= 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem[3..].parse::<u32>().map(|n| (1..=9).contains(&n)).unwrap_or(false));
    if reserved { s.push_str("_lrgex"); }
    if s.is_empty() { s = "pair".into(); }
    s
}

/// Collision-proof + reinstall-stable storage key for a junction source path.
/// Hash base is the CONTRACTED form (pathutil::contract) — stable across
/// username changes (the app's core purpose). The DISPLAY leaf comes from the
/// EXPANDED path: a junction that IS a known-folder root (e.g. Saved Games →
/// %KNOWNFOLDER:SavedGames%) has no trailing component, and taking the leaf
/// from the token leaked a COLON into the folder name — Windows rejects it.
pub fn pair_key(source: &str) -> String {
    let contracted = crate::pathutil::contract(source);
    let leaf = match Path::new(source).file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => "pair".into(),
    };
    format!("{}_{}", safe_filename(&leaf), fnv1a_hex(&contracted))
}

/// One-time migration: if the old leaf-only dir exists and the new keyed dir
/// does not, rename DIR + rename the archive/sidecar FILES inside (they keep
/// their old <leaf>.* names — the new code expects <key>.*). Idempotent — a
/// no-op once migrated. Runs from sync AND restore paths (fresh reinstall may
/// only ever run restore, so both entry points must migrate).
/// v1.6.1: >1 configured pair sharing a leaf name makes the leaf dir
/// AMBIGUOUS — migration/cleanup must not touch it (could be another pair's
/// only backup). Returns true when the leaf is exclusively this source's.
fn leaf_is_unique(source: &str, leaf: &str) -> bool {
    let cfg = load_config();
    let same = cfg.junctions.iter()
        .filter(|j| Path::new(&j.source_path).file_name()
            .map(|n| n.to_string_lossy().to_string())
            .as_deref() == Some(leaf))
        .count();
    let unique = same <= 1;
    if !unique {
        crate::synclog::write(&format!(
            "  [MIGRATE-SKIP] leaf '{}' is shared by {} pairs — leaving legacy dir untouched", leaf, same));
    }
    unique
}

pub fn migrate_pair_key(source: &str) {
    let leaf = match Path::new(source).file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => return,
    };
    let key = pair_key(source);
    if key == leaf { return; }
    if !leaf_is_unique(source, &leaf) { return; } // ambiguous — never touch
    let old_dir = script_dir().join("backup").join(&leaf);
    let new_dir = script_dir().join("backup").join(&key);
    if old_dir.is_dir() && !new_dir.exists() {
        match std::fs::rename(&old_dir, &new_dir) {
            Ok(()) => {
                // Rename files inside: <leaf>.tar.zst -> <key>.tar.zst, same for .size
                let old_arch = new_dir.join(format!("{}.tar.zst", leaf));
                let new_arch = new_dir.join(format!("{}.tar.zst", key));
                if old_arch.exists() { let _ = std::fs::rename(&old_arch, &new_arch); }
                let old_side = new_dir.join(format!("{}.tar.zst.size", leaf));
                let new_side = new_dir.join(format!("{}.tar.zst.size", key));
                if old_side.exists() { let _ = std::fs::rename(&old_side, &new_side); }
            }
            Err(e) => {
                // v1.6.1: NEVER silent — a failed migration is retried on every
                // sync until OneDrive releases the folder.
                crate::synclog::write(&format!(
                    "  [MIGRATE-RETRY] rename {} -> {} failed ({}): will retry next sync",
                    leaf, key, e));
            }
        }
    }
    // Same for versions
    let old_v = trash_base().join(&leaf);
    let new_v = trash_base().join(&key);
    if old_v.is_dir() && !new_v.exists() {
        if let Err(e) = std::fs::rename(&old_v, &new_v) {
            crate::synclog::write(&format!(
                "  [MIGRATE-RETRY] versions rename {} failed ({}): will retry next sync", leaf, e));
        }
    }
}

/// v1.6.1 ORPHAN CLEANUP: when a migration rename failed (OneDrive lock), the
/// sync created the keyed folder and the old leaf dir was abandoned forever —
/// duplicate backups eating cloud space. This removes the superseded leaf dir
/// when (and only when) the live keyed twin exists AND the leaf dir looks like
/// OUR backup (contains <leaf>.tar.zst) — never touches unknown/user folders.
/// Idempotent + retried every sync until it succeeds.
pub fn cleanup_pair_orphan(source: &str) {
    let leaf = match Path::new(source).file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => return,
    };
    let key = pair_key(source);
    if key == leaf { return; }
    if !leaf_is_unique(source, &leaf) { return; } // ambiguous — never delete
    // Safety gate: the keyed twin must hold a COMPLETE backup (archive + sidecar
    // marker) — a partial keyed dir must never justify deleting the leaf copy.
    let keyed_dir = script_dir().join("backup").join(&key);
    let live = keyed_dir.join(format!("{}.tar.zst", key)).exists()
        && keyed_dir.join(format!("{}.tar.zst.size", key)).exists();
    if !live { return; }
    let orphan = script_dir().join("backup").join(&leaf);
    if orphan.is_dir() {
        // Safety: only delete what is provably ours
        let is_ours = orphan.join(format!("{}.tar.zst", leaf)).exists()
            || orphan.join(format!("{}.tar.zst.size", leaf)).exists();
        if is_ours {
            match std::fs::remove_dir_all(&orphan) {
                Ok(()) => crate::synclog::write(&format!(
                    "  [CLEANUP] removed superseded backup folder '{}' (live: {})", leaf, key)),
                Err(e) => crate::synclog::write(&format!(
                    "  [CLEANUP-RETRY] removing old '{}' failed ({}): will retry next sync", leaf, e)),
            }
        }
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

/// Stable volume identity of a drive root — the partition's GUID survives
/// drive-LETTER reshuffles (format C: → E: becomes D:), because the data
/// partition itself is untouched. Returns "{GUID}" form, or None.
#[cfg(target_os = "windows")]
pub fn volume_guid(letter_root: &str) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeNameForVolumeMountPointW;
    use std::os::windows::ffi::OsStrExt;
    let root = format!("{}\\", letter_root.trim_end_matches('\\')); // API needs trailing backslash
    let wide: Vec<u16> = std::ffi::OsStr::new(&root).encode_wide().chain(std::iter::once(0)).collect();
    let mut buf = [0u16; 100];
    let ok = unsafe {
        GetVolumeNameForVolumeMountPointW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32)
    };
    if ok == 0 { return None; }
    let s = String::from_utf16_lossy(&buf);
    match (s.find('{'), s.find('}')) {
        (Some(a), Some(b)) if b > a => Some(s[a..=b].to_string()),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
pub fn volume_guid(_letter_root: &str) -> Option<String> { None }

/// "E:" from "E:\anything" (drive-letter paths only).
fn drive_letter(path: &str) -> Option<String> {
    let b = path.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        Some(path[..2].to_string())
    } else {
        None
    }
}

/// Find which drive letter currently hosts the given volume GUID.
/// `lookup` is injectable for tests.
fn find_guid_letter(guid: &str, lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    for l in b'A'..=b'Z' {
        let letter = format!("{}:", l as char);
        if lookup(&letter).as_deref() == Some(guid) {
            return Some(letter);
        }
    }
    None
}

/// v1.7 DRIVE HEALING: called from load_config for each junction with a
/// non-system (non-contractable) absolute path. Three cases:
///   1. Stored GUID, letter missing or reassigned → find GUID on another letter → heal + re-key.
///   2. No stored GUID (pre-v1.7 config) + drive missing → LEGACY fallback:
///      scan other drives for the SAME relative path — accept only if exactly one match.
///   3. Nothing found → leave untouched, log clearly (GUI Repair tool handles it).
fn heal_drive_letter(source: &str, stored_guid: Option<&str>) -> Option<String> {
    let letter = drive_letter(source)?; // only drive-letter paths
    if source.len() < 4 { return None; } // bare "E:" — nothing relative to keep
    let rel = &source[3..]; // after "X:\"
    let current = volume_guid(&letter);
    let ok = match stored_guid {
        Some(g) => current.as_deref() == Some(g), // GUID matches → healthy
        // No stored GUID (pre-v1.7 config): only heal when the path is GONE
        // (letter vanished or reassigned to a different disk).
        None => std::path::Path::new(source).exists(),
    };
    if ok { return None; }

    // Case 1: GUID hunt
    if let Some(g) = stored_guid {
        if let Some(new_letter) = find_guid_letter(g, volume_guid) {
            return Some(format!("{}\\{}", new_letter, rel));
        }
        crate::synclog::write(&format!(
            "  [HEAL-DRIVE-FAIL] volume {} of '{}' not found on any drive", g, source));
        return None;
    }

    // Case 2: legacy — same relative path on exactly one OTHER drive
    let mut hits = Vec::new();
    for l in b'A'..=b'Z' {
        let letter = format!("{}:", l as char);
        if letter.eq_ignore_ascii_case(&letter_of(source)) { continue; }
        if volume_guid(&letter).is_none() { continue; } // drive exists?
        let candidate = format!("{}\\{}", letter, rel);
        if std::path::Path::new(&candidate).is_dir() {
            hits.push(candidate);
        }
    }
    if hits.len() == 1 {
        crate::synclog::write(&format!(
            "  [HEAL-DRIVE-LEGACY] unique relative-path match: {} → {}", source, hits[0]));
        return Some(hits[0].clone());
    }
    if hits.len() > 1 {
        crate::synclog::write(&format!(
            "  [HEAL-DRIVE-FAIL] '{}' ambiguous — matches multiple drives; use Repair Paths", source));
    }
    None
}

fn letter_of(path: &str) -> String {
    drive_letter(path).unwrap_or_default()
}

/// Rename a keyed backup dir (and versions dir) from one pair key to another —
/// shared by leaf→key migration and drive-letter re-keying. Idempotent.
fn migrate_pair_dir(old_key: &str, new_key: &str) {
    if old_key == new_key { return; }
    let old_dir = script_dir().join("backup").join(old_key);
    let new_dir = script_dir().join("backup").join(new_key);
    if old_dir.is_dir() && !new_dir.exists() {
        if std::fs::rename(&old_dir, &new_dir).is_ok() {
            for (from, to) in [
                (new_dir.join(format!("{}.tar.zst", old_key)), new_dir.join(format!("{}.tar.zst", new_key))),
                (new_dir.join(format!("{}.tar.zst.size", old_key)), new_dir.join(format!("{}.tar.zst.size", new_key))),
            ] {
                if from.exists() { let _ = std::fs::rename(&from, &to); }
            }
        }
    }
    let old_v = trash_base().join(old_key);
    let new_v = trash_base().join(new_key);
    if old_v.is_dir() && !new_v.exists() {
        let _ = std::fs::rename(&old_v, &new_v);
    }
}

/// DRIVE RE-KEY: after a healed path changes its drive letter, the pair key
/// (hash of the contracted path) changes — move the backup so it follows.
pub fn rekey_backup(old_source: &str, new_source: &str) {
    migrate_pair_dir(&pair_key(old_source), &pair_key(new_source));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// THE regression: a junction that IS a known-folder root (Saved Games)
    /// leaked its token — and its COLON — into the folder name, which Windows
    /// rejects. The key must derive its leaf from the EXPANDED path and NEVER
    /// contain an illegal filename character.
    #[test]
    fn test_pair_key_known_folder_root_no_illegal_chars() {
        // Real-world shape from the incident (expanded path of the token):
        let key = pair_key(r"C:\Users\lrg4you\Saved Games");
        assert!(key.starts_with("Saved Games_"), "leaf must be the real folder name: {}", key);
        assert!(!key.contains(':'), "colon must never appear in a key: {}", key);
        assert!(!key.contains('\\') && !key.contains('/'), "no separators: {}", key);
    }

    /// CLASS GUARD: any derived key is a legal Windows path component.
    #[test]
    fn test_pair_key_always_legal_filename() {
        for src in [
            r"C:\Users\x\Saved Games",
            r"C:\Users\x\hermes",
            r"D:\Games\weird?name<>",
            r"E:\a|b*c",
            "%KNOWNFOLDER:SavedGames%",          // raw token passed as source
            "%LOCALAPPDATA%\\app dir",
            r"C:\",                                // drive root — no leaf
        ] {
            let key = pair_key(src);
            assert!(!key.is_empty());
            for c in key.chars() {
                assert!(
                    !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'),
                    "illegal char {:?} in key {} for source {}", c, key, src
                );
                assert!((c as u32) >= 0x20, "control char in key {} for {}", key, src);
            }
            assert!(!key.ends_with('.') && !key.ends_with(' '), "trailing dot/space: {} for {}", key, src);
        }
    }

    #[test]
    fn test_safe_filename_matrix() {
        assert_eq!(safe_filename("%KNOWNFOLDER:SavedGames%"), "%KNOWNFOLDER_SavedGames%");
        assert_eq!(safe_filename("a/b\\c"), "a_b_c");
        assert_eq!(safe_filename("trailing.. "), "trailing");
        assert_eq!(safe_filename("CON"), "CON_lrgex");
        assert_eq!(safe_filename("com1.txt"), "com1.txt_lrgex");
        assert!(safe_filename("").starts_with("pair"));
        assert_eq!(safe_filename("lpt9"), "lpt9_lrgex");
        assert_eq!(safe_filename("lpt10"), "lpt10"); // 10+ not reserved
    }

    /// Stability contract: the same folder under two usernames contracts to
    /// the same token → same hash → same key (restore-after-reinstall works).
    /// Simulated without env access: contract() leaves non-matching paths as-is.
    #[test]
    fn test_pair_key_deterministic() {
        let a = pair_key(r"E:\Games\MySaves");
        let b = pair_key(r"E:\Games\MySaves");
        let c = pair_key(r"E:\Games\OtherSaves");
        assert_eq!(a, b, "same source must produce the same key");
        assert_ne!(a, c, "different sources must not collide");
    }

    #[test]
    fn test_drive_letter_parse() {
        assert_eq!(drive_letter(r"E:\Steam\KSP"), Some("E:".into()));
        assert_eq!(drive_letter(r"C:\"), Some("C:".into()));
        assert_eq!(drive_letter(r"E:"), Some("E:".into()));
        assert_eq!(drive_letter(r"\\\\?\\Volume{X}\\path"), None);
        assert_eq!(drive_letter("relative/path"), None);
    }

    #[test]
    fn test_find_guid_letter_scan() {
        let lookup = |l: &str| if l == "D:" { Some("{ABCD}".to_string()) } else { None };
        assert_eq!(find_guid_letter("{ABCD}", &lookup), Some("D:".to_string()));
        assert_eq!(find_guid_letter("{MISSING}", &lookup), None);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_volume_guid_c_drive() {
        let g = volume_guid("C:");
        assert!(g.is_some(), "C: should have a volume GUID");
        let g = g.unwrap();
        assert!(g.starts_with('{') && g.ends_with('}'));
        assert_eq!(volume_guid("C:"), Some(g), "GUID must be stable across calls");
    }

    #[test]
    fn test_drive_rekey_migration() {
        let old_key = pair_key(r"E:\Games\TestRekey");
        let new_key = pair_key(r"D:\Games\TestRekey");
        assert_ne!(old_key, new_key);
        let old_dir = script_dir().join("backup").join(&old_key);
        let new_dir = script_dir().join("backup").join(&new_key);
        let _ = std::fs::remove_dir_all(&old_dir);
        let _ = std::fs::remove_dir_all(&new_dir);
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join(format!("{}.tar.zst", old_key)), b"x").unwrap();
        migrate_pair_dir(&old_key, &new_key);
        assert!(new_dir.join(format!("{}.tar.zst", new_key)).exists(), "archive must follow the re-key");
        assert!(!old_dir.exists());
        let _ = std::fs::remove_dir_all(&new_dir);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_legacy_heal_no_guid() {
        // Pre-v1.7 config (no stored GUID): dead letter + the SAME relative
        // path on exactly one other drive → legacy fallback heals.
        // Uses the real H: disk (same constraint as the e2e test).
        if volume_guid("H:").is_none() { return; } // disk absent — skip
        let dir = r"H:\LRGEX-HealTest\Saves";
        std::fs::create_dir_all(dir).ok();
        let healed = heal_drive_letter(r"X:\LRGEX-HealTest\Saves", None);
        assert_eq!(healed.as_deref(), Some(dir), "legacy fallback must find the H: match");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_e2e_drive_heal_real_guid() {
        // REAL end-to-end through load_config: junction on a dead letter (X:),
        // volume_id = the REAL GUID of H: (this machine has H:), backup keyed
        // by the old path. load_config must heal X:→H: AND re-key the backup.
        // Donor drive: any existing non-system letter (H: may be renamed mid-session
        // — the physical test moves it to X:!). Prefer fixed data drives.
        // Donor: the letter that ACTUALLY hosts LRGEX-HealTest\Saves (the
        // physical-test flash moves around — H: became X: mid-session!).
        // Self-contained: pick any live non-system drive, CREATE the folder
        // (no dependency on the physical-test flash, which moves/cleans up).
        let candidates = ["E:", "F:", "D:", "T:", "H:", "X:"];
        let donor = candidates.iter().find(|l| volume_guid(l).is_some() && *l != &"C:".to_string())
            .copied().expect("test machine must have a second drive");
        let sep = std::path::MAIN_SEPARATOR;
        let live_dir = format!("{}{}lrgex_e2e_heal{}Saves", donor, sep, sep);
        std::fs::create_dir_all(&live_dir).ok();
        std::fs::write(std::path::Path::new(&live_dir).join("seed.sav"), b"x").ok();
        // Dead letter: any letter with no volume at all (runtime scan).
        let dead = (b'A'..=b'Z').map(|c| format!("{}:", c as char))
            .find(|l| volume_guid(l).is_none() && l != &donor)
            .expect("a free drive letter must exist");
        let h_guid = volume_guid(donor).unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        let old_path = format!("{}{}LRGEX-HealTest{}Saves", dead, sep, sep);

        // Seed the config (test binary's script_dir)
        let cfg_path = config_path();
        let backup = std::fs::read_to_string(&cfg_path).unwrap_or_default();
        let seeded = serde_json::json!({
            "Junctions": [{
                "SourcePath": old_path,
                "AutoRestore": false,
                "Created": "test",
                "IsGame": false,
                "VolumeId": h_guid,
            }],
            "SyncIntervalMinutes": 1440,
            "MaxVersions": 2,
            "ExcludedNames": []
        });
        std::fs::write(&cfg_path, seeded.to_string()).unwrap();

        // Old-keyed backup dir + archive
        let old_key = pair_key(&old_path);
        let old_dir = script_dir().join("backup").join(&old_key);
        let _ = std::fs::remove_dir_all(&old_dir);
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join(format!("{}.tar.zst", old_key)), b"fake-archive").unwrap();

        // HEAL: the exact function the app runs at every startup
        let cfg = load_config();

        let healed = cfg.junctions[0].source_path.clone();
        assert!(healed.starts_with(&format!("{}\\", donor)), "junction must heal to {}, got: {}", donor, healed);
        assert!(healed.ends_with("LRGEX-HealTest\\Saves"), "relative path preserved: {}", healed);

        // Backup followed the re-key
        let new_key = pair_key(&healed);
        let new_dir = script_dir().join("backup").join(&new_key);
        assert!(new_dir.join(format!("{}.tar.zst", new_key)).exists(),
            "backup archive must follow the healed path");
        assert!(!old_dir.exists(), "old-keyed dir must be gone");

        // Cleanup: restore original config (if any) + test dirs
        if backup.is_empty() { let _ = std::fs::remove_file(&cfg_path); }
        else { let _ = std::fs::write(&cfg_path, &backup); }
        let _ = std::fs::remove_dir_all(&new_dir);
        let _ = std::fs::remove_dir_all(&live_dir);
    }
}
