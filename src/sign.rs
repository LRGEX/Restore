//! Release signer - signs the exe with the private key during deploy.
//! SAFETY: verifies private key matches the public key anchor (signing.pub) BEFORE signing.

use ed25519_dalek::{Signer, SigningKey};
use std::io::Read;

const PUBKEY_ANCHOR: &str = include_str!("../signing.pub");

fn main() {
    // L8: key path from env with dev-machine default.
    // H2: `--text` mode signs arbitrary text (manifest signing).
    let args: Vec<String> = std::env::args().collect();
    let signing_key = load_key_checked();

    if args.len() >= 3 && args[1] == "--text" {
        println!("{}", hex::encode(signing_key.sign(args[2].as_bytes()).to_bytes()));
        return;
    }
    let exe_path = args.get(1).expect("Usage: sign <exe_path> | sign --text <text>").clone();

    let mut file = std::fs::File::open(&exe_path).expect("Cannot open exe");
    let mut data = Vec::new();
    file.read_to_end(&mut data).expect("Cannot read exe");

    let signature = signing_key.sign(&data);
    println!("{}", hex::encode(signature.to_bytes()));
}

fn key_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("LRGEX_SIGNING_KEY") {
        return p.into();
    }
    let root = format!("E:{}", std::path::MAIN_SEPARATOR);
    std::path::PathBuf::from(root)
        .join("LRG").join("LRG Data Cloud").join("L.R.G")
        .join("Devoloping").join("Coding").join("Security keys")
        .join("RUST").join("LRGEX-Restore").join("keys").join("signing.key")
}

fn load_key_checked() -> SigningKey {
    let key_path = key_path();
    let priv_hex = std::fs::read_to_string(&key_path)
        .unwrap_or_else(|e| panic!("Cannot read signing key at {}: {}", key_path.display(), e));
    let priv_bytes = hex::decode(priv_hex.trim()).expect("Invalid key format");
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&priv_bytes);
    let signing_key = SigningKey::from_bytes(&secret);

    let derived_pubkey = hex::encode(signing_key.verifying_key().to_bytes());
    let expected_pubkey = PUBKEY_ANCHOR.trim();
    if derived_pubkey != expected_pubkey {
        eprintln!("FATAL: Key mismatch!");
        eprintln!("  Private key derives to: {}", derived_pubkey);
        eprintln!("  Anchor (signing.pub):   {}", expected_pubkey);
        eprintln!("  Updates will be REJECTED by all clients.");
        eprintln!("  Fix: restore the correct private key.");
        std::process::exit(1);
    }
    signing_key
}
