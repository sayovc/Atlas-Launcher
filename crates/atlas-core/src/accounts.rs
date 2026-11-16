//! Accounts and signing in, wired to the app.

use crate::Atlas;
use atlas_auth::{Account, Accounts, DeviceCode, Endpoints, KeyringStore, MemoryStore, MsAuth, SecretStore};
use atlas_launch::LaunchAuth;
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::Serialize;
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LoginState {
    #[default]
    Idle,
    /// Waiting for the user to finish in their browser.
    Browser { url: String },
    /// Waiting for the user to enter `code` at `url`.
    Device { code: String, url: String },
    Done { account: Account },
    Failed { error: AtlasError },
}

#[derive(Default)]
pub(crate) struct Login {
    state: LoginState,
    cancel: Option<CancellationToken>,
}

pub(crate) type LoginRef = Arc<Mutex<Login>>;

fn not_configured() -> AtlasError {
    AtlasError::new(ErrorCode::AuthFailed, "Microsoft sign-in isn't set up in this build yet.")
        .suggest("This build has no Azure app id. You can still play the demo; builds with ATLAS_MS_CLIENT_ID set can sign in.")
}

impl Atlas {
    fn secret_store(&self) -> Arc<dyn SecretStore> {
        // "memory" is for tests only: nothing is written anywhere and it's gone when Atlas closes
        if std::env::var("ATLAS_SECRET_STORE").as_deref() == Ok("memory") {
            return self.memory_secrets.clone();
        }
        Arc::new(KeyringStore)
    }

    pub(crate) fn account_store(&self) -> Accounts {
        Accounts::new(self.paths().accounts_dir(), self.secret_store())
    }

    fn ms_auth(&self) -> AtlasResult<MsAuth> {
        let id = atlas_auth::client_id().ok_or_else(not_configured)?;
        MsAuth::new(id, Endpoints::default())
    }

    pub fn sign_in_available(&self) -> bool {
        atlas_auth::client_id().is_some()
    }

    pub fn accounts(&self) -> AtlasResult<Vec<Account>> {
        self.account_store().list()
    }

    pub fn active_account(&self) -> Option<Account> {
        let list = self.accounts().ok()?;
        let want = self.settings().accounts.active_account;
        want.and_then(|id| list.iter().find(|a| a.id == id).cloned()).or_else(|| list.into_iter().next())
    }

    pub fn active_account_name(&self) -> Option<String> {
        self.active_account().map(|a| a.name)
    }

    pub fn set_active_account(&self, id: &str) -> AtlasResult<()> {
        if !self.accounts()?.iter().any(|a| a.id == id) {
            return Err(AtlasError::new(ErrorCode::InvalidArgument, "No such account."));
        }
        let mut s = self.settings();
        s.accounts.active_account = Some(id.into());
        self.save_settings(s).map(|_| ())
    }

    /// Log out: forget the account and delete its tokens from the keychain.
    pub fn remove_account(&self, id: &str) -> AtlasResult<()> {
        self.account_store().remove(id)?;
        if self.settings().accounts.active_account.as_deref() == Some(id) {
            let mut s = self.settings();
            s.accounts.active_account = self.accounts()?.first().map(|a| a.id.clone());
            self.save_settings(s)?;
        }
        Ok(())
    }

    pub fn login_state(&self) -> LoginState {
        self.login.lock().unwrap_or_else(|p| p.into_inner()).state.clone()
    }

    pub fn cancel_login(&self) {
        let mut l = self.login.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(c) = l.cancel.take() {
            c.cancel();
        }
        l.state = LoginState::Idle;
    }

    fn finish_login(&self, auth: MsAuth, cancel: CancellationToken, wait: impl Future<Output = AtlasResult<atlas_auth::MsTokens>> + Send + 'static) {
        let login = self.login.clone();
        let accounts = self.account_store();
        let settings_atlas = self.settings_handle();
        self.rt.spawn(async move {
            let result = async {
                let ms = wait.await?;
                let mc = auth.minecraft(&ms).await?;
                accounts.add(&ms, mc)
            }
            .await;
            let mut l = login.lock().unwrap_or_else(|p| p.into_inner());
            if cancel.is_cancelled() {
                return;
            }
            l.cancel = None;
            l.state = match result {
                Ok(acc) => {
                    settings_atlas(acc.id.clone());
                    LoginState::Done { account: acc }
                }
                Err(e) => {
                    tracing::warn!(code = %e.code, "sign-in failed: {}", e.message);
                    LoginState::Failed { error: e }
                }
            };
        });
    }

    /// Opens the Microsoft sign-in page in the browser. Returns the URL too,
    /// in case the browser didn't open.
    pub fn start_browser_login(&self) -> AtlasResult<String> {
        let auth = self.ms_auth()?;
        let begin = self.rt.block_on(auth.begin_browser())?;
        let url = begin.url.clone();
        let cancel = CancellationToken::new();
        {
            let mut l = self.login.lock().unwrap_or_else(|p| p.into_inner());
            l.state = LoginState::Browser { url: url.clone() };
            l.cancel = Some(cancel.clone());
        }
        if let Err(e) = open::that_detached(&url) {
            tracing::warn!("couldn't open a browser: {e}");
        }
        let auth2 = self.ms_auth()?;
        let c = cancel.clone();
        self.finish_login(auth, cancel, async move { auth2.finish_browser(begin, &c).await });
        Ok(url)
    }

    /// Device code sign-in: show the code, the user enters it at the URL.
    pub fn start_device_login(&self) -> AtlasResult<DeviceCode> {
        let auth = self.ms_auth()?;
        let dc = self.rt.block_on(auth.device_code())?;
        let cancel = CancellationToken::new();
        {
            let mut l = self.login.lock().unwrap_or_else(|p| p.into_inner());
            l.state = LoginState::Device { code: dc.user_code.clone(), url: dc.verification_uri.clone() };
            l.cancel = Some(cancel.clone());
        }
        let auth2 = self.ms_auth()?;
        let (c, d) = (cancel.clone(), dc.clone());
        self.finish_login(auth, cancel, async move { auth2.poll_device(&d, &c).await });
        Ok(dc)
    }

    /// Small closure that makes a new account the active one (runs on a worker thread).
    fn settings_handle(&self) -> impl Fn(String) + Send + 'static {
        let path = self.paths().settings_file();
        move |id: String| {
            let store = atlas_config::SettingsStore::new(path.clone());
            if let Ok(mut out) = store.load() {
                out.settings.accounts.active_account = Some(id);
                let _ = store.save(&out.settings);
            }
        }
    }

    /// Resolves to the signed-in account (refreshing it if needed), or demo
    /// mode when nobody is signed in.
    pub(crate) fn launch_auth_future(&self) -> impl Future<Output = AtlasResult<LaunchAuth>> + Send + 'static {
        let active = self.active_account();
        let accounts = self.account_store();
        let auth = self.ms_auth().ok();
        async move {
            let Some(acc) = active else {
                tracing::info!("no account signed in, starting in demo mode");
                return Ok(LaunchAuth::demo());
            };
            let s = accounts.session(&acc.id, auth.as_ref()).await?;
            Ok(LaunchAuth { name: s.name, uuid: s.uuid, access_token: s.token.expose().to_string(), xuid: s.xuid, demo: false })
        }
    }
}

pub(crate) fn memory_store() -> Arc<dyn SecretStore> {
    Arc::new(MemoryStore::default())
}
