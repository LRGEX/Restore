use serde::Deserialize;
use std::io::Read;
use ed25519_dalek::{VerifyingKey, Verifier, Signature};

const MANIFEST_URL: &str = "https://download.lrgex.com/app/rst/lrgex-restore/latest.json";

// Public key from the immutable anchor file (signing.pub).
// This is the ONE source of truth — derived from the private key, committed to git.
const UPDATE_PUBKEY_HEX: &str = include_str!("../signing.pub");

#[derive(Deserialize)]
struct Manifest {
    version: String,
    /// H2: Ed25519 signature over "version|url|exe_sha256" — verified BEFORE
    /// version/url are trusted. Kills rollback attacks (old exe re-served as
    /// "v99") because the attacker can't forge this signature.
    manifest_signature: Option<String>,
    platforms: Platforms,
}

#[derive(Deserialize)]
struct Platforms {
    #[serde(rename = "windows-x86_64")]
    windows: Platform,
}

#[derive(Deserialize)]
struct Platform {
    url: String,
    /// H2: SHA-256 of the exe, signed inside manifest_signature. Also lets the
    /// client verify the download independently of the Ed25519 exe signature.
    exe_sha256: Option<String>,
    signature: Option<String>,
}

pub fn check_for_updates() {
    let current = env!("CARGO_PKG_VERSION");
    let exe_path = std::env::current_exe().unwrap_or_default();

    let response = match ureq::get(MANIFEST_URL).timeout(std::time::Duration::from_secs(10)).call() {
        Ok(r) => r,
        Err(_) => return,
    };

    let manifest: Manifest = match response.into_json() {
        Ok(m) => m,
        Err(_) => return,
    };

    // Version-equality fast path (advisor): an up-to-date client deciding "no
    // update" doesn't need to trust the manifest's version claim.
    if manifest.version == current { return; }

    // H-3: FAIL-CLOSED manifest trust (gate BEFORE is_newer). This client ships
    // AFTER the signed-manifest deploy exists (deploy.ps1 signs since 7c4be77) —
    // every genuine manifest it can fetch carries manifest_signature + exe_sha256.
    // Unsigned = forged (attacker re-serving a genuine old exe as "v9.9.9" with
    // its public signature). Old clients never read these fields, so fail-open
    // protected an empty set and armed the downgrade. Refuse quietly + log.
    let (msig, exe_sha) = match (&manifest.manifest_signature, &manifest.platforms.windows.exe_sha256) {
        (Some(m), Some(s)) if !m.is_empty() && !s.is_empty() => (m.clone(), s.clone()),
        _ => {
            crate::synclog::write("[UPDATE] rejected unsigned manifest (no manifest_signature)");
            return;
        }
    };
    {
        let canonical = format!("{}|{}|{}", manifest.version, manifest.platforms.windows.url, exe_sha);
        if verify_signature(canonical.as_bytes(), &msig).is_err() {
            crate::synclog::write("[UPDATE] rejected manifest: signature verification failed");
            return;
        }
    }

    if !is_newer(&manifest.version, current) {
        return;
    }

    let confirm = rfd::MessageDialog::new()
        .set_title("Update Available")
        .set_description(&format!(
            "Version {} is available (you have v{}).\n\nUpdate now?",
            manifest.version, current
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();

    if confirm != rfd::MessageDialogResult::Yes {
        return;
    }

    // H1: unpredictable temp name — a predictable name lets same-user malware
    // pre-place/swap a payload at a known path between verify and copy.
    let nonce = rand::random::<u64>();
    let temp_exe = std::env::temp_dir().join(format!("lrgex_upd_{:016x}.exe", nonce));
    let bat_path = std::env::temp_dir().join(format!("lrgex_upd_{:016x}.bat", nonce));

    let resp = match ureq::get(&format!("{}?v={}", manifest.platforms.windows.url, manifest.version))
        .timeout(std::time::Duration::from_secs(120))
        .call() {
        Ok(r) => r,
        Err(e) => { show_error(&format!("Download failed: {}", e)); return; }
    };

    let mut reader = resp.into_reader();
    let mut data = Vec::new();
    // L-5: bounded read — a compromised/buggy CDN serving a multi-GB body must
    // not OOM the app. Cap at 64 MB (the exe is ~17 MB; huge margin).
    let mut remaining = 64 * 1024 * 1024usize;
    let mut chunk = [0u8; 65536];
    loop {
        let n = match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => { show_error(&format!("Read failed: {}", e)); return; }
        };
        if n > remaining {
            show_error("Downloaded file exceeds 64 MB — refusing. Possible CDN problem.");
            return;
        }
        data.extend_from_slice(&chunk[..n]);
        remaining -= n;
    }

    if data.len() < 1_000_000 {
        show_error(&format!("Downloaded file too small: {} bytes.", data.len()));
        return;
    }

    // H2: verify download against the SIGNED sha256 from the manifest (when
    // present) — the sha256 is covered by manifest_signature, so this check
    // is anchored to the key, not to the network.
    // H2/H-3: exe_sha256 is guaranteed present (fail-closed gate above) — the
    // download MUST match the SIGNED hash from the manifest.
    {
        let expected = &exe_sha;
        let actual = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(&data);
            hex::encode(h.finalize())
        };
        if !actual.eq_ignore_ascii_case(expected) {
            show_error("Download failed hash check — possible corruption. Update aborted.");
            return;
        }
    }

    // Verify Ed25519 signature against the public key from signing.pub
    // (clone the sig so it stays in scope for the on-disk re-verify below)
    let sig_hex = match manifest.platforms.windows.signature.clone() {
        Some(s) if !s.is_empty() => s,
        Some(_) => { show_error("Signature is EMPTY. Update aborted."); return; }
        None => { show_error("No signature in manifest. Update aborted."); return; }
    };
    {
        match verify_signature(&data, &sig_hex) {
            Ok(()) => {}
            Err(e) => {
                show_error(&format!(
                    "Signature verification FAILED.\n\n{}\n\nThe download may be corrupted or tampered with. Update aborted for your safety.",
                    e
                ));
                return;
            }
        }
    }

    // H1: write, then RE-READ from disk and RE-VERIFY — closes the write→copy
    // swap window (the check above validated memory, not the file on disk).
    if let Err(e) = std::fs::write(&temp_exe, &data) {
        show_error(&format!("Save failed: {}", e));
        let _ = std::fs::remove_file(&temp_exe); // I-4: uniform cleanup
        return;
    }
    let on_disk = match std::fs::read(&temp_exe) {
        Ok(d) => d,
        Err(e) => { show_error(&format!("Re-read failed: {}", e)); let _ = std::fs::remove_file(&temp_exe); return; }
    };
    if let Err(e) = verify_signature(&on_disk, &sig_hex) {
        show_error(&format!("On-disk verification failed — possible tampering. Aborted. ({})", e));
        let _ = std::fs::remove_file(&temp_exe);
        return;
    }
    // SHA-256 of the verified bytes — the bat re-checks this before copying,
    // closing the dialog→copy window too (certutil is in-box on Windows).
    let expected_sha256 = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(&on_disk);
        hex::encode(h.finalize())
    };
    let t = temp_exe.to_string_lossy().to_string();
    let e = exe_path.to_string_lossy().to_string();
    let h = &expected_sha256;
    // H1: the bat verifies the SHA-256 (certutil, in-box) right before copying —
    // even if the temp file is swapped during the dialog window, the copy aborts.
    // Built with join to guarantee CRLF line endings (batch files misparse with bare \n).
    let bat_lines: Vec<String> = vec![
        "@echo off".into(),
        "ping 127.0.0.1 -n 3 > nul".into(),
        ":verify".into(),
        format!("certutil -hashfile \"{}\" SHA256 | findstr /i \"{}\" >nul 2>&1", t, h),
        "if errorlevel 1 (".into(),
        format!("  del \"{}\" >nul 2>&1", t),
        "  exit /b 1".into(),
        ")".into(),
        ":retry".into(),
        "set /a tries=0".into(),
        ":retryloop".into(),
        format!("copy /Y \"{}\" \"{}\" >nul 2>&1", t, e),
        "if not errorlevel 1 goto done".into(),
        "set /a tries+=1".into(),
        "if %tries% geq 30 (\r\n  del \"%~f0\" >nul 2>&1\r\n  exit /b 1\r\n)".into(), // L-6a: cap ~90s, no zombie cmd
        "ping 127.0.0.1 -n 3 >nul".into(),
        "goto retryloop".into(),
        ":done".into(),
        format!("del \"{}\" >nul 2>&1", t),
        format!("start \"\" \"{}\"", e),
        "del \"%~f0\"".into(),
    ];
    let bat = bat_lines.join("\r\n") + "\r\n";

    // L-6b: bat write failure must abort — spawning cmd on a nonexistent file
    // silently produced a zombie no-update.
    if std::fs::write(&bat_path, bat).is_err() {
        show_error("Could not write updater script. Update aborted.");
        let _ = std::fs::remove_file(&temp_exe);
        return;
    }

    rfd::MessageDialog::new()
        .set_title("Updating")
        .set_description("Signature verified. The app will restart in a moment with the new version.")
        .set_buttons(rfd::MessageButtons::Ok)
        .show();

    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("cmd.exe")
        .args(["/c", bat_path.to_str().unwrap_or("")])
        .creation_flags(0x08000000u32)
        .spawn();

    std::process::exit(0);
}

