use crate::config;
use rayon::prelude::*;
use std::io::{BufWriter, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// M6: UNREADABLE is NOT empty. Auto-restore only fires on a CONFIRMED empty
// folder; a transient ACL/read error must not "restore over" data we cannot see.
// M6: UNREADABLE is NOT empty. Auto-restore only fires on a CONFIRMED empty
// folder; a transient ACL/read error must not "restore over" data we cannot see.
pub fn is_dir_empty(path: &str) -> bool {
    match std::fs::read_dir(path) {
        Ok(mut entries) => entries.next().is_none(),
        Err(_) => false, // unknown ≠ empty
    }
}

/// L7: orphaned temp-file sweep — PID-suffixed temps from killed compressions.
/// Runs from BOTH the GUI and -sync startup (headless machines never open the GUI,
/// and the task's 2h ExecutionTimeLimit can kill a mega-folder sync mid-compress).
pub fn sweep_orphaned_temps() {
    if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
        let current_pid = std::process::id();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("lrgex_") && name.ends_with(".tar.zst.tmp") {
                // Parse PID from filename: lrgex_<PID>_<leaf>.tar.zst.tmp or
                // lrgex_<PID>_migrate_<leaf>.tar.zst.tmp (L-6d: unify the prefix).
                if let Some(pid_str) = name.strip_prefix("lrgex_") {
                    if let Some(pid_end) = pid_str.find('_') {
                        if let Ok(pid) = pid_str[..pid_end].parse::<u32>() {
                            if pid != current_pid && !crate::synclog::is_pid_alive(pid) {
                                let _ = std::fs::remove_file(entry.path());
                            }
                        }
                    }
                }
            }
        }
    }
}

/// M-5: killed restores leak full-size `.lrgex_restore_<pid>` staging dirs NEXT
/// TO the user's source folders (not %TEMP%). Sweep them from every junction's
/// parent at GUI/-sync startup. Depends on L-3 (ACCESS_DENIED = alive).
pub fn sweep_orphaned_restore_dirs(cfg: &crate::config::Config) {
    for j in &cfg.junctions {
        let parent = match std::path::Path::new(&j.source_path).parent() {
            Some(p) => p.to_path_buf(),
            None => continue,
        };
        let entries = match std::fs::read_dir(&parent) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(pid_part) = name.strip_prefix(".lrgex_restore_") {
                // <pid>_<seq> — parse the PID prefix, ignore the seq suffix
                if let Some(pid_str) = pid_part.split('_').next() {
                    if let Ok(pid) = pid_str.parse::<u32>() {
                        if pid != std::process::id() && !crate::synclog::is_pid_alive(pid) {
                            let _ = std::fs::remove_dir_all(entry.path());
                        }
                    }
                }
            }
        }
    }

    // L-5: ALSO scan %TEMP% — killed decompress_archive staging
    // (.lrgex_restore_<pid>_<seq>) lives there when the archive destination
    // is %TEMP% itself (preview flow), not under a junction parent.
    if let Ok(temp_entries) = std::fs::read_dir(std::env::temp_dir()) {
        for entry in temp_entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(pid_part) = name.strip_prefix(".lrgex_restore_") {
                if let Some(pid_str) = pid_part.split('_').next() {
                    if let Ok(pid) = pid_str.parse::<u32>() {
                        if pid != std::process::id() && !crate::synclog::is_pid_alive(pid) {
                            let _ = std::fs::remove_dir_all(entry.path());
                        }
                    }
                }
            }
        }
    }
}

// ==================== C3: GLOBAL MUTATION LOCK ====================
// Every mutating entry point (scheduled -sync, GUI threads, -link, -autorestore)
// must hold the same named mutex. The old design only locked -sync mode, so a
// GUI restore running during a scheduled sync (StartWhenAvailable fires on wake
// — exactly when users sit down to restore) could interleave the swap steps and
// destroy the user's pre-restore data. Kernel-owned: released on PID death.

pub struct PairLock {
    _handle: windows_sys::Win32::Foundation::HANDLE, // kept alive, released on drop/death
}

/// Acquire the global mutation lock. Waits up to `timeout_ms` for a concurrent
/// operation to finish. Returns None if still busy (caller reports/aborts —
/// NEVER proceeds without the lock).
pub fn acquire_pair_lock(timeout_ms: u32) -> Option<PairLock> {
    use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use std::os::windows::ffi::OsStrExt;
    let name: Vec<u16> = std::ffi::OsStr::new("LRGEXRestoreSyncLock")
        .encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if h.is_null() { return None; }
        let r = WaitForSingleObject(h, timeout_ms);
        if r == WAIT_OBJECT_0 || r == windows_sys::Win32::Foundation::WAIT_ABANDONED_0 {
            // WAIT_ABANDONED: previous holder died (the task's own PT2H kill) —
            // Windows grants US ownership on return. Abandonment cannot corrupt
            // data (every mutation is temp+rename staged); treating it as "busy"
            // poisoned exactly the next sync after every crash.
            Some(PairLock { _handle: h })
        } else {
            // Genuinely still busy after timeout — abort, do NOT proceed unlocked.
            CloseHandle(h);
            None
        }
    }
}

impl Drop for PairLock {
    fn drop(&mut self) {
        unsafe {
            // MUST release before closing — Windows mutexes stay owned by the
            // acquiring thread until ReleaseMutex or thread death. Without this,
            // a finished GUI operation would block every future -sync forever.
            windows_sys::Win32::System::Threading::ReleaseMutex(self._handle);
            windows_sys::Win32::Foundation::CloseHandle(self._handle);
        }
    }
}

// ==================== COMPRESSION (tar + zstd) ====================

const BIG_FILE: u64 = 8 * 1024 * 1024; // stream these instead of preloading
const BATCH: usize = 2048;             // max files per batch
const BATCH_BYTES: u64 = 64 * 1024 * 1024; // M1: cap resident bytes — 2048 x 1MB save
                                       // files would OOM at 2GB if capped by count alone

/// M1: batches capped by BOTH count and total bytes — preloaded files stay
/// bounded no matter how large the individual files are.
fn chunk_by_bytes(files: &[FileEnt], max_bytes: u64, max_count: usize) -> Vec<&[FileEnt]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < files.len() {
        let start = i;
        let mut bytes = 0u64;
        while i < files.len() && (i - start) < max_count {
            if bytes + files[i].size.min(BIG_FILE) > max_bytes && i > start {
                break;
            }
            bytes += files[i].size.min(BIG_FILE); // only small files preload
            i += 1;
        }
        out.push(&files[start..i]);
    }
    out
}

pub struct FileEnt {
    pub path: PathBuf,
    pub rel: PathBuf,
    pub size: u64,
    /// Real modification time, clamped to FAT32's floor (1980-01-01) so
    /// restores onto FAT32/SD cards can set it. 0 = unknown (legacy walk).
    pub mtime: u64,
}

/// FAT32 cannot represent times before 1980-01-01 UTC.
const FAT32_EPOCH: u64 = 315532800;

/// SINGLE WALK — replaces compute_stats + collect_files.
/// Returns (entries, total_bytes, count). Uses DirEntry::metadata() which on
/// Windows is served from the directory enumeration cache (no extra syscall).
/// SINGLE WALK — replaces compute_stats + collect_files.
/// Returns (entries, total_bytes, count). Uses DirEntry::metadata() which on
/// Windows is served from the directory enumeration cache (no extra syscall).
/// v1.6.2: ITERATIVE (explicit heap stack) — the recursive version was the
/// prime suspect for the stack-overflow crashes; this shape physically cannot
/// overflow, at any directory depth, on any thread's stack.
pub fn walk_tree(base: &Path, excluded: &[String]) -> Option<(Vec<FileEnt>, u64, usize)> {
    if std::fs::read_dir(base).is_err() {
        crate::synclog::write(&format!(
            "[WALK-FAIL] source unreadable — backup ABORTED: {}", base.display()));
        return None;
    }
    let mut out: Vec<FileEnt> = Vec::with_capacity(4096);
    let mut total = 0u64;
    let mut failed_dirs: Vec<String> = Vec::new();
    // Explicit stack of directories to visit. Push children in reverse order
    // so the walk visits them in the same order the recursion did.
    let mut stack: Vec<PathBuf> = vec![base.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(e) => e,
            Err(_) => {
                failed_dirs.push(current.to_string_lossy().to_string());
                continue;
            }
        };
        // collect dirs first so we can reverse-push
        let mut subdirs: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_s = name.to_string_lossy();
            if excluded.iter().any(|e| e.as_str() == name_s.as_ref()) { continue; }

            let ft = match entry.file_type() { Ok(t) => t, Err(_) => continue };
            if ft.is_symlink() { continue; }

            let path = entry.path();
            if ft.is_dir() {
                subdirs.push(path);
            } else {
                let meta = entry.metadata();
                let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let mtime = meta.as_ref().ok().and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()).unwrap_or(0);
                total += size;
                let rel = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
                out.push(FileEnt { path, rel, size, mtime });
            }
        }
        for d in subdirs.into_iter().rev() {
            stack.push(d);
        }
    }
    if !failed_dirs.is_empty() {
        let mut msg = String::from("[WALK-WARN] unreadable subfolders EXCLUDED from backup:");
        for d in failed_dirs.iter().take(20) {
            msg.push_str(&format!("\n  {}", d));
        }
        if failed_dirs.len() > 20 {
            msg.push_str(&format!("\n  ... and {} more", failed_dirs.len() - 20));
        }
        crate::synclog::write(&msg);
    }
    let count = out.len();
    Some((out, total, count))
}

fn walk_inner(base: &Path, current: &Path, excluded: &[String], out: &mut Vec<FileEnt>, total: &mut u64, failed: &mut Vec<String>) {
    let entries = match std::fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => {
            failed.push(current.to_string_lossy().to_string());
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_s = name.to_string_lossy();
        if excluded.iter().any(|e| e.as_str() == name_s.as_ref()) { continue; }

        let ft = match entry.file_type() { Ok(t) => t, Err(_) => continue };
        if ft.is_symlink() { continue; }

        let path = entry.path();
        if ft.is_dir() {
            walk_inner(base, &path, excluded, out, total, failed);
        } else {
            let meta = entry.metadata();
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let mtime = meta.as_ref().ok().and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs()).unwrap_or(0);
            *total += size;
            let rel = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
            out.push(FileEnt { path, rel, size, mtime });
        }
    }
}

/// Exact header construction for tar 0.4 (deterministic: mtime/uid/gid zeroed, 0o644).
fn make_header(size: u64, mtime: u64) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Regular);
    h.set_size(size);
    h.set_mode(0o644);
    h.set_uid(0);
    h.set_gid(0);
    // REAL mtime (clamped ≥ FAT32 floor) — preserves the user's actual file
    // dates through backup→format→restore. Games/save-managers that sort by
    // date keep working. mtime=0 entries (legacy archives) are fine: restore
    // falls back to no-mtime on failure.
    h.set_mtime(mtime.max(FAT32_EPOCH));
    h.set_cksum();
    h
}

