/// Path utilities: expand/contract Windows paths using env vars + Known Folder API.
/// Expand on config load (absolute paths in memory), contract on save/export (portable paths).

use windows_sys::core::GUID;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;

// ==================== KNOWN FOLDER GUIDs ====================

const FOLDERID_SAVED_GAMES: GUID = GUID {
    data1: 0x4C5C32FF, data2: 0xBB9D, data3: 0x43B0,
    data4: [0xB5, 0xB4, 0x2D, 0x72, 0xE5, 0x4E, 0xAA, 0xA4],
};
const FOLDERID_DOCUMENTS: GUID = GUID {
    data1: 0xFDD39AD0, data2: 0x238F, data3: 0x46AF,
    data4: [0xAD, 0xB4, 0x6C, 0x85, 0x48, 0x03, 0x69, 0xC7],
};
const FOLDERID_DESKTOP: GUID = GUID {
    data1: 0xB4BFCC3A, data2: 0xDB2C, data3: 0x424C,
    data4: [0xB0, 0x29, 0x7F, 0xE9, 0x9A, 0x87, 0xC6, 0x41],
};
const FOLDERID_DOWNLOADS: GUID = GUID {
    data1: 0x374DE290, data2: 0x123F, data3: 0x4565,
    data4: [0x91, 0x64, 0x39, 0xC4, 0x92, 0x5E, 0x46, 0x7B],
};
const FOLDERID_PICTURES: GUID = GUID {
    data1: 0x33E28130, data2: 0x4E1E, data3: 0x4676,
    data4: [0x83, 0x5A, 0x98, 0x39, 0x5C, 0x3B, 0xC3, 0xBB],
};
const FOLDERID_MUSIC: GUID = GUID {
    data1: 0x4BD8D571, data2: 0x6D19, data3: 0x48D3,
    data4: [0xBE, 0x97, 0x42, 0x22, 0x20, 0x08, 0x0E, 0x43],
};
const FOLDERID_VIDEOS: GUID = GUID {
    data1: 0x18989B1D, data2: 0x99B5, data3: 0x455B,
    data4: [0x84, 0x1C, 0xAB, 0x7C, 0x74, 0xE4, 0xDD, 0xFC],
};

/// (token_name, GUID) — resolved via SHGetKnownFolderPath at runtime.
/// Handles Known Folder Redirection (user moved Saved Games to D:\ etc.)
const KNOWN_FOLDERS: &[(&str, GUID)] = &[
    ("SavedGames", FOLDERID_SAVED_GAMES),
    ("Documents", FOLDERID_DOCUMENTS),
    ("Desktop", FOLDERID_DESKTOP),
    ("Downloads", FOLDERID_DOWNLOADS),
    ("Pictures", FOLDERID_PICTURES),
    ("Music", FOLDERID_MUSIC),
    ("Videos", FOLDERID_VIDEOS),
];

/// (token, env_var_name) — resolved via std::env::var.
/// Ordered SPECIFIC to GENERAL — LOCALAPPDATA before USERPROFILE.
/// This is also the contraction priority (most specific token preferred).
const ENV_VARS: &[(&str, &str)] = &[
    ("%LOCALAPPDATA%", "LOCALAPPDATA"),
    ("%APPDATA%", "APPDATA"),
    ("%USERPROFILE%", "USERPROFILE"),
    ("%ProgramFiles(x86)%", "ProgramFiles(x86)"),
    ("%PROGRAMFILES%", "ProgramFiles"),
    ("%PROGRAMDATA%", "ProgramData"),
    ("%PUBLIC%", "PUBLIC"),
];

// ==================== FFI HELPERS ====================

/// Call SHGetKnownFolderPath to get the actual path of a Known Folder.
/// Returns None if the folder doesn't exist or the API fails.
fn sh_get_known_folder_path(folder_id: &GUID) -> Option<String> {
    unsafe {
        let mut path_ptr: *mut u16 = std::ptr::null_mut();
        let result = SHGetKnownFolderPath(
            folder_id,
            0,                          // dwFlags = 0
            std::ptr::null_mut(),       // hToken = NULL (current user)
            &mut path_ptr,
        );
        if result == 0 && !path_ptr.is_null() {
            // Calculate wide string length
            let mut len = 0usize;
            while *path_ptr.add(len) != 0 { len += 1; }
            let slice = std::slice::from_raw_parts(path_ptr, len);
            let s = String::from_utf16_lossy(slice);
            CoTaskMemFree(path_ptr as *const _);
            Some(s)
        } else {
            if !path_ptr.is_null() {
                CoTaskMemFree(path_ptr as *const _);
            }
            None
        }
    }
}

