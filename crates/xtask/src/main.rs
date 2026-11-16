//! Release helpers. Run with `cargo xtask <command>`.
//!
//!   keygen [--out FILE]                     make a release signing key
//!   manifest --version V --base-url URL DIR write DIR/atlas-update.json
//!   sign FILE                               write FILE.sig (key from ATLAS_UPDATE_KEY or --key FILE)

use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use sha2::Digest as _;
use std::path::{Path, PathBuf};

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}

fn load_key(args: &[String]) -> SigningKey {
    let raw = match arg(args, "--key") {
        Some(f) => std::fs::read_to_string(f).unwrap_or_else(|e| die(&format!("can't read key: {e}"))),
        None => std::env::var("ATLAS_UPDATE_KEY").unwrap_or_else(|_| die("set ATLAS_UPDATE_KEY or pass --key FILE")),
    };
    let seed: [u8; 32] = b64().decode(raw.trim()).ok().and_then(|b| b.try_into().ok()).unwrap_or_else(|| die("key must be 32 bytes, base64"));
    SigningKey::from_bytes(&seed)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") => {
            let out = PathBuf::from(arg(&args, "--out").unwrap_or_else(|| "atlas-update-key.txt".into()));
            if out.exists() {
                die(&format!("{} already exists, not overwriting a key", out.display()));
            }
            let seed: [u8; 32] = rand::random();
            let sk = SigningKey::from_bytes(&seed);
            std::fs::write(&out, b64().encode(seed)).unwrap_or_else(|e| die(&e.to_string()));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o600));
            }
            println!("Private key written to {} - keep it secret, keep it offline.", out.display());
            println!("Add it as the GitHub Actions secret ATLAS_UPDATE_KEY.");
            println!();
            println!("Public key (set as the repo variable ATLAS_UPDATE_PUBKEY):");
            println!("{}", b64().encode(sk.verifying_key().to_bytes()));
        }
        Some("manifest") => {
            let version = arg(&args, "--version").unwrap_or_else(|| die("--version is required"));
            let base = arg(&args, "--base-url").unwrap_or_else(|| die("--base-url is required"));
            let dir = PathBuf::from(args.last().cloned().unwrap_or_else(|| die("missing DIR")));
            let mut assets = serde_json::Map::new();
            for e in std::fs::read_dir(&dir).unwrap_or_else(|e| die(&e.to_string())).flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                // raw binaries are named atlas-launcher-<arch>-<os>[.exe]
                let Some(key) = name.strip_prefix("atlas-launcher-").map(|k| k.trim_end_matches(".exe").to_string()) else { continue };
                if key.contains('.') {
                    continue; // archives, checksums
                }
                let bytes = std::fs::read(e.path()).unwrap_or_else(|e| die(&e.to_string()));
                assets.insert(key, serde_json::json!({"url": format!("{}/{name}", base.trim_end_matches('/')), "sha256": hex::encode(sha2::Sha256::digest(&bytes)), "size": bytes.len()}));
            }
            if assets.is_empty() {
                die("no atlas-launcher-<platform> binaries found");
            }
            let m = serde_json::json!({"version": version.trim_start_matches('v'), "notes": arg(&args, "--notes").unwrap_or_default(), "assets": assets});
            let out = dir.join("atlas-update.json");
            std::fs::write(&out, serde_json::to_vec_pretty(&m).unwrap_or_default()).unwrap_or_else(|e| die(&e.to_string()));
            println!("wrote {}", out.display());
        }
        Some("sign") => {
            let file = args.get(1).filter(|f| !f.starts_with("--")).unwrap_or_else(|| die("usage: sign FILE"));
            let sk = load_key(&args);
            let body = std::fs::read(file).unwrap_or_else(|e| die(&e.to_string()));
            let sig = b64().encode(sk.sign(&body).to_bytes());
            let out = format!("{file}.sig");
            std::fs::write(Path::new(&out), sig).unwrap_or_else(|e| die(&e.to_string()));
            println!("wrote {out}");
        }
        _ => {
            eprintln!("usage: cargo xtask keygen | manifest --version V --base-url URL DIR | sign FILE");
            std::process::exit(2);
        }
    }
}
