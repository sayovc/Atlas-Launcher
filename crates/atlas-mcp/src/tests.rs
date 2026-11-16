#![allow(clippy::unwrap_used)]
use super::*;
use atlas_config::AtlasPaths;

fn atlas(enabled: bool) -> (tempfile::TempDir, Atlas) {
    let t = tempfile::tempdir().unwrap();
    let a = Atlas::open(AtlasPaths::explicit(t.path().join("boot"), t.path().join("data"))).unwrap();
    if enabled {
        a.set_setting("advanced.ai.enabled", "true").unwrap();
        a.set_setting("advanced.ai.mcp_enabled", "true").unwrap();
    }
    (t, a)
}

fn call(s: &Server, name: &str, args: Value) -> Value {
    s.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}})).unwrap()
}

#[test]
fn off_by_default_and_kill_switch_works_mid_session() {
    let (_t, a) = atlas(false);
    assert!(check_enabled(&a).is_err());
    let s = Server::new(&a);
    let r = s.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).unwrap();
    assert!(r.get("error").is_some(), "{r}");

    a.set_setting("advanced.ai.enabled", "true").unwrap();
    a.set_setting("advanced.ai.mcp_enabled", "true").unwrap();
    let r = s.handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
    assert!(r["result"]["tools"].as_array().unwrap().len() > 5);

    a.set_setting("advanced.ai.enabled", "false").unwrap();
    let r = s.handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"})).unwrap();
    assert!(r.get("error").is_some(), "turning AI off stops a running server");
}

#[test]
fn token_is_required_and_private() {
    let (_t, a) = atlas(true);
    let tok = ensure_token(&a.paths().bootstrap_dir).unwrap();
    assert_eq!(tok.len(), 64);
    assert!(check_token(&a, Some(&tok)).is_ok());
    assert!(check_token(&a, Some("nope")).is_err());
    assert!(check_token(&a, None).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(token_file(&a.paths().bootstrap_dir)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn initialize_and_notifications() {
    let (_t, a) = atlas(true);
    let s = Server::new(&a);
    let r = s.handle(&json!({"jsonrpc": "2.0", "id": "x", "method": "initialize", "params": {}})).unwrap();
    assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert!(s.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
}

#[test]
fn settings_tool_cannot_touch_ai_or_accounts() {
    let (_t, a) = atlas(true);
    let s = Server::new(&a);
    let r = call(&s, "atlas_settings", json!({"key": "advanced.ai.enabled", "value": "true"}));
    assert_eq!(r["result"]["isError"], true);
    let r = call(&s, "atlas_settings", json!({"key": "accounts.active_account", "value": "x"}));
    assert_eq!(r["result"]["isError"], true);
    let r = call(&s, "atlas_settings", json!({"key": "memory.max_mb", "value": "6144"}));
    assert_eq!(r["result"]["isError"], false);
    assert_eq!(a.settings().memory.max_mb, 6144);
}

#[test]
fn destructive_tool_needs_human_approval() {
    let (_t, a) = atlas(true);
    let s = Server::new(&a);
    let r = call(&s, "atlas_remove_version", json!({"version": "1.21.4"}));
    let code = r["result"]["structuredContent"]["code"].as_str().unwrap().to_string();
    // retrying with the code before approval fails
    let r = call(&s, "atlas_remove_version", json!({"version": "1.21.4", "confirm_code": code}));
    assert_eq!(r["result"]["isError"], true);
    // a human approves; the code works once, and only for that version
    Approvals::new(a.paths().data_root.join("mcp")).approve(&code).unwrap();
    let r = call(&s, "atlas_remove_version", json!({"version": "9.9", "confirm_code": code}));
    assert_eq!(r["result"]["isError"], true, "code is bound to the version it was issued for");
    let r = call(&s, "atlas_remove_version", json!({"version": "1.21.4", "confirm_code": code}));
    // version isn't installed in this test, so the removal itself reports VERSION_NOT_FOUND
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("VERSION_NOT_FOUND"), "{text}");
}

#[test]
fn status_never_includes_tokens() {
    let (_t, a) = atlas(true);
    let s = Server::new(&a);
    let r = call(&s, "atlas_status", json!({}));
    let text = r.to_string().to_lowercase();
    assert!(!text.contains("access_token") && !text.contains("refresh"));
}
