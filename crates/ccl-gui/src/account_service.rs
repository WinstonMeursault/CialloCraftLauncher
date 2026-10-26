//! GUI-facing account operations. Only non-secret metadata crosses the UI boundary.
use crate::{GuiError, GuiResult};
use ccl_core::{
    accounts::{AccountMetadata, AccountStore, OsSecretStore, SecretStore},
    auth::{AuthConfig, AuthEndpoints, DeviceCode, MicrosoftAuth},
    config::LauncherConfig,
    launch_plan::MicrosoftIdentity,
    task::CancellationToken,
};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone, Serialize)]
pub struct AccountListing {
    pub accounts: Vec<AccountMetadata>,
    pub selected: Option<String>,
}

/// Safe to return to a frontend. The OAuth device code is intentionally omitted.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceAuthorization {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
}

/// Login flow state stays in Rust and never implements Serialize or Debug.
pub struct DeviceLogin {
    auth: MicrosoftAuth,
    code: DeviceCode,
}

pub struct AccountService<S: SecretStore> {
    store: AccountStore<S>,
    config: LauncherConfig,
    endpoints: Option<AuthEndpoints>,
}

impl<S: SecretStore> AccountService<S> {
    pub fn new(config_path: impl AsRef<Path>, secrets: Arc<S>, config: LauncherConfig) -> Self {
        Self::with_endpoints(config_path, secrets, config, None)
    }

    /// Explicit Rust-side endpoint injection for fixtures and embedding applications.
    /// Frontend commands should use `new`, which always uses production endpoints.
    pub fn with_auth_endpoints(
        config_path: impl AsRef<Path>,
        secrets: Arc<S>,
        config: LauncherConfig,
        endpoints: AuthEndpoints,
    ) -> Self {
        Self::with_endpoints(config_path, secrets, config, Some(endpoints))
    }

    fn with_endpoints(
        config_path: impl AsRef<Path>,
        secrets: Arc<S>,
        config: LauncherConfig,
        endpoints: Option<AuthEndpoints>,
    ) -> Self {
        Self {
            store: AccountStore::new(accounts_path(config_path), secrets),
            config,
            endpoints,
        }
    }

    pub fn list(&self) -> GuiResult<AccountListing> {
        Ok(AccountListing {
            accounts: self.store.list().map_err(account_error)?,
            selected: self.store.selected().map_err(account_error)?,
        })
    }

    pub fn select(&self, id: &str) -> GuiResult<()> {
        self.store.select(id).map_err(account_error)
    }

    /// Persist a local offline identity. This path never reads or writes the credential vault.
    pub fn add_offline(&self, username: &str) -> GuiResult<AccountMetadata> {
        let account = self.store.add_offline(username).map_err(account_error)?;
        self.store.select(&account.id).map_err(account_error)?;
        Ok(account)
    }

    pub fn logout(&self, id: &str) -> GuiResult<()> {
        self.store.remove(id).map_err(account_error)
    }

    pub async fn begin_login(
        &self,
        client_id: Option<String>,
        cancel: &CancellationToken,
    ) -> GuiResult<(DeviceLogin, DeviceAuthorization)> {
        if self.config.downloads.offline {
            return Err(GuiError::new(
                "offline",
                "Sign-in requires network access while offline mode is enabled.",
            ));
        }
        let auth = self.auth(client_id)?;
        let code = auth
            .begin_device_login_cancellable(cancel)
            .await
            .map_err(auth_error)?;
        let authorization = DeviceAuthorization {
            user_code: code.user_code.clone(),
            verification_uri: code.verification_uri.clone(),
            expires_in: code.expires_in.as_secs(),
        };
        Ok((DeviceLogin { auth, code }, authorization))
    }

    pub async fn complete_login(
        &self,
        login: DeviceLogin,
        cancel: &CancellationToken,
    ) -> GuiResult<AccountMetadata> {
        let account = login
            .auth
            .complete_device_login(login.code, cancel)
            .await
            .map_err(auth_error)?;
        let metadata = account.metadata.clone();
        account
            .persist_cancellable(&self.store, cancel)
            .map_err(auth_error)?;
        self.store.select(&metadata.id).map_err(account_error)?;
        Ok(metadata)
    }

