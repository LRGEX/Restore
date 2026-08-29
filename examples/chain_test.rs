// Standalone replica of update.rs post-"Yes" flow — per-step timing.
use std::io::Read;

const MANIFEST_URL: &str = "https://download.lrgex.com/app/rst/lrgex-restore/latest.json";
const UPDATE_PUBKEY_HEX: &str = include_str!("../signing.pub");

fn main() {
    let t0 = std::time::Instant::now();
    let step = |name: &str, t: &mut std::time::Instant| {
        println!("  {:<28} {:?}", name, t.elapsed());
        *t = std::time::Instant::now();
    };

    let mut t = t0.clone();
    // 1. Fetch manifest
    let resp = ureq::get(MANIFEST_URL).timeout(std::time::Duration::from_secs(10)).call().expect("manifest fetch");
    let manifest: serde_json::Value = resp.into_json().expect("manifest json");
    step("manifest fetch", &mut t);

    let version = manifest["version"].as_str().unwrap().to_string();
    let url = manifest["platforms"]["windows-x86_64"]["url"].as_str().unwrap().to_string();
    let exe_sha = manifest["platforms"]["windows-x86_64"]["exe_sha256"].as_str().unwrap().to_string();
    let msig = manifest["manifest_signature"].as_str().unwrap().to_string();
    let sig = manifest["platforms"]["windows-x86_64"]["signature"].as_str().unwrap().to_string();
    println!("  version={} sig_len={} msig_len={}", version, sig.len(), msig.len());

    // 2. Manifest signature verify
    let canonical = format!("{}|{}|{}", version, url, exe_sha);
    verify(&canonical.as_bytes(), &msig).expect("MANIFEST SIG FAILED");
    step("manifest sig verify", &mut t);

    // 3. Download exe
    let resp = ureq::get(&format!("{}?v={}", url, version)).timeout(std::time::Duration::from_secs(120)).call().expect("exe fetch");
    let mut reader = resp.into_reader();
    let mut data = Vec::new();
    let mut chunk = [0u8; 65536];
    let mut remaining = 64 * 1024 * 1024usize;
    loop {
        let n = match reader.read(&mut chunk) { Ok(0) => break, Ok(n) => n, Err(e) => panic!("read: {}", e) };
        if n > remaining { panic!("too big"); }
        data.extend_from_slice(&chunk[..n]);
        remaining -= n;
    }
    step(&format!("download {} MB", data.len() / 1048576), &mut t);

    // 4. SHA-256
    let actual = sha2hex(&data);
    assert!(actual.eq_ignore_ascii_case(&exe_sha), "HASH MISMATCH: {} vs {}", actual, exe_sha);
    step("sha256", &mut t);

    // 5. Ed25519 exe sig
    verify(&data, &sig).expect("EXE SIG FAILED");
    step("ed25519 verify", &mut t);

    // 6. Write + re-read + re-verify
    let tmp = std::env::temp_dir().join(format!("lrgex_chain_test_{}.exe", std::process::id()));
    std::fs::write(&tmp, &data).expect("write");
    let on_disk = std::fs::read(&tmp).expect("reread");
    verify(&on_disk, &sig).expect("RE-VERIFY FAILED");
    let _ = std::fs::remove_file(&tmp);
    step("write+reread+reverify", &mut t);

    println!("  TOTAL: {:?}  — ALL GREEN", t0.elapsed());
}

fn sha2hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

fn verify(data: &[u8], sig_hex: &str) -> Result<(), String> {
    use ed25519_dalek::{VerifyingKey, Verifier, Signature};
    let pub_bytes = hex::decode(UPDATE_PUBKEY_HEX.trim()).map_err(|e| e.to_string())?;
    let mut pub_arr = [0u8; 32];
    pub_arr.copy_from_slice(&pub_bytes);
    let vk = VerifyingKey::from_bytes(&pub_arr).map_err(|e| e.to_string())?;
    let sig_bytes = hex::decode(sig_hex).map_err(|e| e.to_string())?;
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let signature = Signature::from_bytes(&sig_arr);
    vk.verify(data, &signature).map_err(|e| e.to_string())
}
