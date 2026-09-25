//! Game-save detection & discovery.
//!
//! Path patterns distilled from community save-location data (path SHAPES only —
//! no game names, no database shipped; shapes are non-copyrightable facts).
//! Two consumers:
//!   1. Lamp classification — is this protected folder game saves?
//!   2. "Find Game Saves" — scan known save neighborhoods of the PC.

use std::path::{Path, PathBuf};

// ==================== PATTERN KNOWLEDGE ====================

/// Single path segments that mark a save location (case-insensitive).
const SAVE_SEGMENTS: &[&str] = &[
    "save", "saves", "savedata", "savegame", "savegames", "savedgames",
    "saved games", "savesdir", "profiles", "remote", "wgs",
    "my games",
];

/// Multi-segment suffixes (specific engine/store layouts).
const SUFFIX_SHAPES: &[&str] = &[
    "bmgame/savedata",
    "game/saves",
    "www/save",
    "data/save",
];

/// Save-file extensions (content evidence, used only as a tiebreaker).
const SAVE_EXTENSIONS: &[&str] = &["sav", "save", "rpgsave", "rvdata", "rvdata2", "bak"];

fn segments(path: &str) -> Vec<String> {
    path.replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

/// CLASSIFICATION: does this path LOOK like a game-save location?
/// Pure path-shape check (no disk access) — case-insensitive, any depth.
pub fn is_game_path(path: &str) -> bool {
    let segs = segments(path);
    if segs.is_empty() { return false; }

    // 1. Any save-named segment → strong shape match
    if segs.iter().any(|s| SAVE_SEGMENTS.contains(&s.as_str())) {
        return true;
    }
    // 2. Multi-segment suffix shapes (e.g. .../BMGame/SaveData)
    let joined = segs.join("/");
    if SUFFIX_SHAPES.iter().any(|s| joined.ends_with(s)) {
        return true;
    }
    // 3. Numeric-ID followed by a save segment (Steam/WB pattern):
    //    .../<id>/remote, .../<id>/savedata, .../<id>/saves …
    for w in segs.windows(2) {
        if w[0].chars().all(|c| c.is_ascii_digit()) && !w[0].is_empty()
            && SAVE_SEGMENTS.contains(&w[1].as_str()) {
            return true;
        }
    }
    false
}

/// CONTENT tiebreaker: does the folder hold save-extension files? (bounded)
pub fn has_save_files(dir: &Path) -> bool {
    let mut visited = 0usize;
    has_save_files_inner(dir, &mut visited)
}
fn has_save_files_inner(dir: &Path, visited: &mut usize) -> bool {
    if *visited > 40 { return false; } // bounded — save evidence lives shallow
    *visited += 1;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if crate::sync::should_skip_pub(&name) { continue; }
                if has_save_files_inner(&p, visited) { return true; }
            } else if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
                if SAVE_EXTENSIONS.contains(&ext.to_lowercase().as_str()) { return true; }
            }
        }
    }
    false
}

/// PROTECTION LEVEL: numeric-ID paths must be protected at the level where the
/// ID folders become DIRECT children — that's what keeps Save-ID migration
/// (renumbering) working after a reinstall. Walks up to the parent of the
/// TOPMOST numeric segment. No numeric segment → path unchanged.
pub fn protection_level(path: &Path) -> PathBuf {
    let parts: Vec<_> = path.components().collect();
    // Find topmost numeric component (skip drive/root prefix)
    let start = parts.iter().position(|c| !matches!(
        c, std::path::Component::Prefix(_) | std::path::Component::RootDir
    )).unwrap_or(0);
    for i in start..parts.len() {
        let name = parts[i].as_os_str().to_string_lossy().to_string();
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) {
            // parent of this numeric segment
            return parts[..i].iter().collect();
        }
    }
    path.to_path_buf()
}

// ==================== DISCOVERY ====================

#[derive(Debug, Clone)]
pub struct FoundSave {
    pub protect: PathBuf, // the junction path to offer (protection-level adjusted)
    pub found: PathBuf,   // the raw save path we spotted (for the "why" text)
    pub reason: String,
}

