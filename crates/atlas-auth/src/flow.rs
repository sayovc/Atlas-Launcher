//! The network side of signing in.

use crate::Secret;
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const SCOPE: &str = "XboxLive.signin offline_access";

/// All the URLs involved, so tests can point them at a local fake.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub authorize: String,
    pub token: String,
    pub device_code: String,
    pub xbl: String,
    pub xsts: String,
    pub mc_login: String,
    pub mc_entitlements: String,
    pub mc_profile: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            authorize: "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize".into(),
            token: "https://login.microsoftonline.com/consumers/oauth2/v2.0/token".into(),
            device_code: "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode".into(),
            xbl: "https://user.auth.xboxlive.com/user/authenticate".into(),
            xsts: "https://xsts.auth.xboxlive.com/xsts/authorize".into(),
            mc_login: "https://api.minecraftservices.com/authentication/login_with_xbox".into(),
            mc_entitlements: "https://api.minecraftservices.com/entitlements/mcstore".into(),
            mc_profile: "https://api.minecraftservices.com/minecraft/profile".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MsTokens {
    pub access: Secret,
    pub refresh: Secret,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McSession {
    pub uuid: String,
    pub name: String,
    pub xuid: String,
    pub token: Secret,
    pub expires_at: u64,
    pub owns_game: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    pub message: String,
    #[serde(skip)]
    device_code: Secret,
    #[serde(skip)]
    interval: u64,
    #[serde(skip)]
    expires_at: u64,
}

/// A browser login in progress: open `url`, then call `finish_browser`.
pub struct BrowserLogin {
    pub url: String,
    listener: TcpListener,
    redirect: String,
    verifier: String,
    state: String,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn rand_token(bytes: usize) -> String {
    let buf: Vec<u8> = (0..bytes).map(|_| rand::random::<u8>()).collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

pub(crate) fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

fn net(what: &str, e: impl std::fmt::Display) -> AtlasError {
    AtlasError::new(ErrorCode::NetworkError, format!("Couldn't reach {what}."))
        .details(e.to_string())
        .suggest("Check your connection and try again.")
}

fn auth_fail(msg: impl Into<String>, details: impl std::fmt::Display) -> AtlasError {
    AtlasError::new(ErrorCode::AuthFailed, msg).details(details.to_string())
}

/// Known XSTS error codes, in words people can act on.
fn xsts_error(code: u64) -> AtlasError {
    let (msg, fix) = match code {
        2148916233 => ("This Microsoft account doesn't have an Xbox profile yet.", "Sign in once at xbox.com to create one, then try again."),
        2148916235 => ("Xbox Live isn't available in your account's country or region.", "There's nothing Atlas can do about this one, sorry."),
        2148916236 | 2148916237 => ("This account needs adult verification on Xbox.", "Visit xbox.com and complete the verification."),
        2148916238 => ("This is a child account and needs to be added to a Microsoft family.", "An adult in the family can add it at account.microsoft.com/family."),
        _ => ("Xbox Live refused the sign-in.", "Try again later."),
    };
    AtlasError::new(ErrorCode::AuthFailed, msg).details(format!("XSTS error {code}")).suggest(fix)
}

pub struct MsAuth {
    http: reqwest::Client,
    pub endpoints: Endpoints,
    client_id: String,
}

impl MsAuth {
    pub fn new(client_id: String, endpoints: Endpoints) -> AtlasResult<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!("AtlasLauncher/{}", atlas_shared::ATLAS_VERSION))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| AtlasError::new(ErrorCode::Internal, "Couldn't start the network client.").details(e))?;
        Ok(Self { http, endpoints, client_id })
    }

    /// Step 1 of the browser flow: listen on a random local port and build
    /// the Microsoft sign-in URL.
    pub async fn begin_browser(&self) -> AtlasResult<BrowserLogin> {
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| net("a local port for the sign-in callback", e))?;
        let port = listener.local_addr().map_err(|e| net("a local port", e))?.port();
        let redirect = format!("http://localhost:{port}");
        let verifier = rand_token(48);
        let state = rand_token(16);
        let mut url = url::Url::parse(&self.endpoints.authorize).map_err(|e| auth_fail("Bad sign-in URL.", e))?;
        url.query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &redirect)
            .append_pair("scope", SCOPE)
            .append_pair("code_challenge", &pkce_challenge(&verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("prompt", "select_account");
        Ok(BrowserLogin { url: url.to_string(), listener, redirect, verifier, state })
    }

    /// Step 2: wait for the browser to come back with a code (5 minutes max).
    pub async fn finish_browser(&self, login: BrowserLogin, cancel: &CancellationToken) -> AtlasResult<MsTokens> {
        let wait = async {
            loop {
                let (mut sock, _) = login.listener.accept().await.map_err(|e| net("the sign-in callback", e))?;
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let parsed = url::Url::parse(&format!("http://localhost{path}")).ok();
                let get = |k: &str| parsed.as_ref().and_then(|u| u.query_pairs().find(|(a, _)| a == k).map(|(_, v)| v.into_owned()));
                // browsers also ask for /favicon.ico and such; ignore those
                if get("code").is_none() && get("error").is_none() {
                    let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    continue;
                }
                let ok = get("state").as_deref() == Some(login.state.as_str()) && get("code").is_some();
                let body = if ok { PAGE_OK } else { PAGE_FAIL };
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = sock.write_all(resp.as_bytes()).await;
                if let Some(err) = get("error") {
                    let desc = get("error_description").unwrap_or_default();
                    return Err(if err == "access_denied" {
                        AtlasError::new(ErrorCode::AuthFailed, "Sign-in was cancelled.")
                    } else {
                        auth_fail("Microsoft sign-in failed.", format!("{err}: {desc}"))
                    });
                }
                if !ok {
                    return Err(auth_fail("The sign-in response didn't match the request.", "state mismatch").suggest("Start the sign-in again."));
                }
                return Ok(get("code").unwrap_or_default());
            }
        };
        let code = tokio::select! {
            _ = cancel.cancelled() => return Err(AtlasError::cancelled()),
            _ = tokio::time::sleep(Duration::from_secs(300)) => return Err(AtlasError::new(ErrorCode::AuthFailed, "Sign-in timed out.").suggest("Start it again and finish within 5 minutes.")),
            r = wait => r?,
        };
        self.token(&[
            ("client_id", self.client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &login.redirect),
            ("code_verifier", &login.verifier),
            ("scope", SCOPE),
        ])
        .await?
        .ok_or_else(|| auth_fail("Microsoft didn't return a token.", "pending"))
    }

    /// Device code flow, for when a browser can't reach localhost.
    pub async fn device_code(&self) -> AtlasResult<DeviceCode> {
        let r: Value = self
            .http
            .post(&self.endpoints.device_code)
            .form(&[("client_id", self.client_id.as_str()), ("scope", SCOPE)])
            .send()
            .await
            .map_err(|e| net("Microsoft", e))?
            .json()
            .await
            .map_err(|e| net("Microsoft", e))?;
        let s = |k: &str| r.get(k).and_then(|v| v.as_str()).map(str::to_owned);
        Ok(DeviceCode {
            user_code: s("user_code").ok_or_else(|| auth_fail("Microsoft didn't give a device code.", &r))?,
            verification_uri: s("verification_uri").unwrap_or_else(|| "https://www.microsoft.com/link".into()),
            message: s("message").unwrap_or_default(),
            device_code: Secret(s("device_code").unwrap_or_default()),
            interval: r.get("interval").and_then(|v| v.as_u64()).unwrap_or(5),
            expires_at: now() + r.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(900),
        })
    }

    pub async fn poll_device(&self, dc: &DeviceCode, cancel: &CancellationToken) -> AtlasResult<MsTokens> {
        let mut interval = dc.interval.max(1);
        loop {
            if now() > dc.expires_at {
                return Err(AtlasError::new(ErrorCode::AuthFailed, "The device code expired.").suggest("Start the sign-in again."));
            }
            tokio::select! {
                _ = cancel.cancelled() => return Err(AtlasError::cancelled()),
                _ = tokio::time::sleep(Duration::from_secs(interval)) => {}
            }
            match self
                .token(&[
                    ("client_id", self.client_id.as_str()),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("device_code", dc.device_code.expose()),
                ])
                .await
            {
                Ok(Some(t)) => return Ok(t),
                Ok(None) => {}
                Err(e) if e.details.contains("slow_down") => interval += 5,
                Err(e) => return Err(e),
            }
        }
    }

    pub async fn refresh(&self, refresh: &Secret) -> AtlasResult<MsTokens> {
        self.token(&[("client_id", self.client_id.as_str()), ("grant_type", "refresh_token"), ("refresh_token", refresh.expose()), ("scope", SCOPE)])
            .await?
            .ok_or_else(|| AtlasError::new(ErrorCode::AuthExpired, "Your sign-in expired.").suggest("Sign in again."))
    }

    /// Ok(None) means "authorization_pending" (device flow keeps polling).
    async fn token(&self, form: &[(&str, &str)]) -> AtlasResult<Option<MsTokens>> {
        let resp = self.http.post(&self.endpoints.token).form(form).send().await.map_err(|e| net("Microsoft", e))?;
        let v: Value = resp.json().await.map_err(|e| net("Microsoft", e))?;
        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
            return match err {
                "authorization_pending" => Ok(None),
                "invalid_grant" => Err(AtlasError::new(ErrorCode::AuthExpired, "Your Microsoft sign-in expired or was revoked.").suggest("Sign in again.")),
                _ => Err(auth_fail("Microsoft sign-in failed.", format!("{err}: {}", v.get("error_description").and_then(|d| d.as_str()).unwrap_or("")))),
            };
        }
        let access = v.get("access_token").and_then(|t| t.as_str()).ok_or_else(|| auth_fail("Microsoft didn't return a token.", "no access_token"))?;
        let refresh = v.get("refresh_token").and_then(|t| t.as_str()).unwrap_or_default();
        Ok(Some(MsTokens { access: Secret(access.into()), refresh: Secret(refresh.into()) }))
    }

    /// Microsoft token -> Minecraft session (with ownership check).
    pub async fn minecraft(&self, ms: &MsTokens) -> AtlasResult<McSession> {
        // Xbox Live
        let xbl: Value = self
            .post_json(
                &self.endpoints.xbl,
                json!({"Properties": {"AuthMethod": "RPS", "SiteName": "user.auth.xboxlive.com", "RpsTicket": format!("d={}", ms.access.expose())},
                       "RelyingParty": "http://auth.xboxlive.com", "TokenType": "JWT"}),
                "Xbox Live",
            )
            .await?;
        let xbl_token = xbl.get("Token").and_then(|t| t.as_str()).ok_or_else(|| auth_fail("Xbox Live sign-in failed.", "no token"))?;

        // XSTS for Minecraft services
        let resp = self
            .http
            .post(&self.endpoints.xsts)
            .json(&json!({"Properties": {"SandboxId": "RETAIL", "UserTokens": [xbl_token]}, "RelyingParty": "rp://api.minecraftservices.com/", "TokenType": "JWT"}))
            .send()
            .await
            .map_err(|e| net("Xbox Live", e))?;
        let status = resp.status();
        let xsts: Value = resp.json().await.unwrap_or(Value::Null);
        if status.as_u16() == 401 {
            return Err(xsts_error(xsts.get("XErr").and_then(|x| x.as_u64()).unwrap_or(0)));
        }
        if !status.is_success() {
            return Err(auth_fail("Xbox Live refused the sign-in.", format!("XSTS HTTP {status}")));
        }
        let xsts_token = xsts.get("Token").and_then(|t| t.as_str()).ok_or_else(|| auth_fail("Xbox Live sign-in failed.", "no XSTS token"))?;
        let claims = xsts.pointer("/DisplayClaims/xui/0");
        let uhs = claims.and_then(|c| c.get("uhs")).and_then(|u| u.as_str()).ok_or_else(|| auth_fail("Xbox Live sign-in failed.", "no user hash"))?;
        let xuid = claims.and_then(|c| c.get("xid")).and_then(|u| u.as_str()).unwrap_or("0").to_string();

        // Minecraft
        let resp = self
            .http
            .post(&self.endpoints.mc_login)
            .json(&json!({"identityToken": format!("XBL3.0 x={uhs};{xsts_token}")}))
            .send()
            .await
            .map_err(|e| net("Minecraft services", e))?;
        let st = resp.status();
        let mc: Value = resp.json().await.unwrap_or(Value::Null);
        if st.as_u16() == 403 || mc.to_string().contains("Invalid app registration") {
            return Err(AtlasError::new(ErrorCode::AuthFailed, "Minecraft services haven't approved this app yet.")
                .details(format!("login_with_xbox HTTP {st}"))
                .suggest("The Atlas app id needs Mojang's approval (aka.ms/mce-reviewappid). Nothing is wrong with your account."));
        }
        let token = mc.get("access_token").and_then(|t| t.as_str()).ok_or_else(|| auth_fail("Minecraft sign-in failed.", format!("HTTP {st}")))?;
        let expires_in = mc.get("expires_in").and_then(|t| t.as_u64()).unwrap_or(86400);

        // ownership: entitlements, then profile (Game Pass shows up as a profile)
        let ents: Value = self.get_bearer(&self.endpoints.mc_entitlements, token).await?.1;
        let has_ent = ents.get("items").and_then(|i| i.as_array()).is_some_and(|items| {
            items.iter().any(|it| it.get("name").and_then(|n| n.as_str()).is_some_and(|n| n == "game_minecraft" || n == "product_minecraft"))
        });
        let (pst, profile) = self.get_bearer(&self.endpoints.mc_profile, token).await?;
        let (uuid, name) = match (pst, profile.get("id").and_then(|v| v.as_str()), profile.get("name").and_then(|v| v.as_str())) {
            (200, Some(id), Some(n)) => (id.to_string(), n.to_string()),
            _ => {
                return Err(AtlasError::new(ErrorCode::AuthFailed, "This account doesn't own Minecraft: Java Edition (or has no player profile yet).")
                    .details(format!("profile HTTP {pst}, entitlement: {has_ent}"))
                    .suggest("Buy the game or set up your profile at minecraft.net. You can still play the demo."));
            }
        };
        Ok(McSession { uuid, name, xuid, token: Secret(token.into()), expires_at: now() + expires_in, owns_game: true })
    }

    async fn post_json(&self, url: &str, body: Value, what: &str) -> AtlasResult<Value> {
        let r = self.http.post(url).header("Accept", "application/json").json(&body).send().await.map_err(|e| net(what, e))?;
        let st = r.status();
        if !st.is_success() {
            return Err(auth_fail(format!("{what} refused the sign-in."), format!("HTTP {st}")));
        }
        r.json().await.map_err(|e| net(what, e))
    }

    async fn get_bearer(&self, url: &str, token: &str) -> AtlasResult<(u16, Value)> {
        let r = self.http.get(url).bearer_auth(token).send().await.map_err(|e| net("Minecraft services", e))?;
        let st = r.status().as_u16();
        Ok((st, r.json().await.unwrap_or(Value::Null)))
    }
}

const PAGE_OK: &str = "<!doctype html><meta charset=utf-8><title>Atlas</title><body style=\"background:#0c0f14;color:#e6eaf0;font:16px system-ui;display:grid;place-items:center;height:100vh;margin:0\"><div style=\"text-align:center\"><h1 style=\"color:#3fd0c0\">Signed in</h1><p>You can close this tab and go back to Atlas.</p></div>";
const PAGE_FAIL: &str = "<!doctype html><meta charset=utf-8><title>Atlas</title><body style=\"background:#0c0f14;color:#e6eaf0;font:16px system-ui;display:grid;place-items:center;height:100vh;margin:0\"><div style=\"text-align:center\"><h1 style=\"color:#f26d6d\">Sign-in didn't finish</h1><p>Go back to Atlas and try again.</p></div>";