fn read_whole(path: &Path, size: u64) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    let mut buf = Vec::with_capacity(size as usize + 64);
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// M-T1: unique-per-invocation fixture tag — PID alone collides when tests
    /// run in parallel threads of one process. Atomic counter => every call
    /// unique => suite is deterministic under default `cargo test`.
    fn tmp_tag() -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        format!("{}_{}", COUNTER.fetch_add(1, Ordering::SeqCst), std::process::id())
    }

    /// Helper: create a temp test folder with known files
    fn make_test_folder() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lrgex_test_{}", tmp_tag()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Create a subfolder
        let sub = dir.join("subfolder");
        std::fs::create_dir_all(&sub).unwrap();
        // Create files with known content
        std::fs::write(dir.join("file1.txt"), b"Hello World").unwrap();
        std::fs::write(dir.join("file2.log"), b"Log data here").unwrap();
        std::fs::write(sub.join("nested.txt"), b"Nested content").unwrap();
        // Create a file that should be excluded
        std::fs::write(dir.join("node_modules"), b"should be excluded").unwrap();
        dir
    }

    #[test]
    fn test_walk_tree_finds_all_files() {
        let dir = make_test_folder();
        let (files, total_bytes, count) = walk_tree(&dir, &[]).expect("walk must succeed");
        println!("\n=== WALK_TREE: {} ===", dir.display());
        println!("  files: {}, bytes: {}", count, total_bytes);
        for f in &files {
            println!("  {} ({} bytes)", f.rel.display(), f.size);
        }
        assert_eq!(count, 4, "Should find 4 files (including node_modules)");
        assert!(total_bytes > 0, "Should have non-zero bytes");
        // Verify each file has correct fields
        let rels: Vec<String> = files.iter().map(|f| f.rel.to_string_lossy().to_string()).collect();
        assert!(rels.contains(&"file1.txt".to_string()), "Missing file1.txt");
        assert!(rels.contains(&"file2.log".to_string()), "Missing file2.log");
        assert!(rels.contains(&"subfolder\\nested.txt".to_string()) ||
               rels.contains(&"subfolder/nested.txt".to_string()), "Missing nested.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_walk_tree_excludes_names() {
        let dir = make_test_folder();
        let excluded = vec!["node_modules".to_string()];
        let (_files, _, count) = walk_tree(&dir, &excluded).expect("walk must succeed");
        println!("\n=== WALK_TREE EXCLUDED ===");
        println!("  files after exclude: {}", count);
        assert_eq!(count, 3, "Should find 3 files (node_modules excluded)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_walk_tree_skips_symlinks() {
        let dir = make_test_folder();
        // Create a symlink (may fail on Windows without admin)
        #[cfg(windows)]
        {
            let link = dir.join("symlink.txt");
            let _ = std::os::windows::fs::symlink_file(dir.join("file1.txt"), &link);
        }
        let (_files, _, count) = walk_tree(&dir, &[]).expect("walk must succeed");
        println!("\n=== WALK_TREE SYMLINKS ===");
        println!("  files: {} (symlinks should be skipped)", count);
        assert_eq!(count, 4, "Symlink should be skipped, still 4 files");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_walk_tree_empty_folder() {
        let dir = std::env::temp_dir().join(format!("lrgex_empty_{}", tmp_tag()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (files, bytes, count) = walk_tree(&dir, &[]).expect("walk must succeed");
        assert_eq!(count, 0, "Empty folder should have 0 files");
        assert_eq!(bytes, 0, "Empty folder should have 0 bytes");
        assert!(files.is_empty(), "File list should be empty");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_walk_tree_byte_count_accurate() {
        let dir = std::env::temp_dir().join(format!("lrgex_bytes_{}", tmp_tag()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"12345").unwrap(); // 5 bytes
        std::fs::write(dir.join("b.txt"), b"1234567890").unwrap(); // 10 bytes
        let (_, total, count) = walk_tree(&dir, &[]).expect("walk must succeed");
        assert_eq!(count, 2);
        assert_eq!(total, 15, "Total bytes should be 15 (5+10)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_compress_decompress_roundtrip() {
        let src = make_test_folder();
        let dest = std::env::temp_dir().join(format!("lrgex_test_archive_{}.tar.zst", std::process::id()));

        println!("\n=== COMPRESS + DECOMPRESS ROUND-TRIP ===");
        println!("  Source: {}", src.display());
        println!("  Archive: {}", dest.display());

        // Compress
        let (ok, skipped) = compress_folder(&src, &dest, &[], None);
        assert!(ok, "Compression should succeed");
        assert!(dest.exists(), "Archive file should exist");
        assert!(dest.metadata().unwrap().len() > 0, "Archive should be non-empty");
        println!("  Compressed: {} bytes, {} skipped", dest.metadata().unwrap().len(), skipped.len());

        // Decompress to a temp folder
        let extract_dir = std::env::temp_dir().join(format!("lrgex_test_extract_{}", tmp_tag()));
        let _ = std::fs::remove_dir_all(&extract_dir);
        std::fs::create_dir_all(&extract_dir).unwrap();
        let (decompressed_ok, msg) = decompress_archive(&dest, &extract_dir);
        assert!(decompressed_ok, "Decompression should succeed: {}", msg);
        println!("  Decompressed to: {}", extract_dir.display());

        // Verify files match
        let original_file1 = std::fs::read(src.join("file1.txt")).unwrap();
        let extracted_file1 = std::fs::read(extract_dir.join("file1.txt")).unwrap_or_default();
        assert_eq!(original_file1, extracted_file1, "file1.txt content should match");

        let original_nested = std::fs::read(src.join("subfolder").join("nested.txt")).unwrap();
        let extracted_nested_path = extract_dir.join("subfolder").join("nested.txt");
        let extracted_nested = std::fs::read(&extracted_nested_path).unwrap_or_default();
        assert_eq!(original_nested, extracted_nested, "nested.txt content should match");

        println!("  ✓ file1.txt matches");
        println!("  ✓ subfolder/nested.txt matches");
        println!("  ✓ Round-trip PASSED");

        // Cleanup
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&extract_dir);
        let _ = std::fs::remove_file(&dest);
    }

    #[test]
    fn test_compress_with_exclusions() {
        let src = make_test_folder();
        let dest = std::env::temp_dir().join(format!("lrgex_test_excl_{}.tar.zst", std::process::id()));
        let excluded = vec!["node_modules".to_string()];

        let (ok, _skipped) = compress_folder(&src, &dest, &excluded, None);
        assert!(ok, "Compression with exclusions should succeed");

        // Decompress + verify node_modules is NOT in the archive
        let extract_dir = std::env::temp_dir().join(format!("lrgex_test_excl_extract_{}", tmp_tag()));
        let _ = std::fs::remove_dir_all(&extract_dir);
        std::fs::create_dir_all(&extract_dir).unwrap();
        let (dec_ok, _) = decompress_archive(&dest, &extract_dir);
        assert!(dec_ok, "Decompression should succeed");
        assert!(!extract_dir.join("node_modules").exists(), "node_modules should be excluded from archive");
        assert!(extract_dir.join("file1.txt").exists(), "file1.txt should be in archive");

        println!("\n=== EXCLUSIONS TEST ===");
        println!("  ✓ node_modules excluded");
        println!("  ✓ file1.txt present");

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&extract_dir);
        let _ = std::fs::remove_file(&dest);
    }

    #[test]
    fn test_compress_large_file_streaming() {
        let dir = std::env::temp_dir().join(format!("lrgex_large_{}", tmp_tag()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Create a file larger than BIG_FILE (8MB)
        let large_path = dir.join("large.bin");
        let mut f = std::fs::File::create(&large_path).unwrap();
        let chunk = vec![0xABu8; 1024 * 1024]; // 1MB chunks
        for _ in 0..10 {
            f.write_all(&chunk).unwrap();
        } // 10MB file
        drop(f);

        let dest = std::env::temp_dir().join(format!("lrgex_large_{}.tar.zst", std::process::id()));
        let (ok, _) = compress_folder(&dir, &dest, &[], None);
        assert!(ok, "Large file compression should succeed");

        // Decompress + verify size
        let extract_dir = std::env::temp_dir().join(format!("lrgex_large_extract_{}", tmp_tag()));
        let _ = std::fs::remove_dir_all(&extract_dir);
        std::fs::create_dir_all(&extract_dir).unwrap();
        let (dec_ok, _) = decompress_archive(&dest, &extract_dir);
        assert!(dec_ok, "Large file decompression should succeed");

        let orig_size = std::fs::metadata(&large_path).unwrap().len();
        let extracted_path = extract_dir.join("large.bin");
        assert!(extracted_path.exists(), "large.bin should exist in archive");
        let extracted_size = std::fs::metadata(&extracted_path).unwrap().len();
        assert_eq!(orig_size, extracted_size, "Large file size should match after round-trip: {} vs {}", orig_size, extracted_size);

        println!("\n=== LARGE FILE STREAMING TEST ===");
        println!("  Original: {} bytes", orig_size);
        println!("  Extracted: {} bytes", extracted_size);
        println!("  ✓ Size matches");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&extract_dir);
        let _ = std::fs::remove_file(&dest);
    }

    #[test]
    fn test_decompress_nonexistent_archive() {
        let result = decompress_archive(std::path::Path::new("C:\\nonexistent\\fake.tar.zst"), std::path::Path::new("C:\\tmp\\fake_extract"));
        assert!(!result.0, "Should fail on nonexistent archive");
        println!("\n=== NONEXISTENT ARCHIVE ===");
        println!("  ✓ Correctly returned failure");
    }

    #[test]
    fn test_decompress_real_hermes_archive() {
        let archive = std::path::PathBuf::from(r"C:\Users\lrg4you\OneDrive\Documents\LRGEX-saves\backup\hermes\hermes.tar.zst");
        if !archive.exists() {
            println!("\n=== HERMES ARCHIVE: SKIPPED (not found) ===");
            return;
        }
        println!("\n=== HERMES ARCHIVE VERIFICATION ===");
        println!("  Archive: {} ({} MB)", archive.display(), archive.metadata().unwrap().len() / 1_048_576);

        let extract_dir = std::env::temp_dir().join(format!("lrgex_hermes_verify_{}", tmp_tag()));
        let _ = std::fs::remove_dir_all(&extract_dir);
        std::fs::create_dir_all(&extract_dir).unwrap();

        let (ok, msg) = decompress_archive(&archive, &extract_dir);
        if ok {
            // Count extracted files
            let mut count = 0usize;
            for entry in walk_dir_count(&extract_dir) {
                count += 1;
                let _ = entry;
            }
            println!("  Decompressed: {} files", count);
            assert!(count > 100000, "Hermes should have 139k+ files, got {}", count);
            println!("  ✓ Archive is valid + complete");
        } else {
            panic!("Hermes archive decompression failed: {}", msg);
        }

        let _ = std::fs::remove_dir_all(&extract_dir);
    }

    fn walk_dir_count(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files.extend(walk_dir_count(&path));
                } else {
                    files.push(path);
                }
            }
        }
        files
    }

    #[test]
    fn test_interval_xml() {
        // H-1 regression: the T designator is MANDATORY — P30M = 30 months!
        assert_eq!(interval_xml(1),    "PT1M");
        assert_eq!(interval_xml(30),   "PT30M");
        assert_eq!(interval_xml(60),   "PT1H");
        assert_eq!(interval_xml(90),   "PT1H30M");
        assert_eq!(interval_xml(1440), "P1D");
        assert_eq!(interval_xml(1500), "P1DT1H");
        assert_eq!(interval_xml(2880), "P2D");
        assert_eq!(interval_xml(1441), "P1DT1M"); // d>0, h=0, min>0 — T still emitted
        assert_eq!(interval_xml(4321), "P3DT1M");
        assert_eq!(interval_xml(0),    "PT1M");  // clamped
        assert_eq!(interval_xml(-5),   "PT1M");  // clamped
    }

    // ============ CONTENT-HASH MANIFEST TESTS ============
    // Unit tests on the pure logic — no shared .lrgex state, parallel-safe.

    /// Build a FileEnt for a real file on disk.
    fn ent(path: &Path, base: &Path) -> FileEnt {
        FileEnt {
            path: path.to_path_buf(),
            rel: path.strip_prefix(base).unwrap().to_path_buf(),
            size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            mtime: std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs()).unwrap_or(0),
        }
    }

    #[test]
    fn test_mtime_preserved_roundtrip() {
        // v1.7: backup must capture the file's REAL mtime (≥ FAT32 floor) and
        // restore must preserve it. Guards the whole chain: walk capture →
        // header clamp → unpack preserve.
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("lrgex_mtime_{}_src", tmp_tag()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("game.sav");
        std::fs::write(&f, b"save bytes").unwrap();
        let arch = std::env::temp_dir().join(format!("lrgex_mtime_{}.tar.zst", tmp_tag()));
        let (ok, _) = compress_folder(&dir, &arch, &[], None);
        assert!(ok, "backup failed");
        // Read the archive header mtime — must be ≥ FAT32 floor (1980)
        let raw = std::fs::read(&arch).unwrap();
        let dec = zstd::stream::decode_all(&raw[..]).unwrap();
        // GNU tar header: mtime at offset 136, 12 octal bytes
        assert!(dec.len() > 148, "archive too small: {}", dec.len());
        let mtime_digits: String = String::from_utf8_lossy(&dec[136..148])
            .chars().take_while(|c| c.is_ascii_digit()).collect();
        let mtime = u64::from_str_radix(&mtime_digits, 8).expect("octal mtime");
        assert!(mtime >= 315532800, "header mtime {} below FAT32 floor", mtime);
        assert!(mtime <= std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 5,
            "header mtime {} in the future", mtime);
        // Restore + verify preserved mtime (±2s tolerance)
        let out = std::env::temp_dir().join(format!("lrgex_mtime_{}_out", tmp_tag()));
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        let (rok, rmsg) = decompress_archive(&arch, &out);
        assert!(rok, "restore failed: {}", rmsg);
        let restored = out.join("game.sav");
        assert!(restored.exists());
        let got = std::fs::metadata(&restored).unwrap().modified().unwrap()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let want = std::fs::metadata(&f).unwrap().modified().unwrap()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert!(got.abs_diff(want) <= 2, "mtime not preserved: restored {} vs source {}", got, want);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&out);
        let _ = std::fs::remove_file(&arch);
    }

    #[test]
    fn test_detect_change_same_size_different_content() {
        // THE bug this whole feature fixes: same size, different content.
        let dir = std::env::temp_dir()
            .join(format!("lrgex_manifest_1_{}", tmp_tag()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("config.json");

        // Backup state: "AAAAAA"
        std::fs::write(&f, b"AAAAAA").unwrap();
        let files = vec![ent(&f, &dir)];
        let stored = compute_manifest(&files, None);

        // User edits to "BBBBBB" — SAME SIZE, DIFFERENT CONTENT
        std::fs::write(&f, b"BBBBBB").unwrap();
        let files2 = vec![ent(&f, &dir)];
        let (changed, _) = detect_change(&stored, &files2, None);
        assert!(changed, "Same-size content edit MUST be detected");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_change_identical_content() {
        let dir = std::env::temp_dir().join(format!("lrgex_manifest_2_{}", tmp_tag()))
            .join("m2");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("save.dat");

        std::fs::write(&f, b"game save v1").unwrap();
        let files = vec![ent(&f, &dir)];
        let stored = compute_manifest(&files, None);

        // Nothing changed — same files re-hashed
        let files2 = vec![ent(&f, &dir)];
        let (changed, _) = detect_change(&stored, &files2, None);
        assert!(!changed, "Identical content must NOT trigger a backup");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_change_file_added() {
        let dir = std::env::temp_dir().join(format!("lrgex_manifest_3_{}", tmp_tag()))
            .join("m3");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f1 = dir.join("a.txt");
        let f2 = dir.join("b.txt");

        std::fs::write(&f1, b"one").unwrap();
        let stored = compute_manifest(&[ent(&f1, &dir)], None);

        std::fs::write(&f2, b"two").unwrap();
        let (changed, _) = detect_change(&stored, &[ent(&f1, &dir), ent(&f2, &dir)], None);
        assert!(changed, "Added file must be detected");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_change_locked_file_size_only() {
        // C1 heal semantics:
        // (a) stored h=0 (was locked at manifest build) + readable now => CHANGED
        //     (heal: re-backup once to learn the content — un-blinds permanently).
        // (b) stored real hash + currently locked => UNCHANGED (size-only compare;
        //     a permanently-locked file must not loop re-backups forever).
        let dir = std::env::temp_dir()
            .join(format!("lrgex_manifest_4_{}", tmp_tag()))
            .join("m4");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("locked.db");
        std::fs::write(&f, b"0123456789").unwrap();

        // (a) heal: stored h=0, readable now => changed
        let mut stored = compute_manifest(&[ent(&f, &dir)], None);
        stored[0].h = 0;
        let (changed, _) = detect_change(&stored, &[ent(&f, &dir)], None);
        assert!(changed, "stored-unknown + readable-now must HEAL (re-backup)");

        // (b) locked now: stored real hash (recorded BEFORE the lock), current
        // file unreadable (exclusive handle => hash_file None => c.h=0) => unchanged.
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let stored2 = compute_manifest(&[ent(&f, &dir)], None); // real hash, file still readable
            assert_ne!(stored2[0].h, 0, "precondition: stored hash must be real");
            let _guard = std::fs::OpenOptions::new()
                .read(true).share_mode(0) // exclusive — nobody else can open it
                .open(&f).expect("open exclusive");
            let current_files = vec![ent(&f, &dir)];
            let (changed2, _) = detect_change(&stored2, &current_files, None);
            assert!(!changed2, "currently-locked file (same size) must NOT loop re-backups");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_manifest_sidecar_roundtrip() {
        let dir = std::env::temp_dir().join(format!("lrgex_manifest_5_{}", tmp_tag()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sidecar = dir.join("sidecar.txt");

        let manifest = vec![
            FileMeta { p: "sub/file1.txt".into(), s: 100, h: 0xdeadbeef },
            FileMeta { p: "locked.db".into(), s: 9999, h: 0 },
        ];
        write_stored_manifest(&sidecar, &manifest);

        let read = read_stored_manifest(&sidecar).expect("manifest must roundtrip");
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].p, "sub/file1.txt");
        assert_eq!(read[0].s, 100);
        assert_eq!(read[0].h, 0xdeadbeef);
        assert_eq!(read[1].h, 0);

        // Totals path (fast-path stats) works too
        let raw = std::fs::read_to_string(&sidecar).unwrap();
        let json = raw.trim().strip_prefix("manifest:").unwrap();
        let (total, count) = parse_manifest_totals(json).unwrap();
        assert_eq!(total, 100 + 9999);
        assert_eq!(count, 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_old_sidecar_migration() {
        // Old "size,count" format: read_stored_stats parses it, read_stored_manifest
        // returns None => detection treats it as changed => re-backup + write manifest.
        let dir = std::env::temp_dir().join(format!("lrgex_manifest_6_{}", tmp_tag()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sidecar = dir.join("old_sidecar.txt");
        std::fs::write(&sidecar, "12345,42").unwrap(); // old format

        let (size, count) = read_stored_stats(&sidecar);
        assert_eq!(size, 12345);
        assert_eq!(count, 42);

        assert!(read_stored_manifest(&sidecar).is_none(),
            "Old format must NOT parse as a manifest — it must trigger migration re-backup");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_hash_file_deterministic() {
        let dir = std::env::temp_dir().join(format!("lrgex_manifest_7_{}", tmp_tag()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.bin");

        std::fs::write(&f, vec![0xABu8; 300_000]).unwrap(); // > 64KB — exercises multi-chunk loop
        let h1 = hash_file(&f).unwrap();
        let h2 = hash_file(&f).unwrap();
        assert_eq!(h1, h2, "Hash must be deterministic");

        std::fs::write(&f, vec![0xCDu8; 300_000]).unwrap();
        let h3 = hash_file(&f).unwrap();
        assert_ne!(h1, h3, "Different content must hash differently");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// E2E: backup → delete source → restore → sync again => "No changes".
    /// This is THE test justifying content hashes over mtime: restored files have
    /// NEW mtimes but IDENTICAL content, so an mtime-based check would false-positive.
    /// Uses a unique leaf name to avoid colliding with other tests in shared .lrgex.
    #[test]
    fn test_restore_invariance_e2e() {
        // Leaf is derived from the source folder name — pid-unique already.
        let src = std::env::temp_dir().join(format!("lrgex_e2e_{}", tmp_tag()));
        let leaf = src.file_name().unwrap().to_string_lossy().to_string();
        let _ = std::fs::remove_dir_all(&src);
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::write(src.join("config.json"), b"{\"theme\":\"dark\"}").unwrap();
        std::fs::write(src.join("nested") .join("save.dat"), b"SAVE-DATA-001").unwrap();

        let src_str = src.to_string_lossy().to_string();

        // 1) First backup (force first run semantics: no backup exists yet)
        let (ok, msg) = sync_pair_to_cloud(&src_str, &[], 2, false);
        assert!(ok, "first backup failed: {}", msg);
        assert!(config::backup_file_for(&src_str).exists(), "backup archive must exist");

        // 2) Wipe source (simulates format / restore scenario)
        std::fs::remove_dir_all(&src).unwrap();

        // 3) Restore from archive
        let archive = config::backup_file_for(&src_str);
        let (ok, msg) = decompress_archive(&archive, &src);
        assert!(ok, "restore failed: {}", msg);

        // 4) Sync again — restored content is IDENTICAL => must be "No changes".
        //    mtime-based detection would fail here (restore writes new mtimes).
        let (ok, msg) = sync_pair_to_cloud(&src_str, &[], 2, false);
        assert!(ok, "post-restore sync failed: {}", msg);
        assert!(msg.contains("No changes"),
            "Restored identical content must NOT trigger a re-backup — got: {}", msg);

        // 5) Same-size edit post-restore => MUST re-backup
        std::fs::write(src.join("config.json"), b"{\"theme\":\"red_\"}").unwrap(); // same length
        let (ok, msg) = sync_pair_to_cloud(&src_str, &[], 2, false);
        assert!(ok, "sync after same-size edit failed: {}", msg);
        assert!(!msg.contains("No changes"),
            "Same-size edit after restore MUST trigger a re-backup — got: {}", msg);

        // Cleanup: remove this test's artifacts from the shared .lrgex
        let _ = std::fs::remove_file(config::backup_file_for(&src_str));
        let _ = std::fs::remove_file(config::sidecar_for(&src_str));
        let _ = std::fs::remove_dir_all(config::trash_path_for(&src_str));
        let _ = std::fs::remove_dir_all(&src);
    }
}

/// Compress a source directory to a .tar.zst. Files are read in parallel batches
/// (rayon) and written to the tar stream in order. `prewalked` lets the caller
/// share the change-detection walk (one walk total).
struct ByteReader<R: std::io::Read> {
    inner: R,
    progress: crate::synclog::Progress,
}

impl<R: std::io::Read> std::io::Read for ByteReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 { self.progress.tick_bytes(n as u64); }
        Ok(n)
    }
}

pub fn compress_folder(
    source: &Path,
    dest: &Path,
    excluded: &[String],
    prewalked: Option<(Vec<FileEnt>, u64, usize)>,
) -> (bool, Vec<String>) {
    crate::synclog::clear_status(); // start clean — clear any stale status from a previous run
    let t_all = std::time::Instant::now();
    let label = source.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let progress = crate::synclog::Progress::new(&label);
    progress.set_phase(0); // walk

    let t_walk = std::time::Instant::now();
    let (files, total_bytes, count) = match prewalked {
        Some(w) => w,
        None => match walk_tree(source, excluded) {
            Some(w) => w,
            None => { progress.finish(4); return (false, vec![]); } // M-1: stop set, no writer leaked
        },
    };
    let walk = t_walk.elapsed();
    progress.set_totals(count, total_bytes);
    progress.set_phase(1); // compress

    let file = match std::fs::File::create(dest) {
        Ok(f) => f,
        Err(_) => { progress.finish(4); return (false, vec![]); } // M-1
    };
    let writer = BufWriter::with_capacity(4 * 1024 * 1024, file);

    let mut encoder = match zstd::Encoder::new(writer, 1) {
        Ok(e) => e,
        Err(_) => { progress.finish(4); return (false, vec![]); } // M-1
    };
    let threads = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
    let _ = encoder.multithread(threads); // multi-threaded compression (zstd workers)
    let _ = encoder.include_checksum(true); // H3: frame checksum — bit rot fails LOUD, not silently
    let mut builder = tar::Builder::new(encoder.auto_finish());

    // M-1: heartbeat spawns ONLY after every early-fail check passed — it can
    // never outlive the function on those paths. Earlier exits called finish(4)
    // which sets stop, so even a pre-existing zombie is retired.
    let heartbeat = progress.spawn_writer();

    let t_append = std::time::Instant::now();
    let mut processed = 0usize;
    let mut skipped: Vec<String> = Vec::new();

    for batch in chunk_by_bytes(&files, BATCH_BYTES, BATCH) {
        let loaded: Vec<Option<std::io::Result<Vec<u8>>>> = batch
            .par_iter()
            .map(|e| {
                if e.size > BIG_FILE {
                    None
                } else {
                    let r = read_whole(&e.path, e.size);
                    progress.tick_bytes(e.size);
                    Some(r)
                }
            })
            .collect();

        for (e, data) in batch.iter().zip(loaded.into_iter()) {
            let res: std::io::Result<()> = match data {
                Some(Ok(buf)) => {
                    let mut h = make_header(buf.len() as u64, e.mtime);
                    let mut slice: &[u8] = buf.as_slice();
                    builder.append_data(&mut h, &e.rel, &mut slice)
                }
                Some(Err(err)) => Err(err),
                None => {
                    match std::fs::File::open(&e.path) {
                        Ok(f) => match f.metadata() {
                            Ok(m) => {
                                let mut h = make_header(m.len(), e.mtime);
                                let mut cr = ByteReader { inner: f, progress: progress.clone() };
                                builder.append_data(&mut h, &e.rel, &mut cr)
                            }
                            Err(err) => Err(err),
                        },
                        Err(err) => Err(err),
                    }
                }
            };

            match res {
                Ok(()) => {
                    processed += 1;
                }
                Err(_) => {
                    let rel_s = e.rel.to_string_lossy().to_string();
                    crate::synclog::write(&format!("  [SKIP] {} (locked/unreadable)", rel_s));
                    skipped.push(rel_s);
                }
            }
        }
    }
    let append = t_append.elapsed();

    progress.set_phase(2); // flush
    let t_flush = std::time::Instant::now();
    let mut ok = builder.finish().is_ok();
    match builder.into_inner() {
        Ok(mut enc) => { if enc.flush().is_err() { ok = false; } }
        Err(_) => ok = false,
    }
    let flush = t_flush.elapsed();
    progress.finish(if ok { 3 } else { 4 });
    let _ = heartbeat.join();

    let mb = total_bytes as f64 / 1_048_576.0;
    let secs = append.as_secs_f64().max(0.001);
    crate::synclog::write(&format!(
        "  [PROFILE] {} files | {:.1} MB | walk={:.2}s | read+compress={:.2}s ({:.0} files/s, {:.1} MB/s) | flush={:.2}s | total={:.2}s | threads={}",
        count, mb, walk.as_secs_f64(), append.as_secs_f64(),
        processed as f64 / secs, mb / secs,
        flush.as_secs_f64(), t_all.elapsed().as_secs_f64(), threads
    ));
    (ok, skipped)
}

/// Decompress a .tar.zst file to a destination directory
/// Atomic extraction: decompress to a TEMP dir first.
/// Only on FULL success: merge into destination (overwrite existing).
/// On ANY failure: discard temp, destination completely untouched.
/// This guarantees no partial restores — ever.

/// Wrapper that counts bytes read for progress tracking
struct CountingReader {
    inner: std::fs::File,
    counter: Arc<AtomicU64>,
}

impl std::io::Read for CountingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.counter.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// Unpack an archive into dest with a FRESH decoder (the zstd stream is
/// single-use). `preserve_mtime=false` for the legacy/FAT32 fallback.
fn unpack_fresh(archive: &Path, dest: &Path, preserve_mtime: bool) -> Result<(), String> {
    use std::io::Read;
    let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let bytes_read = Arc::new(AtomicU64::new(0));
    let counting_file = CountingReader { inner: f, counter: bytes_read.clone() };
    let decoder = zstd::Decoder::new(counting_file).map_err(|e| e.to_string())?;
    let mut tar = tar::Archive::new(decoder);
    tar.set_preserve_mtime(preserve_mtime);
    tar.set_preserve_permissions(false);
    tar.unpack(dest).map_err(|e| e.to_string())
}

pub fn decompress_archive(archive: &Path, dest: &Path) -> (bool, String) {
    let _ = std::fs::create_dir_all(dest);
    let temp_dir = dest.parent().unwrap_or(std::path::Path::new("."))
        .join(format!(".lrgex_restore_{}_{}", std::process::id(), {
            // Unique per call: parallel decompressions in one process (tests, or a
            // restore racing a snapshot restore) must not share a staging dir.
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQ: AtomicU64 = AtomicU64::new(0);
            SEQ.fetch_add(1, Ordering::SeqCst)
        }));
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = std::fs::create_dir_all(&temp_dir);
    // M5: mark staging HIDDEN + TEMPORARY (attrib) — OneDrive/Syncthing watchers
    // skip hidden temp dirs, so a restore inside a synced tree stops re-uploading
    // gigabytes of transient extraction data. No new deps: attrib is in-box.
    let _ = std::process::Command::new("attrib")
        .args(["+h", "+t", temp_dir.to_string_lossy().as_ref()])
        .creation_flags(0x08000000u32)
        .output();

    let arch_size = std::fs::metadata(archive).map(|m| m.len()).unwrap_or(0);
    let leaf_name = archive.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    crate::synclog::write(&format!("  [DECOMPRESS] {} size={} bytes", leaf_name, arch_size));

    let file = match std::fs::File::open(archive) {
        Ok(f) => f,
        Err(e) => { let _ = std::fs::remove_dir_all(&temp_dir); return (false, format!("cannot open archive: {}", e)); }
    };

    let bytes_read = Arc::new(AtomicU64::new(0));
    let counting_file = CountingReader { inner: file, counter: bytes_read.clone() };

    let decoder = match zstd::Decoder::new(counting_file) {
        Ok(d) => d,
        Err(e) => { let _ = std::fs::remove_dir_all(&temp_dir); return (false, format!("corrupt archive (zstd): {}", e)); }
    };
    let mut tar = tar::Archive::new(decoder);
    // Restore REAL mtimes (new archives carry them, clamped FAT32-safe).
    tar.set_preserve_mtime(true);
    tar.set_preserve_permissions(false);

    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();
    let bytes_clone = bytes_read.clone();
    let total = arch_size;
    let leaf_clone = leaf_name.clone();
    std::thread::spawn(move || {
        while !stop_clone.load(Ordering::Relaxed) {
            let read = bytes_clone.load(Ordering::Relaxed);
            let pct = if total > 0 { (read * 100 / total).min(100) } else { 0 };
            let read_mb = read as f64 / 1048576.0;
            let total_mb = total as f64 / 1048576.0;
            crate::synclog::write_progress(&format!("Decompressing {} - {:.0}/{:.0} MB ({}%)", leaf_clone, read_mb, total_mb, pct));
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });

    let _ = &tar; // built above for the header probe
    // Unpack with a FRESH decoder per attempt — the zstd stream is consumed
    // by the first unpack and cannot be replayed.
    let mut unpack_result = unpack_fresh(archive, &temp_dir, true);
    if unpack_result.is_err() {
        // LEGACY-ARCHIVE FALLBACK: pre-v1.7 archives carry mtime=0 (1970)
        // which FAT32 rejects (pre-1980 impossible). Retry WITHOUT metadata —
        // content is what matters; a failed restore is worse than lost dates.
        crate::synclog::write("  [DECOMPRESS] retry without metadata (legacy archive / FAT32 destination)");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);
        unpack_result = unpack_fresh(archive, &temp_dir, false);
    }
    if let Err(e) = unpack_result {
        stop.store(true, Ordering::Relaxed);
        crate::synclog::write_progress("");
        let _ = std::fs::remove_dir_all(&temp_dir);
        return (false, format!("extract failed: {}", e));
    }
    {
        {
            stop.store(true, Ordering::Relaxed);
            crate::synclog::write_progress("");
            let ec = std::fs::read_dir(&temp_dir).map(|d| d.count()).unwrap_or(0);
            crate::synclog::write(&format!("  [DECOMPRESS] unpack OK - {} entries, {} bytes archive", ec, arch_size));
            // M4: PID-suffixed bak name is collision-proof; a crash-recovery bak
            // (stale *.lrgex_bak.* + dest missing) is RECOVERED, never deleted.
            // L-2: when dest EXISTS, any other-pid bak is provably stale (the
            // pair lock makes a concurrent same-name restore impossible) —
            // sweep it instead of leaking a full-size copy per crash.
            if let Some(parent) = dest.parent() {
                let name = dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let bak_prefix = format!("{}.lrgex_bak.", name);
                if !dest.exists() {
                    if let Ok(entries) = std::fs::read_dir(parent) {
                        for entry in entries.flatten() {
                            let en = entry.file_name().to_string_lossy().to_string();
                            if en.starts_with(&bak_prefix) {
                                let _ = std::fs::rename(entry.path(), dest);
                                break;
                            }
                        }
                    }
                } else {
                    // dest present + stale baks => crash happened between rename
                    // and bak-delete. Under the lock, no live sibling can own one.
                    if let Ok(entries) = std::fs::read_dir(parent) {
                        for entry in entries.flatten() {
                            let en = entry.file_name().to_string_lossy().to_string();
                            if en.starts_with(&bak_prefix)
                                && !en.ends_with(&format!(".{}", std::process::id()).to_string()) {
                                let _ = std::fs::remove_dir_all(entry.path());
                            }
                        }
                    }
                }
            }
            let backup_name = dest.parent()
                .map(|p| p.join(format!("{}.lrgex_bak.{}",
                    dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    std::process::id())))
                .unwrap_or_else(|| dest.with_extension("lrgex_bak"));
            let _ = std::fs::remove_dir_all(&backup_name);
            if dest.exists() {
                if std::fs::rename(dest, &backup_name).is_err() {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    return (false, "cannot swap destination (locked?)".into());
                }
            }
            if let Err(e) = std::fs::rename(&temp_dir, dest) {
                if backup_name.exists() { let _ = std::fs::rename(&backup_name, dest); }
                let _ = std::fs::remove_dir_all(&temp_dir);
                return (false, format!("swap failed: {}", e));
            }
            let _ = std::fs::remove_dir_all(&backup_name);
            (true, String::new())
        }
    }
}

// compute_stats removed — merged into walk_tree (single shared walk for change-detection + compression)

/// Read stored source stats from sidecar file
fn read_stored_stats(sidecar: &Path) -> (u64, usize) {
    // NEW manifest format: "manifest:<json>". OLD format: "size,count".
    // For stats purposes we only need totals — hash path handled separately.
    std::fs::read_to_string(sidecar)
        .ok()
        .and_then(|s| {
            let s = s.trim();
            if let Some(json) = s.strip_prefix("manifest:") {
                // New format: parse totals from the manifest JSON
                parse_manifest_totals(json)
            } else {
                let parts: Vec<&str> = s.split(',').collect();
                if parts.len() == 2 {
                    Some((parts[0].parse().ok()?, parts[1].parse().ok()?))
                } else {
                    None
                }
            }
        })
        .unwrap_or((u64::MAX, usize::MAX))
}

// ==================== CONTENT-HASH MANIFEST ====================
// Per-file manifest for content-based change detection. Catches same-size
// modifications that (size, count) stats miss (e.g. "AAAAAA" -> "BBBBBB").
//
// Design rules:
// 1. FAST PATH: (size, count) mismatch => changed, skip hashing entirely.
// 2. MIGRATION: old "size,count" sidecar (no "manifest:" prefix) => treat as
//    changed on first manifest-capable run, write the new format.
// 3. CONSISTENCY: manifest reflects files successfully archived. Hash-read
//    failures on locked files fall back to size-only comparison so a
//    permanently locked file doesn't read as "changed" forever.

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct FileMeta {
    /// Relative path, forward-slash separated (portable across the config).
    p: String,
    s: u64,
    /// xxhash-style content hash — 0 means "unknown" (locked at manifest build).
    h: u64,
}

fn manifest_key(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

/// Parallel content hash of all files. Reuses the rayon walker pool.
/// Returns None per-file on read failure (locked), keeping size for fallback.
/// `progress`: optional live ticker — the content-hash pass is the LONGEST
/// phase on big folders (2 GB ≈ 90 s) and must not be invisible.
fn compute_manifest(files: &[FileEnt], progress: Option<&crate::synclog::Progress>) -> Vec<FileMeta> {
    files
        .par_iter()
        .map(|f| {
            let h = hash_file(&f.path).unwrap_or(0);
            if let Some(p) = progress { p.tick(f.size.max(1)); } // files+bytes — live "N files" display
            FileMeta { p: manifest_key(&f.rel), s: f.size, h }
        })
        .collect()
}

/// FNV-1a based content hash (64-bit). Non-cryptographic by design: we detect
/// accidental change, not adversarial tampering. ~memory-bandwidth speed.
/// v1.6.2: read buffer on the HEAP — the 64KB stack array, inlined by LTO into
/// rayon's split-recursion frames, caused the stack-overflow crashes (each
/// recursion level carried a giant frame; a worker's 2MB stack died in ~15 levels).
fn hash_file(path: &Path) -> Option<u64> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::with_capacity(1 << 20, f);
    let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis
    let mut buf = vec![0u8; 65536]; // heap — no giant stack frame
    loop {
        let n = reader.read(&mut buf).ok()?;
        if n == 0 { break; }
        for &b in &buf[..n] {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    Some(hash)
}

fn parse_manifest_totals(json: &str) -> Option<(u64, usize)> {
    let files: Vec<FileMeta> = serde_json::from_str(json).ok()?;
    let total: u64 = files.iter().map(|m| m.s).sum();
    Some((total, files.len()))
}

fn read_stored_manifest(sidecar: &Path) -> Option<Vec<FileMeta>> {
    let s = std::fs::read_to_string(sidecar).ok()?;
    let json = s.trim().strip_prefix("manifest:")?;
    serde_json::from_str(json).ok()
}

/// Content-based change detection. Called only when size+count already match
/// (the "looks unchanged" path). One full parallel hash pass — the same pass
/// whose result is REUSED as the new manifest if a backup is triggered, so the
/// total cost is exactly one read pass per sync, never two.
fn detect_change(stored: &[FileMeta], current_files: &[FileEnt], progress: Option<&crate::synclog::Progress>) -> (bool, Vec<FileMeta>) {
    let current = compute_manifest(current_files, progress);
    if stored.len() != current.len() { return (true, current); }
    let map: std::collections::HashMap<&str, &FileMeta> =
        stored.iter().map(|m| (m.p.as_str(), m)).collect();
    for c in &current {
        match map.get(c.p.as_str()) {
            None => return (true, current), // file added
            Some(s) => {
                if s.s != c.s { return (true, current); } // size differs
                // C1 heal rule: stored hash unknown (file was locked at some past
                // backup) but readable NOW — we never learned its content, so
                // re-backup once. This is what un-blinds detection permanently:
                // the manifest self-heals instead of staying h=0 forever.
                if s.h == 0 && c.h != 0 {
                    crate::synclog::write("  [HEAL] previously-locked file now readable — re-backup");
                    return (true, current);
                }
                // Currently locked (h=0 now) — size match is all we can verify.
                // Prevents a permanently-locked file from looping re-backups.
                if s.h != 0 && c.h == 0 { continue; }
                if s.h != c.h {
                    return (true, current); // content differs (same size!)
                }
            }
        }
    }
    (false, current)
}

fn write_stored_manifest(sidecar: &Path, manifest: &[FileMeta]) {
    if let Ok(json) = serde_json::to_string(manifest) {
        let _ = std::fs::write(sidecar, format!("manifest:{}", json));
    }
}

// ==================== TIMESTAMPS ====================

fn compact_timestamp() -> String {
    use chrono::Local;
    Local::now().format("%Y%m%d_%H%M%S").to_string()
}


// ==================== VERSIONING ====================

/// Create a versioning snapshot by hardlinking the current .tar.zst backup
fn create_snapshot(backup_7z: &Path, versions_folder: &Path) {
    if !backup_7z.exists() { return; }
    let _ = std::fs::create_dir_all(versions_folder);
    let snapshot_dir = versions_folder.join(compact_timestamp());
    let _ = std::fs::create_dir_all(&snapshot_dir);
    let snapshot_7z = snapshot_dir.join(backup_7z.file_name().unwrap_or_default());
    // Hardlink the .tar.zst (near-zero space if unchanged)
    if std::fs::hard_link(backup_7z, &snapshot_7z).is_err() {
        let _ = std::fs::copy(backup_7z, &snapshot_7z);
    }
}

/// Delete old versioning snapshots
/// Keep only the N newest snapshots, delete the rest
/// L-3: ONE snapshot-name validator, used by clean_versions AND the GUI
/// versions list — 15 ASCII digits + underscore (YYYYMMDD_HHMMSS). Byte-based:
/// a multibyte 15-CHAR name must never slice-panic or delete-sort into the set.
pub fn is_snapshot_name(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 15
        && b[..8].iter().all(|c| c.is_ascii_digit())
        && b[8] == b'_'
        && b[9..].iter().all(|c| c.is_ascii_digit())
}

fn clean_versions(versions_folder: &Path, max_versions: usize) {
    if let Ok(entries) = std::fs::read_dir(versions_folder) {
        let mut snapshots: Vec<_> = entries
            .flatten()
            .filter(|e| is_snapshot_name(&e.file_name().to_string_lossy()))
            .collect();
        snapshots.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        for entry in snapshots.iter().skip(max_versions) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

// ==================== SYNC ENGINE ====================

pub fn sync_pair_to_cloud(source: &str, excluded: &[String], max_versions: i32, force: bool) -> (bool, String) {
    if source.is_empty() || !Path::new(source).exists() {
        return (false, "source does not exist".into());
    }

    // C3: hold the global mutation lock for the whole operation.
    let _pair_lock = match acquire_pair_lock(60_000) {
        Some(l) => l,
        None => return (false, "another LRGEX operation is running — try again in a moment".into()),
    };

    let leaf = Path::new(source).file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // Backup paths
    let backup_7z = config::backup_file_for(source);
    let sidecar = config::sidecar_for(source);
    let backup_dir = config::backup_dir_for(source);
    let _ = std::fs::create_dir_all(&backup_dir);
    let versions_folder = config::trash_path_for(source);

    // C2: one-time migration — rename old leaf-only dirs to keyed dirs (no-op if already keyed)
    config::migrate_pair_key(source);
    // v1.6.1: remove superseded leaf twins left behind by failed migrations
    config::cleanup_pair_orphan(source);

    // Migration: old root-level backup → delete (will re-compress to backup/ on next sync)
    // L-4: only when the app's OWN sidecar marker also exists — a lone
    // <leaf>.tar.zst could be a user file that happens to share the name.
    let old_root_backup = config::script_dir().join(format!("{}.tar.zst", leaf));
    let old_root_sidecar = config::script_dir().join(format!("{}.tar.zst.size", leaf));
    if old_root_backup.exists() && old_root_sidecar.exists() {
        let _ = std::fs::remove_file(&old_root_backup);
        let _ = std::fs::remove_file(&old_root_sidecar);
    }

    // C4/C-1: legacy raw-folder migration — SAFE resolver refuses reserved names
    // (backup/_versions/.lrgex) and the home itself, so protecting a folder named
    // "backup" can never compress-and-delete the app's own backup store.
    // Compress to TEMP, rename into place, delete raw folder ONLY on success.
    if let Some(old_backup_folder) = config::legacy_raw_folder(source) {
        if !backup_7z.exists() {
            // L-6d: lrgex_<pid>_migrate_* prefix so the orphan sweep can parse it
            let tmp = std::env::temp_dir()
                .join(format!("lrgex_{}_migrate_{}.tar.zst.tmp", std::process::id(), leaf));
            let (ok, _) = compress_folder(&old_backup_folder, &tmp, excluded, None);
            if ok && std::fs::rename(&tmp, &backup_7z).is_ok() {
                let _ = std::fs::remove_dir_all(&old_backup_folder);
            } else {
                let _ = std::fs::remove_file(&tmp);
                crate::synclog::write(&format!(
                    "  [MIGRATE-FAIL] kept raw folder for {} (compress failed)", leaf));
            }
        }
    }

    // Change detection — two tiers:
    // 1. FAST PATH: (size, count) from stats/manifest totals. Mismatch => changed, no hashing.
    // 2. CONTENT: sizes match => one full parallel hash pass. Catches same-size edits.
    //    Old-format sidecar (no "manifest:" prefix) => migration: treat as changed once.
    //    The manifest computed here is REUSED at the write site — one hash pass total.
    // H-2: root-unreadable => ABORT. An empty archive would replace a good
    // backup, and the next auto-restore would wipe the user's folder.
    let walked = match walk_tree(Path::new(source), excluded) {
        Some(w) => w,
        None => return (false, "source unreadable — refusing to overwrite the backup with an empty archive".into()),
    };
    // M-6: NEVER archive the app's own home (archive-of-archives = unbounded
    // growth + OneDrive re-upload storms). Filter walked entries under script_dir.
    // M-2: CASE-INSENSITIVE compare — Path::starts_with does no folding, so
    // "d:\cloud" vs home "D:\Cloud" silently re-enabled archive-of-archives.
    let home = config::script_dir();
    let home_lc = home.to_string_lossy().to_lowercase();
    let home_prefix = format!("{}{}", home_lc.trim_end_matches('\\'), "\\");
    let (files, walked_raw_count, mut current_size, mut current_count) = {
        let (f, s, c) = walked;
        (f, c, s, c)
    };
    let filtered: Vec<crate::sync::FileEnt> = files.into_iter()
        .filter(|e| {
            let p_lc = e.path.to_string_lossy().to_lowercase();
            let under_home = p_lc.starts_with(&home_prefix);
            if under_home {
                crate::synclog::write(&format!("  [EXCLUDE] home folder member skipped: {}", e.path.display()));
            }
            !under_home
        })
        .collect();
    if filtered.len() != current_count {
        current_size = filtered.iter().map(|e| e.size).sum();
        current_count = filtered.len();
    }
    let walked = (filtered, current_size, current_count);
    // H-2/M-3: a source reading ZERO files while a non-empty backup exists is
    // suspicious — refuse to destroy the chain silently. Three honest cases:
    //   (a) entire content was home members (M-6 filtered) — remove this pair.
    //   (b) force (re-add) — user's explicit act, bypass with a log line.
    //   (c) genuinely emptied/unreadable — refuse with an actionable message.
    // The OLD guard ran before the force check, turning "remove + re-add" into
    // a trap (M-3); force is now honored.
    if walked.2 == 0 && backup_7z.exists() {
        let (_, stored_count) = read_stored_stats(&sidecar);
        if stored_count > 0 {
            if walked_raw_count > 0 {
                // (a) home members were the ONLY content — archiving is impossible
                crate::synclog::write(&format!(
                    "  [GUARD] {} contains only LRGEX home members — this pair cannot be backed up", leaf));
                return (false, "this folder is inside the LRGEX home — remove it from the list (backing up the backup store is not supported)".into());
            }
            if force {
                // (b) explicit user action (re-add) — allow the empty backup
                crate::synclog::write(&format!(
                    "  [GUARD-BYPASS] {} archived EMPTY by explicit re-add (force)", leaf));
            } else {
                // (c) refuse — with an actionable message (M-3: the old escape
                // hint was a trap because force ran after this guard)
                crate::synclog::write(&format!(
                    "  [GUARD] {} reads as EMPTY but backup holds {} files — refusing to archive an empty set", leaf, stored_count));
                return (false, format!("folder is empty but the backup holds {} files — if you emptied it on purpose, remove the folder from the list and add it back", stored_count));
            }
        }
    }
    let current_size = walked.1;
    let current_count = walked.2;
    let (stored_size, stored_count) = read_stored_stats(&sidecar);
    let mut manifest: Option<Vec<FileMeta>> = None;

    let needs_backup = if force || !backup_7z.exists() {
        true
    } else if current_size != stored_size || current_count != stored_count {
        true // fast path: stats differ, no hashing needed yet
    } else {
        // Stats match — content check (None => old-format migration => re-backup)
        match read_stored_manifest(&sidecar) {
            None => true,
            Some(stored) => {
                // HASH-PROGRESS: hashing a 2 GB folder takes ~90 s — show it live.
                let hp = crate::synclog::Progress::new(&format!("Checking {}", leaf));
                hp.set_phase(0);
                hp.set_totals(current_count.max(1), current_size.max(1));
                let hw = hp.spawn_writer();
                let (changed, m) = detect_change(&stored, &walked.0, Some(&hp));
                hp.finish(4); // hashing done — compress_folder takes over the display
                let _ = hw.join();
                if changed { manifest = Some(m); }
                changed
            }
        }
    };

    if needs_backup {
        let mut snapshotted = false;
        // Something changed — create snapshot of old backup, then re-compress
        if backup_7z.exists() {
            create_snapshot(&backup_7z, &versions_folder);
            snapshotted = true;
        }
        // L1: clean_versions runs AFTER the successful swap below — a failed
        // compress must not burn the oldest snapshot slot.

        // Rule 3 (consistency): manifest reflects files that WERE archived.
        // Computed ONCE (possibly reused from detection), before compress consumes `walked`.
        let mut m = manifest.unwrap_or_else(|| {
            let hp = crate::synclog::Progress::new(&format!("Checking {}", leaf));
            hp.set_phase(0);
            hp.set_totals(current_count.max(1), current_size.max(1));
            let hw = hp.spawn_writer();
            let m = compute_manifest(&walked.0, Some(&hp));
            hp.finish(4);
            let _ = hw.join();
            m
        });

        // Compress source to temp, then move (atomic-ish)
        // PID-unique temp name prevents corruption if two processes ever collide
        let temp_7z = std::env::temp_dir().join(format!("lrgex_{}_{}.tar.zst.tmp", std::process::id(), leaf));
        let (compress_ok, skipped) = compress_folder(Path::new(source), &temp_7z, excluded, Some(walked));
        if compress_ok {
            // Files the archive actually skipped get hash=0 (locked at archive time).
            // Locked files keep their size so a future size change retries the backup,
            // but their content never reads as "changed" (rule 3).
            let skipped_keys: std::collections::HashSet<String> =
                skipped.iter().map(|s| s.replace('\\', "/")).collect();
            for meta in &mut m {
                if skipped_keys.contains(&meta.p) {
                    meta.h = 0;
                }
            }
            // M3: NO remove_file first — std::fs::rename on Windows replaces an
            // existing file atomically (MOVEFILE_REPLACE_EXISTING). The old
            // remove+rename pair left a crash window with NO live archive.
            if std::fs::rename(&temp_7z, &backup_7z).is_ok() {
                clean_versions(&versions_folder, max_versions as usize); // L1: after success
                write_stored_manifest(&sidecar, &m);
            } else {
                // Rename failed (OneDrive lock?) — try copy + delete
                if std::fs::copy(&temp_7z, &backup_7z).is_ok() {
                    // M-2: copy is NOT atomic — verify size match + zstd decodability
                    // before trusting it. A torn copy with the OLD sidecar intact
                    // would read "No changes" forever on a dead archive.
                    let temp_len = std::fs::metadata(&temp_7z).map(|m| m.len()).unwrap_or(u64::MAX);
                    let dest_len = std::fs::metadata(&backup_7z).map(|m| m.len()).unwrap_or(0);
                    // INFO hardening: FULL decode probe (not header-only) — same-length
                    // mid-stream corruption is caught before we trust the copy.
                    let decode_ok = std::fs::File::open(&backup_7z)
                        .ok()
                        .and_then(|f| zstd::stream::read::Decoder::new(f).ok())
                        .map(|mut dec| std::io::copy(&mut dec, &mut std::io::sink()).is_ok())
                        .unwrap_or(false);
                    if temp_len == dest_len && decode_ok {
                        let _ = std::fs::remove_file(&temp_7z);
                        clean_versions(&versions_folder, max_versions as usize); // L1: after success
                        write_stored_manifest(&sidecar, &m);
                    } else {
                        let _ = std::fs::remove_file(&temp_7z);
                        // The torn copy ALREADY replaced backup_7z — verify whether the
                        // pre-change snapshot actually survived before claiming it.
                        let snapshot_ok = {
                            let mut found = false;
                            if let Ok(entries) = std::fs::read_dir(&versions_folder) {
                                for e in entries.flatten() {
                                    if is_snapshot_name(&e.file_name().to_string_lossy()) {
                                        found = true;
                                        break;
                                    }
                                }
                            }
                            found
                        };
                        if snapshot_ok {
                            crate::synclog::write(&format!(
                                "  [VERIFY-FAIL] copy of {} was torn — live archive DAMAGED; good copy confirmed in _versions", leaf));
                            return (false, "archive damaged during copy — restore from Versions".into());
                        } else {
                            crate::synclog::write(&format!(
                                "  [VERIFY-FAIL] copy of {} was torn AND no snapshot exists — data at risk, source files are intact", leaf));
                            return (false, "archive damaged during copy and no version snapshot exists — the SOURCE files are still intact; re-add the folder to rebuild".into());
                        }
                    }
                } else {
                    let _ = std::fs::remove_file(&temp_7z);
                    return (false, "rename failed".into());
                }
            }
        } else {
            let _ = std::fs::remove_file(&temp_7z);
            return (false, "compression failed".into());
        }

        let size_mb = current_size as f64 / 1_048_576.0;
        let skip_msg = if skipped.is_empty() {
            String::new()
        } else {
            format!(" {} file(s) skipped (locked). Archive may be incomplete.", skipped.len())
        };
        let msg = if snapshotted {
            format!("Backed up {} files ({:.1} MB). Previous version archived.{}", current_count, size_mb, skip_msg)
        } else {
            format!("Backed up {} files ({:.1} MB).{}", current_count, size_mb, skip_msg)
        };
        return (true, msg);
    }

    (true, "No changes \u{2014} up to date.".into())
}

/// Pre-check ALL folders before restoring ANY. Returns list of failures (path, reason).
/// If ANY failure exists, the caller must abort the entire restore — no partial restores.
pub fn pre_check_restore(paths: &[String]) -> Vec<(String, String)> {
    let mut failures = Vec::new();
    use std::io::Read;

    for path in paths {
        let leaf = std::path::Path::new(path).file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // L-1: do NOT migrate here — restore_pair_from_cloud migrates under the
        // lock; an unlocked rename from the pre-check violates C3 and converges
        // to a redundant re-backup at best, a lost race at worst.
        let backup = config::backup_file_for(path);

        // H-1: restorable-predicate PARITY with restore_pair_from_cloud — the
        // old check accepted only the keyed archive, so an unmigrated pre-C2
        // store (leaf-only, or v1.3 raw folder) aborted the GUI Restore flow
        // with a false "backup missing" BEFORE the code that would migrate it
        // could ever run. Read-only existence checks; migration still happens
        // under the lock in restore.
        let restorable = backup.exists()
            // pre-C2 leaf-only dir with archive inside (migration will handle it)
            || config::script_dir().join("backup").join(&leaf).join(format!("{}.tar.zst", leaf)).exists()
            // v1.3-era raw folder (restore_pair's own robocopy fallback)
            || config::legacy_raw_folder(path).is_some();

        // 1. Backup archive exists (in any supported form)?
        if !restorable {
            failures.push((leaf, "backup missing".into()));
            continue;
        }

        // 2. Archive is valid zstd? (check magic bytes: 28 b5 2f fd)
        //    Only when the KEYED archive exists — a legacy fallback (raw folder)
        //    is restored by robocopy, not by zstd decode, so magic check N/A.
        if backup.exists() {
            let mut header = [0u8; 4];
            let valid_zstd = std::fs::File::open(&backup)
                .and_then(|mut f| f.read_exact(&mut header).map(|_| f))
                .map(|_| header == [0x28, 0xb5, 0x2f, 0xfd])
                .unwrap_or(false);
            if !valid_zstd {
                failures.push((leaf, "backup archive is corrupt or incomplete".into()));
                continue;
            }
        }
        // 3. Destination is writable? (report ACTUAL error, not generic)
        let dest = std::path::Path::new(path);
        let can_write = if dest.exists() {
            let test = dest.join(".lrgex_write_test");
            match std::fs::File::create(&test) {
                Ok(_) => { let _ = std::fs::remove_file(&test); true }
                Err(e) => { failures.push((leaf, format!("write test failed: {}", e))); continue; }
            }
        } else {
            match std::fs::create_dir_all(dest) {
                Ok(_) => true,
                Err(e) => { failures.push((leaf, format!("cannot create folder: {}", e))); continue; }
            }
        };
        let _ = can_write;
    }

    failures
}

pub fn restore_pair_from_cloud(source: &str) -> (bool, String) {
    // C3: hold the global mutation lock for the whole operation.
    let _pair_lock = match acquire_pair_lock(120_000) {
        Some(l) => l,
        None => return (false, "another LRGEX operation is running — try again in a moment".into()),
    };
    let leaf = Path::new(source).file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // C2: migrate BEFORE resolving keyed paths — a fresh reinstall may run
    // restore before any sync ever fires (OneDrive carried the old leaf dirs).
    config::migrate_pair_key(source);
    let backup_7z = config::backup_file_for(source);

    // Try .tar.zst first (new format)
    if backup_7z.exists() {
        let (ok, msg) = decompress_archive(&backup_7z, Path::new(source));
        if ok {
            return (true, String::new());
        }
        return (false, if msg.is_empty() { "decompression failed".into() } else { msg });
    }

    // Fallback: old raw folder format
    // C-1: safe resolver — never robocopy from the app's own store (reserved
    // names / home structure) into a user source folder.
    if let Some(old_backup) = config::legacy_raw_folder(source) {
        return robocopy_tree(&old_backup, source);
    }

    (false, "backup missing".into())
}

/// L-8: single robocopy-tree helper (was duplicated verbatim in two restores).
pub fn robocopy_tree(src: &Path, dst: &str) -> (bool, String) {
    let _ = std::fs::create_dir_all(dst);
    let args: Vec<String> = vec![
        src.to_string_lossy().to_string(), dst.into(),
        "/E".into(), "/XJ".into(), "/NFL".into(), "/NDL".into(),
        "/NJH".into(), "/NJS".into(), "/NP".into(), "/R:5".into(), "/W:5".into(),
    ];
    let output = Command::new("robocopy.exe").args(&args)
        .creation_flags(0x08000000).output();
    match output {
        Ok(out) => {
            let code = out.status.code().unwrap_or(16);
            if code < 8 { (true, String::new()) }
            else { (false, format!("robocopy exit {}", code)) }
        }
        Err(e) => (false, e.to_string()),
    }
}

pub fn sync_all_pairs() {
    let cfg = config::load_config();
    let mut ok = 0i32;
    let mut fail = 0i32;
    let mut restored = 0i32;
    let mut lock_skipped = 0i32;
    let mut restored_names: Vec<String> = vec![];

    crate::synclog::write("------------------------------------------------------------");
    crate::synclog::write(&format!("Sync cycle — {}", crate::crashlog::version_stamp()));
    crate::synclog::write_progress("");

    for j in &cfg.junctions {
        let leaf = Path::new(&j.source_path).file_name()
            .map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        // M-5: re-validate against the LIVE config each iteration — a GUI
        // Remove mid-cycle must not let this stale snapshot resurrect the
        // deleted pair's backup (multi-GB orphan the user was told was gone).
        let live = config::load_config();
        if !live.junctions.iter().any(|lj| config::same_path(&lj.source_path, &j.source_path)) {
            crate::synclog::write(&format!("  [SKIP] {}  -  removed from config mid-cycle", leaf));
            continue;
        }
        let missing = !Path::new(&j.source_path).exists() || is_dir_empty(&j.source_path);

        if missing && j.auto_restore {
            let (success, reason) = restore_pair_from_cloud(&j.source_path);
            if success {
                restored += 1;
                restored_names.push(leaf.clone());
                crate::synclog::write(&format!("  [RESTORE] {}  -  was missing, restored", leaf));
                // Set migration marker so future syncs check for new game-created ID folders
                set_migration_pending();
                // Also try migration now (game might have already created new ID)
                let mig = migrate_save_ids(Path::new(&j.source_path));
                for m in &mig { crate::synclog::write(&format!("  [MIGRATE] {}", m)); }
            } else if reason.contains("another LRGEX operation") {
                // M-4: lock held by a concurrent GUI operation — skip, not fail
                lock_skipped += 1;
                crate::synclog::write(&format!("  [SKIP] {}  -  concurrent operation holds the lock", leaf));
            } else {
                fail += 1;
                crate::synclog::write(&format!("  [FAIL] {}  -  restore failed", leaf));
            }
        } else {
            crate::synclog::write_progress(&format!("Compressing {}...", leaf));
            let (success, reason) = sync_pair_to_cloud(&j.source_path, &cfg.excluded_names, cfg.max_versions, false);
            crate::synclog::write_progress("");
            if success {
                ok += 1;
                crate::synclog::write(&format!("  [ OK ] {}", leaf));
                // Only scan for migration if a restore happened recently (post-restore gate)
                if migration_pending() {
                    let mig = migrate_save_ids(Path::new(&j.source_path));
                    for m in &mig { crate::synclog::write(&format!("  [MIGRATE] {}", m)); }
                }
            } else {
                // M-4: lock-timeout is NOT a failure — a long GUI restore holding the
                // global lock made every overlapping scheduled cycle report a
                // false RED. Skip, don't fail.
                if reason.contains("another LRGEX operation") {
                    lock_skipped += 1;
                    crate::synclog::write(&format!("  [SKIP] {}  -  concurrent operation holds the lock", leaf));
                } else {
                    fail += 1;
                    crate::synclog::write(&format!("  [FAIL] {}  -  {}", leaf, reason));
                }
            }
        }
    }

    // M-4: an all-skipped cycle (lock held by a long GUI restore) carries ZERO
    // information — writing "0 folders protected" would overwrite a good status
    // for up to a full interval. Keep the previous status instead.
    if ok + restored + fail == 0 && lock_skipped > 0 {
        crate::synclog::write(&format!("Done: all {} skipped (lock held) — previous status kept.", lock_skipped));
        return;
    }
    crate::synclog::write(&format!("Done: {} compressed, {} restored, {} failed.", ok, restored, fail));
    crate::health::write_status(ok + restored, fail, restored, &restored_names);
    // INTERVAL-GUARD: only a REAL cycle (something attempted) stamps the marker —
    // an all-skip cycle (lock held) must not defer the next scheduled run.
    if ok + restored + fail > 0 {
        config::write_last_sync_marker();
    }
}

// ==================== RESTORE FROM SNAPSHOT ====================

/// Restore a specific snapshot version to the source location
pub fn restore_snapshot(snapshot_dir: &Path, source: &str) -> (bool, String) {
    // C3: hold the global mutation lock for the whole operation.
    let _pair_lock = match acquire_pair_lock(120_000) {
        Some(l) => l,
        None => return (false, "another LRGEX operation is running — try again in a moment".into()),
    };
    // Look for .tar.zst file in the snapshot directory
    if let Ok(entries) = std::fs::read_dir(snapshot_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "zst").unwrap_or(false) {
                let _ = std::fs::create_dir_all(source);
                let (ok, msg) = decompress_archive(&path, Path::new(source));
                if ok {
                    return (true, String::new());
                }
                return (false, if msg.is_empty() { "decompression failed".into() } else { msg });
            }
        }
    }
    // Fallback: old-style raw files snapshot
    robocopy_tree(snapshot_dir, source)
}

/// List files inside a .tar.zst archive (without extracting)

// ==================== SCHEDULED TASK ====================

/// Register (or update) the Windows Scheduled Task.
/// Uses CANONICAL HOME from registry — never current_exe().
/// This prevents stray copies from retargeting the task.

/// Delete old VBS-based scheduled tasks from the PowerShell version.

/// Delete old VBS-based scheduled tasks from the PowerShell version.
/// Prevents "cannot find sync-runner.vbs" errors for users upgrading from old version.
/// Scans every launch on background thread. Catches old VBS tasks from any previous version.
/// H-1: exact ISO-8601 duration (pure — unit-tested). Time designator T is
/// REQUIRED before any H/M component; without it P30M means 30 MONTHS and
/// schtasks rejects the XML silently. Days precede T: P1DT2H30M.
pub fn interval_xml(minutes: i64) -> String {
    let m = minutes.max(1) as u64;
    let (d, h, min) = (m / 1440, (m % 1440) / 60, m % 60);
    let mut s = String::from("P");
    if d > 0 { s.push_str(&format!("{}D", d)); }
    let has_time = h > 0 || min > 0 || d == 0; // exact-days edge: P1D stays P1D
    if has_time {
        s.push('T');
        if h > 0 { s.push_str(&format!("{}H", h)); }
        if min > 0 { s.push_str(&format!("{}M", min)); }
        if h == 0 && min == 0 { s.push_str("0M"); } // d==0 && m<60 && h==0 && min==0 can't happen (m>=1), but P<1H alone is never emitted
    }
    s
}

pub fn register_sync_task(interval_minutes: i32) -> bool {
    let home = match config::canonical_home() {
        Some(h) => h,
        None => return false,
    };
    let exe = home.join("LRGEXRestore.exe");
    let exe_str = exe.to_string_lossy();

    // XML task definition — uses StartWhenAvailable=true so missed runs
    // (PC off/asleep) are caught up on wake. This is the ROOT FIX for
    // stale backups on machines that sleep at the scheduled time.
    let interval_xml = interval_xml(interval_minutes as i64);

    // Escape XML special chars in the exe path.
    let exe_escaped = exe_str
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");

    let xml = format!(
        r#"<?xml version="1.0"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>LRGEX Restore automatic backup sync</Description>
  </RegistrationInfo>
  <Triggers>
    <TimeTrigger>
      <Repetition>
        <Interval>{interval}</Interval>
        <StopAtDurationEnd>false</StopAtDurationEnd>
      </Repetition>
      <StartBoundary>2026-01-01T00:00:00</StartBoundary>
      <Enabled>true</Enabled>
    </TimeTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT2H</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>-sync</Arguments>
    </Exec>
  </Actions>
</Task>"#,
        interval = interval_xml,
        exe = exe_escaped
    );

    let xml_path = std::env::temp_dir().join("lrgex-task.xml");
    if std::fs::write(&xml_path, &xml).is_err() {
        return false;
    }

    let result = match Command::new("schtasks.exe")
        .args([
            "/Create",
            "/XML", &xml_path.to_string_lossy(),
            "/TN", "LRGEX-Restore-Rust",
            "/F",
        ])
        .creation_flags(0x08000000u32)
        .output()
    {
        Ok(out) => out.status.success(),
        Err(_) => false,
    };
    let _ = std::fs::remove_file(&xml_path);
    result
}
// ==================== SAVE-ID MIGRATION ====================

/// Directories that never contain game saves. Skipped during scanning to avoid
/// false positives (e.g. node_modules/es-abstract/2015/) and wasted traversal.
const SKIP_DIRS: &[&str] = &[
    "node_modules", ".git", "target", "build", "__pycache__",
    ".venv", "venv", "dist", ".cache", ".npm", ".cargo",
    "dependencies", "packages", "system32", "winsxs",
];

/// Returns true if a directory name should be skipped during scanning.
fn should_skip(name: &str) -> bool {
    let lower = name.to_lowercase();
    SKIP_DIRS.iter().any(|s| *s == lower)
}

/// Public wrapper for other modules (gamescan discovery).
pub fn should_skip_pub(name: &str) -> bool {
    should_skip(name)
}

/// Marker file: set after restore so syncs know to check for new game ID folders.
/// Auto-expires after 7 days. Cleared after successful migration.
pub fn set_migration_pending() {
    let _ = std::fs::write(config::data_dir().join("migration-pending"), "");
}

pub fn clear_migration_pending() {
    let _ = std::fs::remove_file(config::data_dir().join("migration-pending"));
}

fn migration_pending() -> bool {
    let marker = config::data_dir().join("migration-pending");
    if !marker.exists() { return false; }
    if let Ok(meta) = std::fs::metadata(&marker) {
        if let Ok(mtime) = meta.modified() {
            if mtime.elapsed().unwrap_or_default().as_secs() < 7 * 86400 {
                return true;
            }
        }
    }
    let _ = std::fs::remove_file(&marker); // Expired — clean up
    false
}

/// Cheap check (file existence only) for the health timer.
/// Avoids spawning a thread every 30s when no restore has happened.
pub fn migration_marker_exists() -> bool {
    config::data_dir().join("migration-pending").exists()
}

/// Shallow-ish check: does this folder contain game saves at any reasonable depth?
/// Recurses up to 4 levels into non-numeric subdirs, stops at first match.
/// Used for the UI lamp indicator. Diagnostic: if lamp is off but saves exist,
/// the detection patterns need updating.
pub fn is_game_folder(path: &str) -> bool {
    let mut visited = 0usize;
    check_game_at_depth(std::path::Path::new(path), 0, &mut visited)
}

fn check_game_at_depth(dir: &Path, depth: usize, visited: &mut usize) -> bool {
    if depth > 4 { return false; }
    // Cap: bail after 300 dirs visited. Real game saves are found within
    // the first few dozen dirs. This bounds the worst case for large trees.
    if *visited > 300 { return false; }
    *visited += 1;

    let save_dirs = ["savedata", "save", "saves", "SaveGames", "SaveData",
                     "remote", "profiles", "slot", "slots",
                     "saved games", "savegame", "saved"];

    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut numeric_dirs: Vec<std::path::PathBuf> = Vec::new();
        let mut other_dirs: Vec<std::path::PathBuf> = Vec::new();

        for entry in entries.flatten() {
            if !entry.path().is_dir() { continue; }
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip known non-game directories (node_modules, .git, target, etc.)
            if should_skip(&name) { continue; }
            if name.chars().all(|c| c.is_ascii_digit()) {
                numeric_dirs.push(entry.path());
            } else {
                other_dirs.push(entry.path());
            }
        }

        // Check if any numeric dir has a recognized save subdir with files
        for nd in &numeric_dirs {
            for sd in &save_dirs {
                let p = nd.join(sd);
                if p.is_dir() {
                    if let Ok(files) = std::fs::read_dir(&p) {
                        if files.flatten().next().is_some() { return true; }
                    }
                }
            }
            // Also check depth 2: numeric/<appID>/remote (Steam pattern)
            if let Ok(sub) = std::fs::read_dir(nd) {
                for s in sub.flatten() {
                    if s.path().is_dir() {
                        let remote = s.path().join("remote");
                        if remote.is_dir() {
                            if let Ok(files) = std::fs::read_dir(&remote) {
                                if files.flatten().next().is_some() { return true; }
                            }
                        }
                    }
                }
            }
        }

        // Recurse into non-numeric subdirs (stop at first match)
        for od in &other_dirs {
            if check_game_at_depth(od, depth + 1, visited) { return true; }
        }
    }
    false
}

pub fn migrate_save_ids(folder: &Path) -> Vec<String> {
    let mut migrations = Vec::new();
    scan_for_id_folders(folder, &mut migrations);
    migrations
}

/// Lightweight check for GUI startup: only runs if marker exists.
/// Returns migration messages (empty = nothing to do or no marker).
/// Mutex prevents concurrent migration runs (scheduled sync, post-launch check,
/// health timer can all fire at once after a restore).
static MIGRATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn run_migration_check() -> Vec<String> {
    // Only one migration run at a time — skip if another thread is already migrating
    let _lock = match MIGRATION_LOCK.try_lock() {
        Ok(guard) => guard,
        Err(_) => return vec![],
    };
    if !migration_pending() { return vec![]; }
    let cfg = config::load_config();
    let mut all = Vec::new();
    for j in &cfg.junctions {
        let mig = migrate_save_ids(Path::new(&j.source_path));
        for m in &mig { all.push(m.clone()); }
    }
    // Only clear marker if a real migration happened (not REFUSED, not empty).
    // This way: first launch (games not installed yet) → marker stays →
    // next launch (after games installed) → migration runs → marker cleared.
    // Tradeoff: repeated scans on each launch until migration succeeds or
    // 7-day expiry. Acceptable — runs on background thread, scans are lightweight.
    let has_real_migration = all.iter().any(|m| !m.starts_with("REFUSED"));
    if has_real_migration {
        clear_migration_pending();
    }
    all
}

/// Universal scan: walk the tree looking for ANY directory that has 2+ numeric
/// subdirectories. Works for users\<ID>, userdata\<ID>, <game-name>\<ID>, etc.
/// Does NOT recurse into numeric subdirs (they're leaf ID folders, not nesting levels).
/// Safety: source must have savedata\/remote\ with files, target must be empty (≤5 files).
fn scan_for_id_folders(dir: &Path, migrations: &mut Vec<String>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut numeric_subdirs: Vec<(std::path::PathBuf, String)> = Vec::new();
        let mut other_subdirs: Vec<std::path::PathBuf> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() { continue; }
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip known non-game directories (node_modules, .git, target, etc.)
            if should_skip(&name) { continue; }

            if name.chars().all(|c| c.is_ascii_digit()) {
                numeric_subdirs.push((path, name)); // Leaf ID folder — don't recurse
            } else {
                other_subdirs.push(path); // Non-numeric — safe to recurse
            }
        }

        // If this directory has 2+ numeric subdirs, check for migration
        if numeric_subdirs.len() >= 2 {
            try_migrate(dir, &numeric_subdirs, migrations);
        }

        // Only recurse into non-numeric subdirs
        for path in &other_subdirs {
            scan_for_id_folders(path, migrations);
        }
    }
}

fn try_migrate(parent: &Path, numeric: &[(std::path::PathBuf, String)], migrations: &mut Vec<String>) {
    // Classify each numeric folder
    let mut classified: Vec<(&std::path::PathBuf, &String, SaveState, usize, u64, std::time::SystemTime)> = Vec::new();

    for (id_path, id_name) in numeric {
        let (state, save_count, save_bytes, mtime) = classify_folder(id_path);
        classified.push((id_path, id_name, state, save_count, save_bytes, mtime));
    }

    // Three-state: if ANY folder is Unknown → refuse migration entirely
    let unknown_names: Vec<&str> = classified.iter()
        .filter(|(_, _, s, _, _, _)| *s == SaveState::Unknown)
        .map(|(_, name, _, _, _, _)| name.as_str())
        .collect();
    if !unknown_names.is_empty() {
        migrations.push(format!("REFUSED — ambiguous folder(s): {}", unknown_names.join(", ")));
        return;
    }

    let sources: Vec<_> = classified.iter().filter(|(_, _, s, _, _, _)| *s == SaveState::HasSaveData).collect();
    let targets: Vec<_> = classified.iter().filter(|(_, _, s, _, _, _)| *s == SaveState::FreshInstall).collect();

    if sources.is_empty() || targets.is_empty() { return; }

    // Rank sources: save_count DESC → save_bytes DESC → mtime DESC → name ASC
    let source = sources.iter().max_by(|a, b| {
        a.3.cmp(&b.3)              // save_count: more = better
            .then_with(|| a.4.cmp(&b.4))  // save_bytes: more = better
            .then_with(|| a.5.cmp(&b.5))  // mtime: newer = better
    }).unwrap();

    for (_, tgt_name, _, _, _, _) in &targets {
        let target = parent.join(tgt_name);
        copy_dir_merge(source.0, &target);
        let msg = format!("{}: {} → {}", parent.file_name()
            .map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), source.1, tgt_name);
        migrations.push(msg);
    }
}

/// Three-state classification: HasSaveData / FreshInstall / Unknown.
/// If ANY folder in a group is Unknown → refuse migration entirely (conservative).
#[derive(Clone, Copy, PartialEq)]
enum SaveState {
    HasSaveData,
    FreshInstall,
    Unknown,
}

/// Score a folder for save data presence. Returns (state, save_file_count, save_bytes, mtime).
/// Score >= 100 = HasSaveData, score == 0 = FreshInstall, 0 < score < 100 = Unknown.
fn classify_folder(dir: &Path) -> (SaveState, usize, u64, std::time::SystemTime) {
    let mut score = 0i32;

    // STRONG signal (+100): recognized save directory with files
    let save_dirs = ["savedata", "save", "saves", "SaveGames", "SaveData",
                     "remote", "profiles", "slot", "slots",
                     "saved games", "savegame", "saved"];
    for sd in &save_dirs {
        let p = dir.join(sd);
        if p.is_dir() && dir_has_files_recursive(&p) {
            score += 100;
            break;
        }
    }

    // STRONG signal (+100): userdata/<userID>/<appID>/remote pattern (depth 2)
    if score < 100 {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let remote = entry.path().join("remote");
                    if remote.is_dir() && dir_has_files_recursive(&remote) {
                        score += 100;
                        break;
                    }
                }
            }
        }
    }

    // MEDIUM signal (+50): specific save filenames anywhere in tree
    // (SAVEFILE*, autosave*, checkpoint* — these are strong save names)
    if score < 100 && has_specific_save_names(dir) {
        score += 50;
    }

    // WEAK signal (+30): save extensions (*.sav, *.save, *.slot)
    if score < 100 && has_save_extensions(dir) {
        score += 30;
    }

    // WEAK signal (+20): 6+ files with no recognized patterns (might be unrecognized format)
    if score == 0 {
        let (total_files, _) = count_files_recursive(dir);
        if total_files >= 6 {
            score += 20;
        }
    }

    let state = if score >= 100 {
        SaveState::HasSaveData
    } else if score == 0 {
        SaveState::FreshInstall
    } else {
        SaveState::Unknown
    };

    let (save_count, save_bytes, mtime) = compute_save_stats(dir);
    (state, save_count, save_bytes, mtime)
}

/// Check for specific save filenames (SAVEFILE*, autosave*, checkpoint*).
fn has_specific_save_names(dir: &Path) -> bool {
    fn check(d: &Path) -> bool {
        if let Ok(entries) = std::fs::read_dir(d) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if check(&path) { return true; }
                } else {
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    if name.starts_with("savefile")
                        || name.starts_with("autosave")
                        || name.starts_with("checkpoint") {
                        return true;
                    }
                }
            }
        }
        false
    }
    check(dir)
}

/// Check for save extensions (*.sav, *.save, *.slot).
fn has_save_extensions(dir: &Path) -> bool {
    fn check(d: &Path) -> bool {
        if let Ok(entries) = std::fs::read_dir(d) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if check(&path) { return true; }
                } else {
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    if name.ends_with(".sav")
                        || name.ends_with(".save")
                        || name.ends_with(".slot") {
                        return true;
                    }
                }
            }
        }
        false
    }
    check(dir)
}

/// Compute total file count, total bytes, and newest mtime for ranking.
fn compute_save_stats(dir: &Path) -> (usize, u64, std::time::SystemTime) {
    let mut count = 0usize;
    let mut bytes = 0u64;
    let mut newest = std::time::UNIX_EPOCH;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let (c, b, m) = compute_save_stats(&path);
                count += c;
                bytes += b;
                if m > newest { newest = m; }
            } else {
                count += 1;
                if let Ok(meta) = entry.metadata() {
                    bytes += meta.len();
                    if let Ok(m) = meta.modified() {
                        if m > newest { newest = m; }
                    }
                }
            }
        }
    }
    (count, bytes, newest)
}

