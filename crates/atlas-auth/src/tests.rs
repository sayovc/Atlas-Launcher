#![allow(clippy::unwrap_used)]
use super::*;
use crate::flow::pkce_challenge;
use atlas_shared::ErrorCode;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Fake Microsoft + Xbox + Minecraft: route -> (status, json).
fn fake(routes: HashMap<&'static str, (u16, Value)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let s2 = seen.clone();
    let routes = Arc::new(routes);
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let Ok(mut stream) = stream else { continue };
            let routes = routes.clone();
            let seen = s2.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                let mut len = 0;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h == "\r\n" || h.is_empty() { break; }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") { len = v.trim().parse().unwrap_or(0); }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                let path = first.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap().to_string();
                seen.lock().unwrap().push(format!("{path} {}", String::from_utf8_lossy(&body)));
                let (st, v) = routes.get(path.as_str()).cloned().unwrap_or((404, json!({})));
                let b = v.to_string();
                let _ = write!(stream, "HTTP/1.1 {st} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len());
            });
        }
    });
    (base, seen)
}

fn ok_routes() -> HashMap<&'static str, (u16, Value)> {
    HashMap::from([
        ("/token", (200, json!({"access_token": "MS-ACCESS", "refresh_token": "MS-REFRESH", "expires_in": 3600}))),
        ("/xbl", (200, json!({"Token": "XBL-TOKEN", "DisplayClaims": {"xui": [{"uhs": "UHS"}]}}))),
        ("/xsts", (200, json!({"Token": "XSTS-TOKEN", "DisplayClaims": {"xui": [{"uhs": "UHS", "xid": "2535400000"}]}}))),
        ("/mc", (200, json!({"access_token": "MC-TOKEN", "expires_in": 86400}))),
        ("/ent", (200, json!({"items": [{"name": "product_minecraft"}, {"name": "game_minecraft"}]}))),
        ("/profile", (200, json!({"id": "0123456789abcdef0123456789abcdef", "name": "Alex"}))),
    ])
}

fn endpoints(base: &str) -> Endpoints {
    Endpoints {
        authorize: format!("{base}/authorize"),
        token: format!("{base}/token"),
        device_code: format!("{base}/device"),
        xbl: format!("{base}/xbl"),
        xsts: format!("{base}/xsts"),
        mc_login: format!("{base}/mc"),
        mc_entitlements: format!("{base}/ent"),
        mc_profile: format!("{base}/profile"),
    }
}

async fn browser_login(auth: &MsAuth, tamper_state: bool) -> atlas_shared::AtlasResult<MsTokens> {
    let login = auth.begin_browser().await?;
    let u = url::Url::parse(&login.url).unwrap();
    let q: HashMap<_, _> = u.query_pairs().into_owned().collect();
    assert_eq!(q["code_challenge_method"], "S256");
    assert!(q["redirect_uri"].starts_with("http://localhost:"));
    let state = if tamper_state { "evil".to_string() } else { q["state"].clone() };
    let cb = format!("{}/?code=THE-CODE&state={state}", q["redirect_uri"]);
    // play the browser: the favicon request first, then the real callback
    let fav = format!("{}/favicon.ico", q["redirect_uri"]);
    tokio::spawn(async move {
        let c = reqwest::Client::new();
        let _ = c.get(fav).send().await;
        let _ = c.get(cb).send().await;
    });
    auth.finish_browser(login, &CancellationToken::new()).await
}

#[test]
fn pkce_is_base64url_sha256() {
    // expected value computed separately with Python's hashlib + base64
    assert_eq!(pkce_challenge("dBjftJeZ4CVP-mJ92IcxN6a-yKtlRk-ajcdiCE9R0ww"), "722Sb95WYKGgBeTL5yIimMAlzE_OwpfDrCD2Rw7-6aY");
}

#[test]
fn secrets_hide_in_debug() {
    assert_eq!(format!("{:?}", Secret("abc".into())), "Secret([REDACTED])");
}