// ==================== PATH NORMALIZATION ====================

/// Normalize for comparison: lowercase, force backslashes, trim trailing separator.
/// NEVER use std::fs::canonicalize — it resolves symlinks and requires the path to exist.
fn normalize(path: &str) -> String {
    let p = path.replace('/', "\\").to_lowercase();
    // Don't trim trailing \ from root paths like "c:\"
    if p.len() > 3 {
        p.trim_end_matches('\\').to_string()
    } else {
        p
    }
}

// ==================== EXPAND (config → absolute) ====================

/// Expand a portable path (with tokens) to an absolute filesystem path.
/// Handles %KNOWNFOLDER:xxx% and %ENVVAR% tokens. Case-insensitive.
/// Paths without tokens are returned as-is.
pub fn expand(path: &str) -> String {
    let mut result = path.to_string();
    let mut changed = true;

    while changed {
        changed = false;
        let lower = result.to_lowercase();

        // Try known folder tokens: %KNOWNFOLDER:xxx%
        for (name, guid) in KNOWN_FOLDERS {
            let token = format!("%knownfolder:{}%", name.to_lowercase());
            if let Some(pos) = lower.find(&token) {
                // M7: fold-length changes (e.g. İ 2→3 bytes) shift indices between
                // `lower` and the original `result`. Verify the token ACTUALLY sits
                // at this offset in the original — else skip (never mis-slice).
                if !result.is_char_boundary(pos)
                    || !result.is_char_boundary(pos + token.len())
                    || !result[pos..pos + token.len()].eq_ignore_ascii_case(&token) {
                    continue;
                }
                if let Some(actual) = sh_get_known_folder_path(guid) {
                    result = format!("{}{}{}", &result[..pos], actual, &result[pos + token.len()..]);
                    changed = true;
                    break;
                }
            }
        }

        if changed { continue; }

        // Try env var tokens: %VAR%
        for (token, var_name) in ENV_VARS {
            let token_lower = token.to_lowercase();
            if let Some(pos) = lower.find(&token_lower) {
                // M7: same boundary + verify guard as above.
                if !result.is_char_boundary(pos)
                    || !result.is_char_boundary(pos + token.len())
                    || !result[pos..pos + token.len()].eq_ignore_ascii_case(&token) {
                    continue;
                }
                if let Ok(value) = std::env::var(var_name) {
                    result = format!("{}{}{}", &result[..pos], value, &result[pos + token.len()..]);
                    changed = true;
                    break;
                }
            }
        }
    }

    result
}

// ==================== CONTRACT (absolute → portable) ====================

