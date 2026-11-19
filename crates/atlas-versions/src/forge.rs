//! Forge and NeoForge.
//!
//! Profiles come from Modrinth's launcher-meta, which has already pulled the
//! installer apart for us. What's left is the installer's "processors":
//! small Java tools that split, remap and patch the vanilla jar. We run the
//! client ones ourselves and check their outputs.
//!
//! One file in those profiles (the binary patch set) has no published hash,
//! so we don't take it from there. We grab the official installer from the
//! Forge/NeoForge maven instead (that one has a .sha1) and pull the patch
//! out of it.

use crate::loaders::{maven_path, LoaderKind, LoaderVersion};
use crate::record::RecordedFile;
use crate::service::VersionService;
use atlas_downloads::{verify_file, CancellationToken, Checksum, DownloadRequest, Downloader, Progress};
use atlas_shared::fs_safety::safe_join;
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn slug(l: LoaderKind) -> &'static str {
    if l == LoaderKind::NeoForge { "neo" } else { "forge" }
}

fn err(msg: impl Into<String>, d: impl std::fmt::Display) -> AtlasError {
    AtlasError::new(ErrorCode::PreparationFailed, msg).details(d.to_string()).suggest("Try another loader version, or check the logs for the installer output.")
}

pub(crate) async fn versions(svc: &VersionService, dl: &Downloader, loader: LoaderKind, mc: &str, cancel: &CancellationToken) -> AtlasResult<Vec<LoaderVersion>> {
    let b = dl.fetch_bytes(&format!("{}/{}/v0/manifest.json", svc.loader_sources.modrinth_meta, slug(loader)), cancel).await?;
    let v: Value = serde_json::from_slice(&b).map_err(|e| err("Couldn't read the loader list.", e))?;
    let game = v.get("gameVersions").and_then(|g| g.as_array()).and_then(|a| a.iter().find(|g| g.get("id").and_then(|i| i.as_str()) == Some(mc)));
    Ok(game
        .and_then(|g| g.get("loaders"))
        .and_then(|l| l.as_array())
        .map(|a| a.iter().filter_map(|l| Some(LoaderVersion { version: l.get("id")?.as_str()?.to_string(), stable: l.get("stable").and_then(|s| s.as_bool()).unwrap_or(false) })).collect())
        .unwrap_or_default())
}

/// The official installer for a loader version, plus where its .sha1 lives.
fn installer_url(loader: LoaderKind, mc: &str, lv: &str) -> String {
    match loader {
        LoaderKind::NeoForge => format!("https://maven.neoforged.net/releases/net/neoforged/neoforge/{lv}/neoforge-{lv}-installer.jar"),
        // old Forge versions carry the MC version in the artifact version
        _ => {
            let full = if lv.starts_with(mc) { lv.to_string() } else { format!("{mc}-{lv}") };
            format!("https://maven.minecraftforge.net/net/minecraftforge/forge/{full}/forge-{full}-installer.jar")
        }
    }
}

fn main_class_of(jar: &Path) -> AtlasResult<String> {
    let f = std::fs::File::open(jar).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "open an installer tool", jar, e))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| err("An installer tool is damaged.", e))?;
    let mut m = String::new();
    z.by_name("META-INF/MANIFEST.MF").map_err(|e| err("An installer tool has no manifest.", e))?.read_to_string(&mut m).map_err(|e| err("An installer tool has no manifest.", e))?;
    m.lines()
        .find_map(|l| l.strip_prefix("Main-Class:").map(|c| c.trim().to_string()))
        .ok_or_else(|| err("An installer tool doesn't say how to run it.", jar.display()))
}