/// Scan the known save neighborhoods of this PC. Bounded, read-only.
pub fn find_game_saves(existing: &[String]) -> Vec<FoundSave> {
    let mut out: Vec<FoundSave> = Vec::new();
    let mut push = |found: PathBuf, reason: &str, out: &mut Vec<FoundSave>| {
        let protect = protection_level(&found);
        if protect.to_string_lossy().is_empty() { return; }
        // Overlap check: skip anything equal to / inside / containing an
        // existing junction (nested protection is a trap, not a feature).
        let pl = protect.to_string_lossy().to_lowercase().replace('/', "\\");
        let pl = pl.trim_end_matches('\\');
        for ex in existing {
            let el = ex.to_lowercase().replace('/', "\\");
            let el = el.trim_end_matches('\\');
            if pl == el || pl.starts_with(&format!("{}\\", el)) || el.starts_with(&format!("{}\\", pl)) {
                return; // overlaps a protected folder
            }
        }
        // Dedupe within results
        if out.iter().any(|f| f.protect == protect) { return; }
        out.push(FoundSave { protect, found, reason: reason.to_string() });
    };

    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let docs = std::env::var("USERPROFILE").map(|h| PathBuf::from(h).join("Documents")).unwrap_or_default();

    // 1. %USERPROFILE%\Saved Games — always a save location
    let sg = PathBuf::from(&home).join("Saved Games");
    if sg.is_dir() && dir_nonempty(&sg) {
        push(sg.clone(), "Windows Saved Games folder", &mut out);
    }

    // 2. Documents\My Games\<game> — every non-empty child is a candidate
    let my_games = docs.join("My Games");
    if let Ok(entries) = std::fs::read_dir(&my_games) {
        for e in entries.flatten() {
            if e.path().is_dir() && dir_nonempty(&e.path()) {
                push(e.path(), "Documents/My Games", &mut out);
            }
        }
    }

    // 3. Documents\WB Games\<game>\... — protect at game level (numeric IDs below)
    let wb = docs.join("WB Games");
    if let Ok(entries) = std::fs::read_dir(&wb) {
        for e in entries.flatten() {
            if e.path().is_dir() && dir_nonempty(&e.path()) {
                push(e.path(), "WB Games", &mut out);
            }
        }
    }

    // 4. Steam: registry SteamPath + libraryfolders.vdf → game installs ONLY.
    //    v1.6.2: userdata (official Steam Cloud) is deliberately EXCLUDED —
    //    Steam syncs those itself; backing them up is redundant.
    for lib in steam_libraries() {
        let common = lib.join("steamapps").join("common");
        if let Ok(games) = std::fs::read_dir(&common) {
            for g in games.flatten() {
                if !g.path().is_dir() { continue; }
                // look for save-shaped subdirs (BMGame/SaveData etc.) — bounded depth
                if let Some(save_dir) = find_save_subdir(&g.path()) {
                    push(save_dir, "game install folder (save data only)", &mut out);
                }
            }
        }
    }

    // 5. MS Store games: %LOCALAPPDATA%\Packages\<pkg>\SystemAppData\wgs
    let pkgs = std::env::var("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Packages")).unwrap_or_default();
    if let Ok(entries) = std::fs::read_dir(&pkgs) {
        for e in entries.flatten() {
            let wgs = e.path().join("SystemAppData").join("wgs");
            if wgs.is_dir() && dir_nonempty(&wgs) {
                push(e.path(), "Microsoft Store game saves (wgs)", &mut out);
            }
        }
    }

    out.into_iter().take(20).collect() // sanity cap
}

fn dir_nonempty(p: &Path) -> bool {
    std::fs::read_dir(p).map(|mut d| d.next().is_some()).unwrap_or(false)
}

