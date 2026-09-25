// Repro v2: the app's EXACT hash path — rayon par_iter + Progress-style
// heartbeat writer thread. Bisects: rayon? heartbeat? both?
use rayon::prelude::*;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 64 { return; }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            let ft = match e.file_type() { Ok(t) => t, Err(_) => continue };
            if ft.is_symlink() { continue; }
            if ft.is_dir() { walk(&p, out, depth + 1); }
            else { out.push(p); }
        }
    }
}

fn hash_file(path: &Path) -> Option<u64> {
    let f = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::with_capacity(1 << 20, f);
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut buf = [0u8; 65536];
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

struct Ent { path: PathBuf, size: u64 }

fn main() {
    let root = std::env::args().nth(1).expect("usage: <folder>");
    let mode = std::env::args().nth(2).unwrap_or_else(|| "both".into()); // rayon | hb | both
    let mut files = Vec::new();
    walk(Path::new(&root), &mut files, 0);
    let ents: Vec<Ent> = files.into_iter().map(|p| {
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        Ent { path: p, size }
    }).collect();
    println!("walked {} files, mode={}", ents.len(), mode);

    // Heartbeat-style writer thread (every 500ms)
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bytes = Arc::new(AtomicU64::new(0));
    let hb = if mode == "hb" || mode == "both" {
        let stop = stop.clone();
        let bytes = bytes.clone();
        Some(std::thread::spawn(move || {
            let mut i = 0;
            while !stop.load(Ordering::Relaxed) {
                let _ = bytes.load(Ordering::Relaxed);
                std::fs::write(std::env::temp_dir().join("repro_hb.txt"), format!("{}", i)).ok();
                i += 1;
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }))
    } else { None };

    let bytes2 = bytes.clone();
    let result: u64 = if mode == "hb" {
        // sequential (already proven fine) — control
        ents.iter().map(|e| hash_file(&e.path).unwrap_or(0)).sum()
    } else {
        // rayon — exactly like compute_manifest
        ents.par_iter().map(|e| {
            let h = hash_file(&e.path).unwrap_or(0);
            bytes2.fetch_add(e.size.max(1), Ordering::Relaxed);
            h
        }).sum()
    };

    stop.store(true, Ordering::Relaxed);
    if let Some(h) = hb { let _ = h.join(); }
    println!("OK — hash sum: {}", result);
}