fn verify_signature(data: &[u8], sig_hex: &str) -> Result<(), String> {
    let pub_hex = UPDATE_PUBKEY_HEX.trim();
    let pub_bytes = hex::decode(pub_hex).map_err(|e| format!("Bad public key: {}", e))?;
    let mut pub_arr = [0u8; 32];
    pub_arr.copy_from_slice(&pub_bytes);
    let verifying_key = VerifyingKey::from_bytes(&pub_arr).map_err(|e| format!("Bad public key: {}", e))?;

    let sig_bytes = hex::decode(sig_hex).map_err(|e| format!("Bad signature format: {}", e))?;
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let signature = Signature::from_bytes(&sig_arr);

    verifying_key.verify(data, &signature).map_err(|e| format!("Invalid signature: {}", e))
}

fn show_error(msg: &str) {
    rfd::MessageDialog::new()
        .set_title("Update Failed")
        .set_description(msg)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

fn is_newer(remote: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.split('.').filter_map(|n| n.parse().ok()).collect()
    };
    let r = parse(remote);
    let c = parse(current);
    for i in 0..r.len().max(c.len()) {
        let rv = r.get(i).copied().unwrap_or(0);
        let cv = c.get(i).copied().unwrap_or(0);
        if rv > cv { return true; }
        if rv < cv { return false; }
    }
    false
}