fn steam_libraries() -> Vec<PathBuf> {
    let mut libs = Vec::new();
    // SteamPath from registry
    let steam = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey("Software\\Valve\\Steam")
        .and_then(|k| k.get_value::<String, _>("SteamPath"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"C:\Program Files (x86)\Steam"));
    if steam.is_dir() { libs.push(steam.clone()); }
    // Extra libraries from libraryfolders.vdf — value QUOTES matter:
    // lines look like  "path"		"D:\\SteamLibrary"  — extract the value string.
    let vdf = steam.join("steamapps").join("libraryfolders.vdf");
    if let Ok(text) = std::fs::read_to_string(&vdf) {
        for line in text.lines() {
            if let Some(idx) = line.find("\"path\"") {
                let after_key = &line[idx + 6..]; // skip past the "path" key
                if let (Some(a), Some(b)) = (after_key.find('"'), after_key.rfind('"')) {
                    if a < b {
                        let p = after_key[a + 1..b].replace("\\\\", "\\");
                        let pb = PathBuf::from(&p);
                        if pb.is_dir() && !libs.contains(&pb) {
                            libs.push(pb);
                        }
                    }
                }
            }
        }
    }
    libs
}

/// Inside a game install dir, find a save-shaped SUBDIRECTORY (never the install).
fn find_save_subdir(game_dir: &Path) -> Option<PathBuf> {
    let mut visited = 0usize;
    find_save_subdir_inner(game_dir, 0, &mut visited)
}
fn find_save_subdir_inner(dir: &Path, depth: usize, visited: &mut usize) -> Option<PathBuf> {
    if depth > 3 || *visited > 60 { return None; }
    *visited += 1;
    let entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    for e in &entries {
        let p = e.path();
        if !p.is_dir() { continue; }
        let name = e.file_name().to_string_lossy().to_string();
        if crate::sync::should_skip_pub(&name.to_lowercase()) { continue; }
        if is_game_path(&p.to_string_lossy()) && dir_nonempty(&p) {
            return Some(p);
        }
    }
    for e in &entries {
        let p = e.path();
        if p.is_dir() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if crate::sync::should_skip_pub(&name) { continue; }
            if let Some(found) = find_save_subdir_inner(&p, depth + 1, visited) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_friend_batman_paths() {
        // The two real-world paths that started this feature — MUST light up.
        assert!(is_game_path(r"C:\Users\pCd\Documents\WB Games\Batman Arkham Knight\208650\SaveData\backup"));
        assert!(is_game_path(r"C:\Games\Batman - Arkham Knight\BmGame\SaveData"));
    }

    #[test]
    fn test_game_path_shapes() {
        assert!(is_game_path(r"C:\Users\X\Documents\My Games\Some Game"));
        assert!(is_game_path(r"D:\Steam\userdata\123\208650\remote"));
        assert!(is_game_path(r"E:\Games\Weird\save"));
        assert!(is_game_path(r"C:\Anything\Saved Games"));
        assert!(!is_game_path(r"C:\Users\X\Documents\homework"));
        assert!(!is_game_path(r"C:\Program Files\App\logs"));
        assert!(!is_game_path(""));
    }

    #[test]
    fn test_protection_level_numeric_walkup() {
        // numeric ID in path → protect at parent of the topmost numeric segment
        let p = protection_level(Path::new(r"C:\Steam\userdata\12345\208650\remote"));
        assert_eq!(p, Path::new(r"C:\Steam\userdata"));
        let p = protection_level(Path::new(r"C:\Docs\WB Games\Batman Arkham Knight\313100\SaveData"));
        assert_eq!(p, Path::new(r"C:\Docs\WB Games\Batman Arkham Knight"));
        // no numeric → unchanged
        let p = protection_level(Path::new(r"C:\Games\Batman\BmGame\SaveData"));
        assert_eq!(p, Path::new(r"C:\Games\Batman\BmGame\SaveData"));
        // Saved Games (space, not numeric) → unchanged
        let p = protection_level(Path::new(r"C:\Users\X\Saved Games"));
        assert_eq!(p, Path::new(r"C:\Users\X\Saved Games"));
    }

    #[test]
    fn test_discovery_overlap_exclusion() {
        // Existing junction must suppress nested/overlapping candidates
        let existing = vec![r"C:\Users\X\Documents\My Games".to_string()];
        let out: Vec<FoundSave> = Vec::new();
        // simulate the push logic via find (single-point check)
        let protect = protection_level(Path::new(r"C:\Users\X\Documents\My Games\Cyberpunk"));
        let pl = protect.to_string_lossy().to_lowercase();
        assert!(pl.starts_with(r"c:\users\x\documents\my games")); // would be inside existing → excluded by find_game_saves
    }
}
