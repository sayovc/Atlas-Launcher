//! One-time codes for destructive tools. The assistant asks, the human
//! approves in a terminal, the assistant retries with the code.

use atlas_shared::fs_safety::atomic_write;
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const TTL_SECS: u64 = 10 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pending {
    pub code: String,
    /// What it's for, e.g. "remove_version:1.21.4". Codes only work for that exact action.
    pub action: String,
    pub created: u64,
    pub approved: bool,
}

pub struct Approvals {
    file: PathBuf,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Approvals {
    pub fn new(dir: PathBuf) -> Self {
        Self { file: dir.join("approvals.json") }
    }

    fn read(&self) -> Vec<Pending> {
        let list: Vec<Pending> = std::fs::read(&self.file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        list.into_iter().filter(|p| now() < p.created + TTL_SECS).collect()
    }

    fn write(&self, list: &[Pending]) -> AtlasResult<()> {
        let b = serde_json::to_vec_pretty(list).map_err(|e| AtlasError::new(ErrorCode::Internal, "Couldn't save approvals.").details(e))?;
        atomic_write(&self.file, &b)
    }

    pub fn request(&self, action: &str) -> AtlasResult<String> {
        let code = format!("{:06}", rand::random::<u32>() % 1_000_000);
        let mut list = self.read();
        list.push(Pending { code: code.clone(), action: action.into(), created: now(), approved: false });
        self.write(&list)?;
        Ok(code)
    }

    pub fn pending(&self) -> Vec<Pending> {
        self.read().into_iter().filter(|p| !p.approved).collect()
    }

    pub fn approve(&self, code: &str) -> AtlasResult<Pending> {
        let mut list = self.read();
        let p = list.iter_mut().find(|p| p.code == code && !p.approved).ok_or_else(|| AtlasError::new(ErrorCode::InvalidArgument, "No pending request with that code (they expire after 10 minutes)."))?;
        p.approved = true;
        let out = p.clone();
        self.write(&list)?;
        Ok(out)
    }

    /// Use up an approved code for exactly this action.
    pub fn consume(&self, code: &str, action: &str) -> bool {
        let mut list = self.read();
        let Some(i) = list.iter().position(|p| p.code == code && p.action == action && p.approved) else { return false };
        list.remove(i);
        self.write(&list).is_ok()
    }
}