pub(crate) async fn install(svc: &VersionService, dl: &Downloader, loader: LoaderKind, mc: &str, lv: &str, progress: Arc<Progress>, cancel: CancellationToken) -> AtlasResult<String> {
    let url = format!("{}/{}/v0/versions/{lv}.json", svc.loader_sources.modrinth_meta, slug(loader));
    let raw = dl.fetch_bytes(&url, &cancel).await?;
    let mut profile: Value = serde_json::from_slice(&raw).map_err(|e| err("The loader profile couldn't be read.", e))?;
    let id = profile.get("id").and_then(|v| v.as_str()).map(str::to_owned).ok_or_else(|| err("The loader profile has no id.", &url))?;
    if !crate::valid_id(&id) || profile.get("inheritsFrom").and_then(|v| v.as_str()) != Some(mc) {
        return Err(err("The loader profile is for a different Minecraft version.", &id));
    }
    if svc.record(&id).is_ok() {
        return Ok(id);
    }
    let libs_dir = svc.paths.libraries_dir();
    let tmp = svc.paths.staging_dir().join(format!("{id}-installer"));
    std::fs::create_dir_all(&tmp).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "create a temp folder", &tmp, e))?;

    // 1. official installer (hash from its own maven), for the patch files
    let inst_url = installer_url(loader, mc, lv);
    let sha = dl.fetch_bytes(&format!("{inst_url}.sha1"), &cancel).await?;
    let sha = String::from_utf8_lossy(&sha).split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    if !crate::valid_sha1(&sha) {
        return Err(err("The official installer has no usable checksum.", &inst_url));
    }
    let installer = tmp.join("installer.jar");
    dl.download(&DownloadRequest { url: inst_url.clone(), dest: installer.clone(), checksum: Checksum::Sha1(sha), size: None, reuse_from: vec![] }, &progress, &cancel).await?;

    // 2. libraries: normal ones are hashed; the "installer-extracts" ones come out of the installer
    let mut reqs = vec![];
    let mut files: Vec<RecordedFile> = vec![];
    let mut extracts: Vec<(String, PathBuf)> = vec![];
    let mut classpath_libs = vec![];
    let mut seen = std::collections::HashSet::new();
    for lib in profile.get("libraries").and_then(|l| l.as_array()).cloned().unwrap_or_default() {
        let in_cp = lib.get("include_in_classpath").and_then(|v| v.as_bool()).unwrap_or(true);
        if let Some(a) = lib.pointer("/downloads/artifact") {
            let path = a.get("path").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let sha1 = a.get("sha1").and_then(|v| v.as_str()).unwrap_or_default().to_ascii_lowercase();
            let url = a.get("url").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            if in_cp {
                classpath_libs.push(lib.clone());
            }
            if url.is_empty() || !seen.insert(path.clone()) {
                continue; // made by a processor, or listed twice
            }
            if !crate::valid_sha1(&sha1) || !url.starts_with("https://") {
                return Err(err("A loader library has no usable checksum.", &path));
            }
            let size = a.get("size").and_then(|v| v.as_u64());
            let dest = safe_join(&libs_dir, &path)?;
            files.push(RecordedFile { path: format!("libraries/{path}"), sha1: sha1.clone(), size: size.unwrap_or(0), url: url.clone() });
            reqs.push(DownloadRequest { url, dest, checksum: Checksum::Sha1(sha1), size, reuse_from: svc.reuse_roots.iter().filter_map(|r| safe_join(&r.join("libraries"), &path).ok()).collect() });
        } else if let Some(name) = lib.get("name").and_then(|v| v.as_str()) {
            let path = maven_path(name).ok_or_else(|| err("A loader library has a bad name.", name))?;
            if name.contains("installer-extracts") {
                if name.contains(":client@") {
                    extracts.push(("data/client.lzma".into(), safe_join(&libs_dir, &path)?));
                }
                continue;
            }
            return Err(err("A loader library has no checksum.", name));
        }
    }
    tracing::info!(log_source = "install", loader = loader.label(), version = %id, libraries = reqs.len(), "installing loader");
    dl.download_all(reqs, progress, cancel.clone()).await?;

    // pull the patch set out of the verified installer
    {
        let f = std::fs::File::open(&installer).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "open the installer", &installer, e))?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| err("The installer is damaged.", e))?;
        for (entry, dest) in &extracts {
            let mut e = z.by_name(entry).map_err(|e| err("The installer is missing a file.", e))?;
            if let Some(d) = dest.parent() {
                std::fs::create_dir_all(d).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "create a folder", d, e))?;
            }
            let mut out = std::fs::File::create(dest).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "write a file", dest, e))?;
            std::io::copy(&mut (&mut e).take(256 * 1024 * 1024), &mut out).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "write a file", dest, e))?;
        }
    }

    // 3. processors
    let vanilla_jar = svc.paths.versions_dir().join(mc).join(format!("{mc}.jar"));
    let java = crate::java::ensure_runtime(dl, &svc.paths.data_root.join("runtimes"), &java_component(svc, mc), svc.platform, crate::java::RUNTIME_INDEX_URL, Arc::new(Progress::new()), cancel.clone()).await?;
    run_processors(svc, &profile, mc, &vanilla_jar, &installer, &java)?;

    // NeoForge/Forge expect the game jar under the profile's own name
    let dir = svc.paths.versions_dir().join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "create a folder", &dir, e))?;
    let own_jar = dir.join(format!("{id}.jar"));
    std::fs::copy(&vanilla_jar, &own_jar).map_err(|e| AtlasError::io(ErrorCode::PreparationFailed, "copy the game jar", &own_jar, e))?;

    // keep only classpath libraries in the saved profile; processors are done
    if let Some(o) = profile.as_object_mut() {
        o.remove("processors");
        o.remove("data");
        o.insert("libraries".into(), Value::Array(classpath_libs));
        o.insert("atlasOwnJar".into(), Value::Bool(true));
    }
    let _ = std::fs::remove_dir_all(&tmp);
    svc.write_profile(&id, &profile, files, loader, lv, mc)?;
    Ok(id)
}

fn java_component(svc: &VersionService, mc: &str) -> String {
    std::fs::read(svc.paths.versions_dir().join(mc).join(format!("{mc}.json")))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.pointer("/javaVersion/component").and_then(|c| c.as_str()).map(str::to_owned))
        .unwrap_or_else(|| "jre-legacy".into())
}