fn dir_has_files_recursive(dir: &Path) -> bool {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if dir_has_files_recursive(&path) { return true; }
            } else {
                return true; // Found at least one file
            }
        }
    }
    false
}

fn count_files_recursive(dir: &Path) -> (usize, std::time::SystemTime) {
    let mut count = 0usize;
    let mut newest = std::time::UNIX_EPOCH;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let (sub_count, sub_mtime) = count_files_recursive(&path);
                count += sub_count;
                if sub_mtime > newest { newest = sub_mtime; }
            } else {
                count += 1;
                if let Ok(m) = entry.metadata().and_then(|m| m.modified()) {
                    if m > newest { newest = m; }
                }
            }
        }
    }
    (count, newest)
}

fn copy_dir_merge(source: &Path, target: &Path) {
    if let Ok(entries) = std::fs::read_dir(source) {
        for entry in entries.flatten() {
            let src_path = entry.path();
            let name = entry.file_name();
            let tgt_path = target.join(&name);

            if src_path.is_dir() {
                if !tgt_path.exists() {
                    let _ = std::fs::create_dir_all(&tgt_path);
                }
                copy_dir_merge(&src_path, &tgt_path);
            } else {
                // Never overwrite — preserves game's fresh screeninfo.cfg etc.
                if !tgt_path.exists() {
                    let _ = std::fs::copy(&src_path, &tgt_path);
                }
            }
        }
    }
}

