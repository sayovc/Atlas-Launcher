use crate::Approvals;
use atlas_core::{Atlas, Confirmation, PlayOptions};
use atlas_logging::{LogFilter, LogLevel};
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde_json::{json, Value};

fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({"name": name, "description": description, "inputSchema": {"type": "object", "properties": props, "required": required}})
}

pub fn definitions() -> Vec<Value> {
    vec![
        tool("atlas_status", "Current launcher state: selected installation, game state, signed-in account name.", json!({}), &[]),
        tool("atlas_versions", "List Minecraft versions. installed=true for installed ones only.", json!({"installed": {"type": "boolean"}, "snapshots": {"type": "boolean"}, "limit": {"type": "integer"}}), &[]),
        tool("atlas_install", "Install a Minecraft version (id, 'latest' or 'latest-snapshot'). Starts a background download and returns a job id.", json!({"version": {"type": "string"}}), &["version"]),
        tool("atlas_prepare", "Verify an installed version and repair missing or corrupt files.", json!({"version": {"type": "string"}}), &["version"]),
        tool("atlas_jobs", "Progress of installs and repairs.", json!({}), &[]),
        tool("atlas_instances", "List installations.", json!({}), &[]),
        tool("atlas_play", "Start the game for an installation (default: the selected one), optionally joining a server.", json!({"installation": {"type": "string"}, "server": {"type": "string"}}), &[]),
        tool("atlas_stop", "Ask the running game to quit.", json!({}), &[]),
        tool("atlas_servers", "List saved servers. ping=true also checks who's online.", json!({"ping": {"type": "boolean"}}), &[]),
        tool("atlas_mods", "List mods in an installation.", json!({"installation": {"type": "string"}}), &[]),
        tool("atlas_logs", "Recent launcher and game log lines (credentials are redacted).", json!({"level": {"type": "string", "enum": ["trace", "debug", "info", "warn", "error"]}, "search": {"type": "string"}, "limit": {"type": "integer"}}), &[]),
        tool("atlas_settings", "Read settings. With key+value, change a setting (AI and account settings can't be changed from here).", json!({"key": {"type": "string"}, "value": {"type": "string"}}), &[]),
        tool("atlas_remove_version", "Remove an installed version. Destructive: first call returns a code the user must approve with `atlas mcp approve <code>`, then call again with confirm_code.", json!({"version": {"type": "string"}, "confirm_code": {"type": "string"}}), &["version"]),
    ]
}

fn s<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(|v| v.as_str())
}

fn to<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

// keys an assistant must never flip: the AI switches themselves and account selection
const LOCKED_PREFIXES: &[&str] = &["advanced.ai", "accounts"];