/// Contract an absolute path to a portable form using env vars / known folders.
/// Longest-prefix-first, boundary-aware (\ or end-of-string).
/// Only contracts paths under the CURRENT user's profile / known folders.
/// UNC paths, SUBST drives, custom paths → left unchanged.
pub fn contract(path: &str) -> String {
    let normalized_path = normalize(path);

    // Build list of (token, normalized_actual_path) pairs
    let mut prefixes: Vec<(String, String)> = Vec::new();

    // Known folders
    for (name, guid) in KNOWN_FOLDERS {
        if let Some(actual) = sh_get_known_folder_path(guid) {
            let token = format!("%KNOWNFOLDER:{}%", name);
            prefixes.push((token, normalize(&actual)));
        }
    }

    // Env vars
    for (token, var_name) in ENV_VARS {
        if let Ok(value) = std::env::var(var_name) {
            prefixes.push((token.to_string(), normalize(&value)));
        }
    }

    // Sort by actual path length DESCENDING — longest prefix wins.
    // This ensures %LOCALAPPDATA% is preferred over %USERPROFILE%\AppData\Local,
    // and %ProgramFiles(x86)% is preferred over %PROGRAMFILES%.
    prefixes.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    for (token, prefix) in &prefixes {
        if normalized_path == *prefix {
            // Exact match — entire path IS the known folder/env var
            return token.clone();
        }
        // Boundary-aware: prefix must be followed by \ (not partial match)
        let prefix_with_sep = format!("{}\\", prefix);
        if normalized_path.starts_with(&prefix_with_sep) {
            // Replace prefix with token, preserve original suffix (case + separators)
            let suffix = if path.is_char_boundary(prefix.len()) { &path[prefix.len()..] } else { return path.to_string(); }; // Unicode-safe
            return format!("{}{}", token, suffix);
        }
    }

    // No match — return as-is (UNC, SUBST, custom drive, etc.)
    path.to_string()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_real_folders() {
        println!("\n=== REAL FOLDER ROUND-TRIP TESTS ===");
        let real_paths = vec![
            r"C:\Users\lrg4you\Saved Games",
            r"C:\Users\lrg4you\AppData\Local\hermes",
            r"C:\Users\lrg4you\AppData\Roaming\GSE Saves",
            r"C:\Program Files\Nilesoft Shell\imports",
            r"C:\Users\lrg4you\.pi",
        ];
        for path in real_paths {
            let contracted = contract(path);
            let expanded = expand(&contracted);
            let match_ok = path.to_lowercase() == expanded.to_lowercase();
            println!("  {} → {} → {}", path, contracted, expanded);
            println!("    Round-trip: {}", if match_ok { "✓" } else { "✗ MISMATCH" });
            assert!(match_ok, "Round-trip failed for {}: got {}", path, expanded);
        }
    }

    #[test]
    fn test_env_vars_contract() {
        let local = std::env::var("LOCALAPPDATA").unwrap();
        let roaming = std::env::var("APPDATA").unwrap();
        assert_eq!(contract(&(local + r"\hermes")), r"%LOCALAPPDATA%\hermes");
        assert_eq!(contract(&(roaming + r"\GSE Saves")), r"%APPDATA%\GSE Saves");
    }

    #[test]
    fn test_known_folder_savedgames() {
        let contracted = contract(r"C:\Users\lrg4you\Saved Games");
        println!("SavedGames contracted: {}", contracted);
        assert!(contracted.starts_with("%KNOWNFOLDER:") || contracted.contains("Saved Games"));
    }

    #[test]
    fn test_programfiles() {
        let pf = std::env::var("ProgramFiles").unwrap();
        let path = format!(r"{}\Nilesoft Shell\imports", pf);
        let contracted = contract(&path);
        println!("ProgramFiles contracted: {}", contracted);
        assert!(contracted.starts_with("%PROGRAMFILES%") || contracted.starts_with("%ProgramFiles"));
    }

    #[test]
    fn test_custom_drive_unchanged() {
        assert_eq!(contract(r"D:\Games\Saves"), r"D:\Games\Saves");
    }

    #[test]
    fn test_unc_unchanged() {
        assert_eq!(contract(r"\NAS\Games\Saves"), r"\NAS\Games\Saves");
    }

    #[test]
    fn test_boundary_awareness() {
        let up = std::env::var("USERPROFILE").unwrap();
        let fake = format!("{}XYZ", up);
        let contracted = contract(&fake);
        assert!(!contracted.starts_with("%USERPROFILE%XYZ"), "Boundary failed: {}", contracted);
    }

    #[test]
    fn test_case_insensitive() {
        let contracted = contract(r"c:\users\lrg4you\AppData\Local\hermes");
        assert!(contracted.starts_with("%LOCALAPPDATA%"), "Case-insensitive failed: {}", contracted);
    }

    #[test]
    fn test_longest_prefix() {
        let local = std::env::var("LOCALAPPDATA").unwrap();
        let path = local + r"\test";
        let contracted = contract(&path);
        assert!(contracted.starts_with("%LOCALAPPDATA%"), "Should prefer %LOCALAPPDATA%: {}", contracted);
    }

    #[test]
    fn test_expand_no_tokens() {
        assert_eq!(expand(r"D:\Games\Saves"), r"D:\Games\Saves");
    }

    #[test]
    fn test_forward_slashes() {
        let local = std::env::var("LOCALAPPDATA").unwrap().replace(r"\", "/");
        let path = format!("{}/hermes", local);
        let contracted = contract(&path);
        println!("Forward slash: {} → {}", path, contracted);
        assert!(contracted.starts_with("%LOCALAPPDATA%"), "Forward slash failed: {}", contracted);
    }
}
