use crate::config;
use rayon::prelude::*;
use std::io::{BufWriter, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};

pub fn is_dir_empty(path: &str) -> bool {
    match std::fs::read_dir(path) {
        Ok(mut entries) => entries.next().is_none(),
        Err(_) => true,
    }
}

// ==================== COMPRESSION (tar + zstd) ====================

const BIG_FILE: u64 = 8 * 1024 * 1024; // stream these instead of preloading
const BATCH: usize = 2048;             // ~8 MB resident for 4 KB files

pub struct FileEnt {
    pub path: PathBuf,
    pub rel: PathBuf,
    pub size: u64,
}

/// SINGLE WALK — replaces compute_stats + collect_files.
/// Returns (entries, total_bytes, count). Uses DirEntry::metadata() which on
/// Windows is served from the directory enumeration cache (no extra syscall).
pub fn walk_tree(base: &Path, excluded: &[String]) -> (Vec<FileEnt>, u64, usize) {
    let mut out = Vec::with_capacity(4096);
    let mut total = 0u64;
    walk_inner(base, base, excluded, &mut out, &mut total);
    let count = out.len();
    (out, total, count)
}

fn walk_inner(base: &Path, current: &Path, excluded: &[String], out: &mut Vec<FileEnt>, total: &mut u64) {
    let entries = match std::fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_s = name.to_string_lossy();
        if excluded.iter().any(|e| e.as_str() == name_s.as_ref()) { continue; }

        let ft = match entry.file_type() { Ok(t) => t, Err(_) => continue };
        if ft.is_symlink() { continue; }

        let path = entry.path();
        if ft.is_dir() {
            walk_inner(base, &path, excluded, out, total);
        } else {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            *total += size;
            let rel = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
            out.push(FileEnt { path, rel, size });
        }
    }
}

