//! Release helpers. Run with `cargo xtask <command>`.
//!
//!   keygen [--out FILE]                     make a release signing key
//!   manifest --version V --base-url URL DIR write DIR/atlas-update.json
//!   sign FILE                               write FILE.sig (key from ATLAS_UPDATE_KEY or --key FILE)
//!   icons                                   regenerate packaging/icons from the Atlas mark

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
        Some("icons") => {
            let dir = Path::new("packaging/icons");
            std::fs::create_dir_all(dir).unwrap_or_else(|e| die(&e.to_string()));
            for size in [32u32, 128, 256, 512] {
                let img = render_icon(size);
                img.save(dir.join(format!("{size}x{size}.png"))).unwrap_or_else(|e| die(&e.to_string()));
            }
            render_icon(256).save(dir.join("icon.ico")).unwrap_or_else(|e| die(&e.to_string()));
            println!("wrote packaging/icons");
        }
        _ => {
            eprintln!("usage: cargo xtask keygen | manifest --version V --base-url URL DIR | sign FILE");
            std::process::exit(2);
        }
    }
}

/// Same globe mark as the app, drawn with 4x supersampling for smooth edges.
fn render_icon(size: u32) -> image::RgbaImage {
    let ss = 4u32;
    let n = size * ss;
    let c = n as f32 / 2.0;
    let r = c * 0.74;
    let w = n as f32 * 0.045;
    let accent = [0x3f, 0xd0, 0xc0];
    let mut big = vec![[0u8; 4]; (n * n) as usize];
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            let d = (dx * dx + dy * dy).sqrt();
            let mut px = [0u8; 4];
            // rounded dark tile
            let (ax, ay) = (dx.abs() - c * 0.78, dy.abs() - c * 0.78);
            let corner = (ax.max(0.0).powi(2) + ay.max(0.0).powi(2)).sqrt();
            if corner < c * 0.2 {
                px = [0x10, 0x14, 0x1b, 255];
            }
            let (rx, ry) = (dx * 0.91 + dy * 0.41, -dx * 0.41 + dy * 0.91);
            let ring = (d - r).abs() < w;
            let mer = (((rx / (r * 0.42)).powi(2) + (ry / r).powi(2)).sqrt() - 1.0).abs() * r < w * 0.8;
            let lat = (((rx / r).powi(2) + (ry / (r * 0.34)).powi(2)).sqrt() - 1.0).abs() * r * 0.6 < w * 0.8;
            let moon = ((dx - r * 0.66).powi(2) + (dy + r * 0.66).powi(2)).sqrt() < r * 0.15;
            if ring || moon || ((mer || lat) && d < r) {
                px = [accent[0], accent[1], accent[2], 255];
            }
            big[(y * n + x) as usize] = px;
        }
    }
    image::RgbaImage::from_fn(size, size, |x, y| {
        let mut acc = [0u32; 4];
        for sy in 0..ss {
            for sx in 0..ss {
                let p = big[((y * ss + sy) * n + x * ss + sx) as usize];
                for i in 0..4 {
                    acc[i] += p[i] as u32;
                }
            }
        }
        let k = ss * ss;
        image::Rgba([(acc[0] / k) as u8, (acc[1] / k) as u8, (acc[2] / k) as u8, (acc[3] / k) as u8])
    })
}
