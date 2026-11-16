//! Turn a version JSON (+ asset index) into a verified download plan.

use crate::model::{AssetIndex, VersionJson};
use crate::rules::{allowed, Platform};
use crate::service::VersionPaths;
use crate::valid_sha1;
use atlas_downloads::{Checksum, DownloadRequest};
use atlas_shared::fs_safety::safe_join;
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use std::path::PathBuf;

fn bad_meta(msg: impl Into<String>) -> AtlasError {
    AtlasError::new(ErrorCode::InvalidInstallation, "The version metadata is malformed.")
        .details(msg.into())
        .suggest("Refresh the version list and try again. If it persists, the metadata source may be compromised.")
}

fn sha1(h: &str) -> AtlasResult<Checksum> {
    if valid_sha1(h) { Ok(Checksum::Sha1(h.to_ascii_lowercase())) } else { Err(bad_meta(format!("invalid sha1 {h:?}"))) }
}

/// Library + natives downloads that apply to `platform`.
pub(crate) fn libraries(v: &VersionJson, paths: &VersionPaths, reuse: &[PathBuf], platform: Platform) -> AtlasResult<Vec<DownloadRequest>> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for lib in &v.libraries {
        if !allowed(&lib.rules, platform) {
            continue;
        }
        let mut arts = Vec::new();
        if let Some(a) = &lib.downloads.artifact {
            arts.push(a);
        }
        // Legacy natives: "natives": {"linux": "natives-linux", "windows": "natives-windows-${arch}"}
        if let Some(key) = lib.natives.get(platform.os) {
            let key = key.replace("${arch}", platform.bits());
            if let Some(a) = lib.downloads.classifiers.get(&key) {
                arts.push(a);
            }
        }
        for a in arts {
            if !seen.insert(a.path.clone()) {
                continue;
            }
            let dest = safe_join(&paths.libraries_dir(), &a.path)?;
            out.push(DownloadRequest {
                url: a.url.clone(),
                dest,
                checksum: sha1(&a.sha1)?,
                size: Some(a.size),
                reuse_from: reuse.iter().filter_map(|r| safe_join(&r.join("libraries"), &a.path).ok()).collect(),
            });
        }
    }
    Ok(out)
}

pub(crate) fn assets(index: &AssetIndex, paths: &VersionPaths, reuse: &[PathBuf], base_url: &str) -> AtlasResult<Vec<DownloadRequest>> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for obj in index.objects.values() {
        let h = obj.hash.to_ascii_lowercase();
        if !valid_sha1(&h) {
            return Err(bad_meta(format!("invalid asset hash {h:?}")));
        }
        if !seen.insert(h.clone()) {
            continue; // same object referenced under several names
        }
        let rel = format!("{}/{}", &h[..2], h);
        out.push(DownloadRequest {
            url: format!("{base_url}/{rel}"),
            dest: paths.assets_dir().join("objects").join(&h[..2]).join(&h),
            checksum: Checksum::Sha1(h.clone()),
            size: Some(obj.size),
            reuse_from: reuse.iter().map(|r| r.join("assets/objects").join(&h[..2]).join(&h)).collect(),
        });
    }
    Ok(out)
}

pub(crate) fn client_jar(v: &VersionJson, dir: &std::path::Path, reuse: &[PathBuf]) -> AtlasResult<DownloadRequest> {
    let c = &v.downloads.client;
    Ok(DownloadRequest {
        url: c.url.clone(),
        dest: dir.join(format!("{}.jar", v.id)),
        checksum: sha1(&c.sha1)?,
        size: Some(c.size),
        reuse_from: reuse.iter().map(|r| r.join("versions").join(&v.id).join(format!("{}.jar", v.id))).collect(),
    })
}

pub(crate) fn logging_config(v: &VersionJson, paths: &VersionPaths) -> AtlasResult<Option<DownloadRequest>> {
    let Some(c) = &v.logging.client else { return Ok(None) };
    let f = &c.file;
    if !crate::valid_id(&f.id) {
        return Err(bad_meta(format!("invalid logging file id {:?}", f.id)));
    }
    Ok(Some(DownloadRequest {
        url: f.url.clone(),
        dest: paths.assets_dir().join("log_configs").join(&f.id),
        checksum: sha1(&f.sha1)?,
        size: Some(f.size),
        reuse_from: vec![],
    }))
}

pub(crate) fn asset_index(v: &VersionJson, paths: &VersionPaths, reuse: &[PathBuf]) -> AtlasResult<DownloadRequest> {
    let a = &v.asset_index;
    if !crate::valid_id(&a.id) {
        return Err(bad_meta(format!("invalid asset index id {:?}", a.id)));
    }
    Ok(DownloadRequest {
        url: a.url.clone(),
        dest: paths.assets_dir().join("indexes").join(format!("{}.json", a.id)),
        checksum: sha1(&a.sha1)?,
        size: Some(a.size),
        reuse_from: reuse.iter().map(|r| r.join("assets/indexes").join(format!("{}.json", a.id))).collect(),
    })
}