/// Exact header construction for tar 0.4 (deterministic: mtime/uid/gid zeroed, 0o644).
fn make_header(size: u64) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Regular);
    h.set_size(size);
    h.set_mode(0o644);
    h.set_uid(0);
    h.set_gid(0);
    h.set_mtime(0);
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

    /// Helper: create a temp test folder with known files
    fn make_test_folder() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lrgex_test_{}", std::process::id()));
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
        let (files, total_bytes, count) = walk_tree(&dir, &[]);
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
        let (files, _, count) = walk_tree(&dir, &excluded);
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
        let (files, _, count) = walk_tree(&dir, &[]);
        println!("\n=== WALK_TREE SYMLINKS ===");
        println!("  files: {} (symlinks should be skipped)", count);
        assert_eq!(count, 4, "Symlink should be skipped, still 4 files");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_walk_tree_empty_folder() {
        let dir = std::env::temp_dir().join(format!("lrgex_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (files, bytes, count) = walk_tree(&dir, &[]);
        assert_eq!(count, 0, "Empty folder should have 0 files");
        assert_eq!(bytes, 0, "Empty folder should have 0 bytes");
        assert!(files.is_empty(), "File list should be empty");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_walk_tree_byte_count_accurate() {
        let dir = std::env::temp_dir().join(format!("lrgex_bytes_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"12345").unwrap(); // 5 bytes
        std::fs::write(dir.join("b.txt"), b"1234567890").unwrap(); // 10 bytes
        let (_, total, count) = walk_tree(&dir, &[]);
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
        let extract_dir = std::env::temp_dir().join(format!("lrgex_test_extract_{}", std::process::id()));
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
        let extract_dir = std::env::temp_dir().join(format!("lrgex_test_excl_extract_{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("lrgex_large_{}", std::process::id()));
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
        let extract_dir = std::env::temp_dir().join(format!("lrgex_large_extract_{}", std::process::id()));
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

        let extract_dir = std::env::temp_dir().join(format!("lrgex_hermes_verify_{}", std::process::id()));
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
    let heartbeat = progress.spawn_writer();
    progress.set_phase(0); // walk

    let t_walk = std::time::Instant::now();
    let (files, total_bytes, count) = match prewalked {
        Some(w) => w,
        None => walk_tree(source, excluded),
    };
    let walk = t_walk.elapsed();
    progress.set_totals(count, total_bytes);
    progress.set_phase(1); // compress

    let file = match std::fs::File::create(dest) {
        Ok(f) => f,
        Err(_) => return (false, vec![]),
    };
    let writer = BufWriter::with_capacity(4 * 1024 * 1024, file);

    let mut encoder = match zstd::Encoder::new(writer, 1) {
        Ok(e) => e,
        Err(_) => return (false, vec![]),
    };
    let threads = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
    let _ = encoder.multithread(threads);
    let _ = encoder.include_checksum(false);
    let mut builder = tar::Builder::new(encoder.auto_finish());

    let t_append = std::time::Instant::now();
    let mut processed = 0usize;
    let mut skipped: Vec<String> = Vec::new();

    for batch in files.chunks(BATCH) {
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
                    let mut h = make_header(buf.len() as u64);
                    let mut slice: &[u8] = buf.as_slice();
                    builder.append_data(&mut h, &e.rel, &mut slice)
                }
                Some(Err(err)) => Err(err),
                None => {
                    match std::fs::File::open(&e.path) {
                        Ok(f) => match f.metadata() {
                            Ok(m) => {
                                let mut h = make_header(m.len());
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

pub fn decompress_archive(archive: &Path, dest: &Path) -> (bool, String) {
    let _ = std::fs::create_dir_all(dest);
    let temp_dir = dest.parent().unwrap_or(std::path::Path::new("."))
        .join(format!(".lrgex_restore_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = std::fs::create_dir_all(&temp_dir);

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

    match tar.unpack(&temp_dir) {
        Ok(_) => {
            stop.store(true, Ordering::Relaxed);
            crate::synclog::write_progress("");
            let ec = std::fs::read_dir(&temp_dir).map(|d| d.count()).unwrap_or(0);
            crate::synclog::write(&format!("  [DECOMPRESS] unpack OK - {} entries, {} bytes archive", ec, arch_size));
            let backup_name = dest.with_extension("lrgex_bak");
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
        Err(e) => {
            stop.store(true, Ordering::Relaxed);
            crate::synclog::write_progress("");
            let _ = std::fs::remove_dir_all(&temp_dir);
            (false, format!("extract failed: {}", e))
        }
    }
}

// compute_stats removed — merged into walk_tree (single shared walk for change-detection + compression)

/// Read stored source stats from sidecar file
fn read_stored_stats(sidecar: &Path) -> (u64, usize) {
    std::fs::read_to_string(sidecar)
        .ok()
        .and_then(|s| {
            let parts: Vec<&str> = s.trim().split(',').collect();
            if parts.len() == 2 {
                Some((parts[0].parse().ok()?, parts[1].parse().ok()?))
            } else {
                None
            }
        })
        .unwrap_or((u64::MAX, usize::MAX))
}

/// Write source stats to sidecar file
fn write_stored_stats(sidecar: &Path, size: u64, count: usize) {
    let _ = std::fs::write(sidecar, format!("{},{}", size, count));
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
fn clean_versions(versions_folder: &Path, max_versions: usize) {
    if let Ok(entries) = std::fs::read_dir(versions_folder) {
        let mut snapshots: Vec<_> = entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().len() == 15)
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

    let leaf = Path::new(source).file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // Backup paths
    let backup_7z = config::backup_file_for(&leaf);
    let sidecar = config::sidecar_for(&leaf);
    let backup_dir = config::backup_dir_for(&leaf);
    let _ = std::fs::create_dir_all(&backup_dir);
    let versions_folder = config::trash_path_for(&leaf);

    // Migration: old root-level backup → delete (will re-compress to backup/ on next sync)
    let old_root_backup = config::script_dir().join(format!("{}.tar.zst", leaf));
    let old_root_sidecar = config::script_dir().join(format!("{}.tar.zst.size", leaf));
    if old_root_backup.exists() {
        let _ = std::fs::remove_file(&old_root_backup);
        let _ = std::fs::remove_file(&old_root_sidecar);
    }

    // Migration: if old raw backup folder exists, compress it
    let old_backup_folder = config::script_dir().join(&leaf);
    if old_backup_folder.is_dir() && !backup_7z.exists() {
        let _ = compress_folder(&old_backup_folder, &backup_7z, excluded, None); // (bool, Vec) — ignore result for migration
        let _ = std::fs::remove_dir_all(&old_backup_folder);
    }

    // Change detection: compare current source stats with stored stats
    let walked = walk_tree(Path::new(source), excluded);
    let current_size = walked.1;
    let current_count = walked.2;
    let (stored_size, stored_count) = read_stored_stats(&sidecar);

    if force || current_size != stored_size || current_count != stored_count || !backup_7z.exists() {
        let mut snapshotted = false;
        // Something changed — create snapshot of old backup, then re-compress
        if backup_7z.exists() {
            create_snapshot(&backup_7z, &versions_folder);
            snapshotted = true;
        }
        clean_versions(&versions_folder, max_versions as usize);

        // Compress source to temp, then move (atomic-ish)
        // PID-unique temp name prevents corruption if two processes ever collide
        let temp_7z = std::env::temp_dir().join(format!("lrgex_{}_{}.tar.zst.tmp", std::process::id(), leaf));
        let (compress_ok, skipped) = compress_folder(Path::new(source), &temp_7z, excluded, Some(walked));
        if compress_ok {
            let _ = std::fs::remove_file(&backup_7z);
            if std::fs::rename(&temp_7z, &backup_7z).is_ok() {
                write_stored_stats(&sidecar, current_size, current_count);
            } else {
                // Rename failed (OneDrive lock?) — try copy + delete
                if std::fs::copy(&temp_7z, &backup_7z).is_ok() {
                    let _ = std::fs::remove_file(&temp_7z);
                    write_stored_stats(&sidecar, current_size, current_count);
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
        let backup = config::backup_file_for(&leaf);

        // 1. Backup archive exists?
        if !backup.exists() {
            failures.push((leaf, "backup missing".into()));
            continue;
        }

        // 2. Archive is valid zstd? (check magic bytes: 28 b5 2f fd)
        let mut header = [0u8; 4];
        let valid_zstd = std::fs::File::open(&backup)
            .and_then(|mut f| f.read_exact(&mut header).map(|_| f))
            .map(|_| header == [0x28, 0xb5, 0x2f, 0xfd])
            .unwrap_or(false);
        if !valid_zstd {
            failures.push((leaf, "backup archive is corrupt or incomplete".into()));
            continue;
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
    let leaf = Path::new(source).file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let backup_7z = config::backup_file_for(&leaf);

    // Try .tar.zst first (new format)
    if backup_7z.exists() {
        let (ok, msg) = decompress_archive(&backup_7z, Path::new(source));
        if ok {
            return (true, String::new());
        }
        return (false, if msg.is_empty() { "decompression failed".into() } else { msg });
    }

    // Fallback: old raw folder format
    let old_backup = config::script_dir().join(&leaf);
    if old_backup.exists() {
        let _ = std::fs::create_dir_all(source);
        let args: Vec<String> = vec![
            old_backup.to_string_lossy().to_string(), source.into(),
            "/E".into(), "/XJ".into(), "/NFL".into(), "/NDL".into(),
            "/NJH".into(), "/NJS".into(), "/NP".into(), "/R:5".into(), "/W:5".into(),
        ];
        let output = Command::new("robocopy.exe").args(&args)
            .creation_flags(0x08000000).output();
        return match output {
            Ok(out) => {
                let code = out.status.code().unwrap_or(16);
                if code < 8 { (true, String::new()) }
                else { (false, format!("robocopy exit {}", code)) }
            }
            Err(e) => (false, e.to_string()),
        };
    }

    (false, "backup missing".into())
}

pub fn sync_all_pairs() {
    let cfg = config::load_config();
    let mut ok = 0i32;
    let mut fail = 0i32;
    let mut restored = 0i32;
    let mut restored_names: Vec<String> = vec![];

    crate::synclog::write("------------------------------------------------------------");
    crate::synclog::write("Sync cycle");
    crate::synclog::write_progress("");

    for j in &cfg.junctions {
        let leaf = Path::new(&j.source_path).file_name()
            .map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let missing = !Path::new(&j.source_path).exists() || is_dir_empty(&j.source_path);

        if missing && j.auto_restore {
            let (success, _) = restore_pair_from_cloud(&j.source_path);
            if success {
                restored += 1;
                restored_names.push(leaf.clone());
                crate::synclog::write(&format!("  [RESTORE] {}  -  was missing, restored", leaf));
                // Set migration marker so future syncs check for new game-created ID folders
                set_migration_pending();
                // Also try migration now (game might have already created new ID)
                let mig = migrate_save_ids(Path::new(&j.source_path));
                for m in &mig { crate::synclog::write(&format!("  [MIGRATE] {}", m)); }
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
                fail += 1;
                crate::synclog::write(&format!("  [FAIL] {}  -  {}", leaf, reason));
            }
        }
    }

    crate::synclog::write(&format!("Done: {} compressed, {} restored, {} failed.", ok, restored, fail));
    crate::health::write_status(ok + restored, fail, restored, &restored_names);
}

// ==================== RESTORE FROM SNAPSHOT ====================

/// Restore a specific snapshot version to the source location
pub fn restore_snapshot(snapshot_dir: &Path, source: &str) -> (bool, String) {
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
    let args: Vec<String> = vec![
        snapshot_dir.to_string_lossy().to_string(), source.into(),
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

/// List files inside a .tar.zst archive (without extracting)

// ==================== SCHEDULED TASK ====================

/// Register (or update) the Windows Scheduled Task.
/// Uses CANONICAL HOME from registry — never current_exe().
/// This prevents stray copies from retargeting the task.

/// Delete old VBS-based scheduled tasks from the PowerShell version.

/// Delete old VBS-based scheduled tasks from the PowerShell version.
/// Prevents "cannot find sync-runner.vbs" errors for users upgrading from old version.
/// Scans every launch on background thread. Catches old VBS tasks from any previous version.
pub fn register_sync_task(interval_minutes: i32) -> bool {
    let home = match config::canonical_home() {
        Some(h) => h,
        None => return false,
    };
    let exe = home.join("LRGEXRestore.exe");
    let task_cmd = format!("\"{}\" -sync", exe.to_string_lossy());

    // schtasks /SC MINUTE max is 1439, /SC HOURLY max is 23.
    // Pick the right schedule type based on interval size.
    let (schedule, modifier) = if interval_minutes >= 1440 {
        // 24+ hours: MINUTE max is 1439, use DAILY instead
        let days = (interval_minutes / 1440).max(1);
        ("DAILY", days.to_string())
    } else {
        // Under 24 hours: MINUTE with exact precision
        ("MINUTE", interval_minutes.to_string())
    };

    match Command::new("schtasks.exe")
        .args([
            "/Create",
            "/TN", "LRGEX-Restore-Rust",
            "/TR", &task_cmd,
            "/SC", schedule,
            "/MO", &modifier,
            "/F",
        ])
        .creation_flags(0x08000000u32)
        .output()
    {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
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