fn run_processors(svc: &VersionService, profile: &Value, mc: &str, vanilla_jar: &Path, installer: &Path, java: &Path) -> AtlasResult<()> {
    let libs = svc.paths.libraries_dir();
    let lib_path = |coord: &str| -> AtlasResult<PathBuf> {
        let p = maven_path(coord).ok_or_else(|| err("A processor refers to a bad library.", coord))?;
        safe_join(&libs, &p)
    };
    // {KEY} values for the client side
    let mut data: HashMap<String, String> = HashMap::new();
    for (k, v) in profile.get("data").and_then(|d| d.as_object()).cloned().unwrap_or_default() {
        let Some(c) = v.get("client").and_then(|c| c.as_str()) else { continue };
        let resolved = if let Some(coord) = c.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            lib_path(coord)?.to_string_lossy().into_owned()
        } else if let Some(lit) = c.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
            lit.to_string()
        } else {
            c.to_string()
        };
        data.insert(k, resolved);
    }
    data.insert("SIDE".into(), "client".into());
    data.insert("MINECRAFT_JAR".into(), vanilla_jar.to_string_lossy().into_owned());
    data.insert("MINECRAFT_VERSION".into(), mc.into());
    data.insert("ROOT".into(), svc.paths.data_root.to_string_lossy().into_owned());
    data.insert("LIBRARY_DIR".into(), libs.to_string_lossy().into_owned());
    data.insert("INSTALLER".into(), installer.to_string_lossy().into_owned());

    let resolve = |a: &str| -> AtlasResult<String> {
        if let Some(coord) = a.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            return Ok(lib_path(coord)?.to_string_lossy().into_owned());
        }
        if let Some(key) = a.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            return data.get(key).cloned().ok_or_else(|| err("A processor uses an unknown value.", key));
        }
        if let Some(lit) = a.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
            return Ok(lit.to_string());
        }
        Ok(a.to_string())
    };
    let sep = if cfg!(windows) { ";" } else { ":" };
    for p in profile.get("processors").and_then(|p| p.as_array()).cloned().unwrap_or_default() {
        if let Some(sides) = p.get("sides").and_then(|s| s.as_array())
            && !sides.iter().any(|s| s.as_str() == Some("client"))
        {
            continue;
        }
        let jar_coord = p.get("jar").and_then(|j| j.as_str()).ok_or_else(|| err("A processor has no jar.", &p))?;
        let jar = lib_path(jar_coord)?;
        // outputs already right? then this step ran before
        let outputs: Vec<(PathBuf, Option<String>)> = p
            .get("outputs")
            .and_then(|o| o.as_object())
            .map(|o| o.iter().map(|(k, v)| Ok((PathBuf::from(resolve(k)?), v.as_str().map(&resolve).transpose()?))).collect::<AtlasResult<Vec<_>>>())
            .transpose()?
            .unwrap_or_default();
        if !outputs.is_empty() && outputs.iter().all(|(f, sha)| sha.as_ref().is_some_and(|s| verify_file(f, &Checksum::Sha1(s.to_ascii_lowercase()), None).unwrap_or(false))) {
            continue;
        }
        let mut cp = vec![jar.to_string_lossy().into_owned()];
        for c in p.get("classpath").and_then(|c| c.as_array()).into_iter().flatten().filter_map(|c| c.as_str()) {
            cp.push(lib_path(c)?.to_string_lossy().into_owned());
        }
        let args: Vec<String> = p.get("args").and_then(|a| a.as_array()).into_iter().flatten().filter_map(|a| a.as_str()).map(resolve).collect::<AtlasResult<_>>()?;
        let main = main_class_of(&jar)?;
        tracing::info!(log_source = "install", processor = jar_coord, "running installer step");
        let out = std::process::Command::new(java).arg("-cp").arg(cp.join(sep)).arg(&main).args(&args).current_dir(&svc.paths.data_root).output().map_err(|e| err("Java couldn't run an installer step.", e))?;
        for line in String::from_utf8_lossy(&out.stdout).lines().chain(String::from_utf8_lossy(&out.stderr).lines()) {
            tracing::debug!(log_source = "install", "{line}");
        }
        if !out.status.success() {
            let tail: Vec<_> = String::from_utf8_lossy(&out.stderr).lines().rev().take(10).map(str::to_owned).collect();
            return Err(err("A loader installer step failed.", format!("{jar_coord}: {}", tail.into_iter().rev().collect::<Vec<_>>().join("\n"))));
        }
        for (f, sha) in &outputs {
            if let Some(s) = sha
                && !verify_file(f, &Checksum::Sha1(s.to_ascii_lowercase()), None)?
            {
                return Err(AtlasError::new(ErrorCode::ChecksumMismatch, "A loader installer step produced the wrong file.").details(f.display()).suggest("Try installing again."));
            }
        }
    }
    Ok(())
}
