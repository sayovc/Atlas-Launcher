//! A small, local MCP server so AI assistants can drive Atlas.
//!
//! Ground rules:
//! * Off unless both `advanced.ai.enabled` and `advanced.ai.mcp_enabled`
//!   are on. Checked at startup *and* on every call, so flipping the
//!   switch off stops it immediately.
//! * stdio only: the assistant has to launch `atlas mcp` itself, nothing
//!   listens on the network.
//! * The caller must present the token from `mcp-token` (user-only file).
//! * Nothing here can return account tokens; logs are redacted.
//! * Destructive tools need a one-time code that a human approves with
//!   `atlas mcp approve <code>`.

mod approvals;
mod tools;

pub use approvals::Approvals;

use atlas_core::Atlas;
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

pub fn token_file(bootstrap_dir: &Path) -> PathBuf {
    bootstrap_dir.join("mcp-token")
}

/// Read the token, creating one the first time.
pub fn ensure_token(bootstrap_dir: &Path) -> AtlasResult<String> {
    let p = token_file(bootstrap_dir);
    if let Ok(t) = std::fs::read_to_string(&p) {
        let t = t.trim().to_string();
        if t.len() >= 32 {
            return Ok(t);
        }
    }
    let bytes: Vec<u8> = (0..32).map(|_| rand::random::<u8>()).collect();
    let token = hex::encode(bytes);
    atlas_shared::fs_safety::atomic_write(&p, token.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).map_err(|e| AtlasError::io(ErrorCode::ConfigIo, "protect the MCP token", &p, e))?;
    }
    Ok(token)
}

pub fn check_enabled(atlas: &Atlas) -> AtlasResult<()> {
    let ai = atlas.settings().advanced.ai;
    if !ai.enabled {
        return Err(AtlasError::new(ErrorCode::InvalidArgument, "AI features are turned off in Atlas.").suggest("Turn on Settings → Advanced → Enable AI features, then the MCP server."));
    }
    if !ai.mcp_enabled {
        return Err(AtlasError::new(ErrorCode::InvalidArgument, "The Atlas MCP server is turned off.").suggest("Turn it on in Settings → Advanced."));
    }
    Ok(())
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn check_token(atlas: &Atlas, presented: Option<&str>) -> AtlasResult<()> {
    let want = ensure_token(&atlas.paths().bootstrap_dir)?;
    match presented {
        Some(p) if constant_time_eq(p.trim(), &want) => Ok(()),
        _ => Err(AtlasError::new(ErrorCode::AuthFailed, "Missing or wrong MCP token.").suggest("Set ATLAS_MCP_TOKEN to the value shown by `atlas mcp token`.")),
    }
}

pub struct Server<'a> {
    atlas: &'a Atlas,
    approvals: Approvals,
}

impl<'a> Server<'a> {
    pub fn new(atlas: &'a Atlas) -> Self {
        let approvals = Approvals::new(atlas.paths().data_root.join("mcp"));
        Self { atlas, approvals }
    }

    /// Handle one JSON-RPC message. Returns None for notifications.
    pub fn handle(&self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        id.as_ref()?; // notifications get no reply
        let reply = |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result});
        let error = |code: i64, message: String| json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}});

        // the kill switch applies to every request, not just startup
        if let Err(e) = check_enabled(self.atlas) {
            return Some(error(-32001, e.message));
        }
        Some(match method {
            "initialize" => reply(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "atlas-launcher", "version": atlas_shared::ATLAS_VERSION},
                "instructions": "Tools for Atlas Launcher (Minecraft: Java Edition). Destructive tools return a confirmation code the user must approve with `atlas mcp approve <code>`."
            })),
            "ping" => reply(json!({})),
            "tools/list" => reply(json!({ "tools": tools::definitions() })),
            "tools/call" => {
                let name = msg.pointer("/params/name").and_then(|n| n.as_str()).unwrap_or("");
                let args = msg.pointer("/params/arguments").cloned().unwrap_or(json!({}));
                tracing::info!(tool = name, "mcp tool call");
                match tools::call(self.atlas, &self.approvals, name, &args) {
                    Ok(v) => reply(json!({"content": [{"type": "text", "text": v.to_string()}], "structuredContent": v, "isError": false})),
                    Err(e) => reply(json!({"content": [{"type": "text", "text": serde_json::to_string(&e).unwrap_or_default()}], "isError": true})),
                }
            }
            _ => error(-32601, format!("unknown method {method}")),
        })
    }

    /// Serve newline-delimited JSON-RPC on stdin/stdout until EOF.
    pub fn run_stdio(&self) -> AtlasResult<()> {
        let stdin = std::io::stdin();
        let mut out = std::io::stdout();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Value>(&line) {
                Ok(msg) => self.handle(&msg),
                Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}})),
            };
            if let Some(r) = reply {
                let _ = writeln!(out, "{r}");
                let _ = out.flush();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