    pub async fn refresh(
        &self,
        id: &str,
        cancel: &CancellationToken,
    ) -> GuiResult<AccountMetadata> {
        if self.config.downloads.offline {
            return Err(GuiError::new(
                "offline",
                "Refreshing an account requires network access while offline mode is enabled.",
            ));
        }
        let client_id = self.account_client_id(id)?;
        self.auth(Some(client_id))?
            .refresh_account(&self.store, id, cancel)
            .await
            .map_err(auth_error)
    }

    pub async fn launch_identity(
        &self,
        id: &str,
        cancel: &CancellationToken,
    ) -> GuiResult<MicrosoftIdentity> {
        if self.config.downloads.offline {
            cancel
                .check()
                .map_err(|_| GuiError::new("cancelled", "Account operation was cancelled."))?;
            let metadata = self
                .store
                .list()
                .map_err(account_error)?
                .into_iter()
                .find(|account| account.id == id)
                .ok_or_else(|| {
                    GuiError::new("account_missing", "The selected account is unavailable.")
                })?;
            let credentials = self
                .store
                .credentials(id)
                .map_err(account_error)?
                .ok_or_else(|| {
                    GuiError::new(
                        "account_credentials",
                        "Saved account credentials are unavailable.",
                    )
                })?;
            return MicrosoftIdentity::new(
                metadata.username,
                metadata.uuid,
                metadata.xuid.unwrap_or_default(),
                metadata.oauth_client_id.ok_or_else(|| {
                    GuiError::new("account_metadata", "Saved account metadata is incomplete.")
                })?,
                credentials.access_token().to_owned(),
            )
            .map_err(|_| GuiError::new("account_metadata", "Saved account metadata is invalid."));
        }
        let client_id = self.account_client_id(id)?;
        self.auth(Some(client_id))?
            .launch_identity(&self.store, id, cancel)
            .await
            .map_err(auth_error)
    }

    fn account_client_id(&self, id: &str) -> GuiResult<String> {
        self.store
            .list()
            .map_err(account_error)?
            .into_iter()
            .find(|account| account.id == id)
            .and_then(|account| account.oauth_client_id)
            .ok_or_else(|| {
                GuiError::new(
                    "account_metadata",
                    "Saved account metadata is missing its Microsoft client ID.",
                )
            })
    }

    fn auth(&self, override_client_id: Option<String>) -> GuiResult<MicrosoftAuth> {
        let client_id = override_client_id
            .or_else(|| self.config.auth.microsoft_client_id.clone())
            .ok_or_else(|| {
                GuiError::new(
                    "auth_config",
                    "Set auth.microsoft_client_id before signing in.",
                )
            })?;
        let mut config = AuthConfig::new(client_id);
        config.proxy = self.config.downloads.http_proxy.clone();
        if let Some(endpoints) = &self.endpoints {
            config.endpoints = endpoints.clone();
        }
        MicrosoftAuth::new(config).map_err(auth_error)
    }
}

/// Check native credential-vault availability without reading any user account.
pub fn check_storage(config_path: impl AsRef<Path>) -> GuiResult<()> {
    let secrets =
        OsSecretStore::for_account_file(&accounts_path(config_path)).map_err(account_error)?;
    secrets.probe().map_err(account_error)
}

fn accounts_path(config_path: impl AsRef<Path>) -> PathBuf {
    let path = config_path.as_ref();
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("accounts.json")
}

fn account_error(error: ccl_core::accounts::AccountError) -> GuiError {
    GuiError::new("account_storage", error.to_string())
}
fn auth_error(error: ccl_core::auth::AuthError) -> GuiError {
    let code = match error {
        ccl_core::auth::AuthError::Cancelled => "cancelled",
        ccl_core::auth::AuthError::Expired => "auth_expired",
        ccl_core::auth::AuthError::NoEntitlement => "no_entitlement",
        ccl_core::auth::AuthError::Account(_) => "account_storage",
        _ => "authentication",
    };
    GuiError::new(code, error.to_string())
}