pub fn call(atlas: &Atlas, approvals: &Approvals, name: &str, a: &Value) -> AtlasResult<Value> {
    match name {
        "atlas_status" => {
            let st = atlas.status();
            Ok(json!({"installation": st.installation, "minecraft_version": st.minecraft_version, "game_state": to(&st.game_state), "account": st.account, "installed_versions": st.installed_versions, "version_update": st.version_update}))
        }
        "atlas_versions" => {
            if a.get("installed").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Ok(to(&atlas.installed_versions()?));
            }
            let snaps = a.get("snapshots").and_then(|v| v.as_bool()).unwrap_or(false);
            let limit = a.get("limit").and_then(|v| v.as_u64()).unwrap_or(25) as usize;
            let list = atlas.versions(false)?;
            let rows: Vec<_> = list.versions.iter().filter(|v| snaps || v.kind == atlas_core::VersionKind::Release).take(limit).collect();
            Ok(json!({"latest_release": list.latest_release, "latest_snapshot": list.latest_snapshot, "versions": to(&rows)}))
        }
        "atlas_install" => {
            let v = s(a, "version").ok_or_else(|| AtlasError::new(ErrorCode::InvalidArgument, "version is required"))?;
            Ok(json!({"job": atlas.start_install(v)?}))
        }
        "atlas_prepare" => {
            let v = s(a, "version").ok_or_else(|| AtlasError::new(ErrorCode::InvalidArgument, "version is required"))?;
            let rep = atlas.verify_version(v)?;
            if rep.ok() {
                Ok(json!({"ok": true, "checked": rep.checked}))
            } else {
                Ok(json!({"ok": false, "missing": rep.missing.len(), "corrupt": rep.corrupt.len(), "repair_job": atlas.start_repair(v)?}))
            }
        }
        "atlas_jobs" => Ok(to(&atlas.jobs())),
        "atlas_instances" => Ok(to(&atlas.installations()?)),
        "atlas_play" => {
            atlas.play(s(a, "installation"), PlayOptions { server: s(a, "server").map(str::to_owned) })?;
            Ok(json!({"started": true}))
        }
        "atlas_stop" => {
            atlas.stop(None)?;
            Ok(json!({"stopping": true}))
        }
        "atlas_servers" => {
            let list = atlas.servers()?;
            if a.get("ping").and_then(|v| v.as_bool()).unwrap_or(false) {
                let rows: Vec<Value> = list
                    .iter()
                    .map(|sv| match atlas.ping_server(&sv.id) {
                        Ok(p) => json!({"id": sv.id, "name": sv.name, "address": sv.host_port(), "online": p.online, "max": p.max, "version": p.version, "motd": p.motd}),
                        Err(e) => json!({"id": sv.id, "name": sv.name, "address": sv.host_port(), "error": e.message}),
                    })
                    .collect();
                return Ok(Value::Array(rows));
            }
            // icons are big base64 blobs; leave them out
            Ok(Value::Array(list.iter().map(|sv| json!({"id": sv.id, "name": sv.name, "address": sv.host_port(), "favorite": sv.favorite, "version": sv.version})).collect()))
        }
        "atlas_mods" => Ok(to(&atlas.mods(s(a, "installation"))?)),
        "atlas_logs" => {
            let level = s(a, "level").and_then(LogLevel::parse).unwrap_or(LogLevel::Info);
            let limit = a.get("limit").and_then(|v| v.as_u64()).unwrap_or(100).min(1000) as usize;
            let f = LogFilter { min_level: level, search: s(a, "search").unwrap_or("").into(), source: None };
            let lines: Vec<String> = atlas.file_logs(&f, limit)?.iter().map(|r| r.to_line()).collect();
            Ok(json!({"lines": lines}))
        }
        "atlas_settings" => match (s(a, "key"), s(a, "value")) {
            (Some(k), Some(v)) => {
                if LOCKED_PREFIXES.iter().any(|p| k == *p || k.starts_with(&format!("{p}."))) {
                    return Err(AtlasError::new(ErrorCode::InvalidArgument, format!("'{k}' can't be changed through MCP.")).suggest("Change it in the Atlas app."));
                }
                atlas.set_setting(k, v)?;
                Ok(json!({"key": k, "value": to(&atlas.get_setting(k)?)}))
            }
            (Some(k), None) => Ok(json!({"key": k, "value": to(&atlas.get_setting(k)?)})),
            _ => Ok(to(&atlas.settings())),
        },
        "atlas_remove_version" => {
            let v = s(a, "version").ok_or_else(|| AtlasError::new(ErrorCode::InvalidArgument, "version is required"))?;
            let action = format!("remove_version:{v}");
            match s(a, "confirm_code") {
                Some(code) if approvals.consume(code, &action) => {
                    atlas.remove_version(v, Confirmation::Confirmed)?;
                    Ok(json!({"removed": v}))
                }
                Some(_) => Err(AtlasError::confirmation_required(&action).suggest("That code isn't approved (or is for something else). Ask the user to run `atlas mcp approve <code>`.")),
                None => {
                    let code = approvals.request(&action)?;
                    Ok(json!({"confirmation_required": true, "code": code, "tell_user": format!("To allow removing {v}, run: atlas mcp approve {code}")}))
                }
            }
        }
        other => Err(AtlasError::new(ErrorCode::InvalidArgument, format!("Unknown tool {other}"))),
    }
}