#[tokio::test]
async fn full_sign_in_and_account_storage() {
    let (base, seen) = fake(ok_routes());
    let auth = MsAuth::new("client-123".into(), endpoints(&base)).unwrap();
    let ms = browser_login(&auth, false).await.unwrap();
    assert_eq!(ms.access.expose(), "MS-ACCESS");
    let token_call = seen.lock().unwrap().iter().find(|l| l.starts_with("/token")).cloned().unwrap();
    assert!(token_call.contains("code_verifier=") && token_call.contains("code=THE-CODE"));

    let mc = auth.minecraft(&ms).await.unwrap();
    assert_eq!((mc.name.as_str(), mc.xuid.as_str()), ("Alex", "2535400000"));
    assert!(seen.lock().unwrap().iter().any(|l| l.starts_with("/mc") && l.contains("XBL3.0 x=UHS;XSTS-TOKEN")));

    let t = tempfile::tempdir().unwrap();
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    let accs = Accounts::new(t.path().into(), store.clone());
    let a = accs.add(&ms, mc).unwrap();
    assert_eq!(accs.list().unwrap().len(), 1);
    let file = std::fs::read_to_string(t.path().join("accounts.toml")).unwrap();
    for secret in ["MS-REFRESH", "MC-TOKEN", "MS-ACCESS"] {
        assert!(!file.contains(secret), "{secret} leaked into accounts.toml");
    }
    let s = accs.session(&a.id, None).await.unwrap();
    assert_eq!(s.token.expose(), "MC-TOKEN");
    accs.remove(&a.id).unwrap();
    assert!(store.get(&a.id).unwrap().is_none(), "logout deletes the stored tokens");
    assert!(accs.list().unwrap().is_empty());
}

#[tokio::test]
async fn state_mismatch_is_rejected() {
    let (base, _) = fake(ok_routes());
    let auth = MsAuth::new("c".into(), endpoints(&base)).unwrap();
    let e = browser_login(&auth, true).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::AuthFailed);
}

#[tokio::test]
async fn xbox_errors_are_explained() {
    let mut r = ok_routes();
    r.insert("/xsts", (401, json!({"XErr": 2148916238u64})));
    let (base, _) = fake(r);
    let auth = MsAuth::new("c".into(), endpoints(&base)).unwrap();
    let ms = MsTokens { access: Secret("a".into()), refresh: Secret("r".into()) };
    let e = auth.minecraft(&ms).await.unwrap_err();
    assert!(e.message.contains("child account"), "{e:?}");
}

#[tokio::test]
async fn not_owning_the_game() {
    let mut r = ok_routes();
    r.insert("/ent", (200, json!({"items": []})));
    r.insert("/profile", (404, json!({"error": "NOT_FOUND"})));
    let (base, _) = fake(r);
    let auth = MsAuth::new("c".into(), endpoints(&base)).unwrap();
    let ms = MsTokens { access: Secret("a".into()), refresh: Secret("r".into()) };
    let e = auth.minecraft(&ms).await.unwrap_err();
    assert!(e.message.contains("doesn't own"));
}

#[tokio::test]
async fn unapproved_app_is_explained() {
    let mut r = ok_routes();
    r.insert("/mc", (403, json!({"errorMessage": "Invalid app registration"})));
    let (base, _) = fake(r);
    let auth = MsAuth::new("c".into(), endpoints(&base)).unwrap();
    let ms = MsTokens { access: Secret("a".into()), refresh: Secret("r".into()) };
    let e = auth.minecraft(&ms).await.unwrap_err();
    assert!(e.message.contains("approved"));
}

#[tokio::test]
async fn expired_session_refreshes() {
    let (base, seen) = fake(ok_routes());
    let auth = MsAuth::new("c".into(), endpoints(&base)).unwrap();
    let t = tempfile::tempdir().unwrap();
    let accs = Accounts::new(t.path().into(), Arc::new(MemoryStore::default()));
    let ms = MsTokens { access: Secret("old".into()), refresh: Secret("OLD-REFRESH".into()) };
    let mc = McSession { uuid: "u1".into(), name: "Alex".into(), xuid: "x".into(), token: Secret("STALE".into()), expires_at: 0, owns_game: true };
    accs.add(&ms, mc).unwrap();
    let s = accs.session("u1", Some(&auth)).await.unwrap();
    assert_eq!(s.token.expose(), "MC-TOKEN");
    assert!(seen.lock().unwrap().iter().any(|l| l.contains("grant_type=refresh_token") && l.contains("OLD-REFRESH")));
    // and without an auth client an expired token is a clear AUTH_EXPIRED
    let accs2 = Accounts::new(t.path().join("b"), Arc::new(MemoryStore::default()));
    let mc = McSession { uuid: "u2".into(), name: "B".into(), xuid: "x".into(), token: Secret("S".into()), expires_at: 0, owns_game: true };
    accs2.add(&ms, mc).unwrap();
    assert_eq!(accs2.session("u2", None).await.unwrap_err().code, ErrorCode::AuthExpired);
}
