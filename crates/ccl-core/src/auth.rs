//! Microsoft device-code login and Minecraft Java token exchange.
use crate::{
    accounts::{AccountError, AccountMetadata, AccountSecrets, SecretStore},
    task::CancellationToken,
};
use reqwest::{Client, Url};
use serde::Deserialize;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MS_SCOPE: &str = "XboxLive.signin offline_access";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

#[derive(Clone)]
pub struct AuthEndpoints {
    pub device_code: Url,
    pub token: Url,
    pub xbox_live: Url,
    pub xsts: Url,
    pub minecraft: Url,
    pub entitlements: Url,
    pub profile: Url,
}
impl Default for AuthEndpoints {
    fn default() -> Self {
        Self {
            device_code: "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode"
                .parse()
                .unwrap(),
            token: "https://login.microsoftonline.com/consumers/oauth2/v2.0/token"
                .parse()
                .unwrap(),
            xbox_live: "https://user.auth.xboxlive.com/user/authenticate"
                .parse()
                .unwrap(),
            xsts: "https://xsts.auth.xboxlive.com/xsts/authorize"
                .parse()
                .unwrap(),
            minecraft: "https://api.minecraftservices.com/authentication/login_with_xbox"
                .parse()
                .unwrap(),
            entitlements: "https://api.minecraftservices.com/entitlements/mcstore"
                .parse()
                .unwrap(),
            profile: "https://api.minecraftservices.com/minecraft/profile"
                .parse()
                .unwrap(),
        }
    }
}
#[derive(Clone)]
pub struct AuthConfig {
    pub client_id: String,
    pub endpoints: AuthEndpoints,
    pub proxy: Option<String>,
}
impl AuthConfig {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            endpoints: AuthEndpoints::default(),
            proxy: None,
        }
    }
}

#[derive(Clone)]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    pub message: String,
    pub expires_in: Duration,
    deadline: Instant,
    pub(crate) code: String,
    pub(crate) interval: Duration,
}
impl std::fmt::Debug for DeviceCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceCode")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}
pub struct AuthenticatedAccount {
    pub metadata: AccountMetadata,
    pub(crate) secrets: AccountSecrets,
}
impl AuthenticatedAccount {
    pub fn persist<S: SecretStore>(
        self,
        store: &crate::accounts::AccountStore<S>,
    ) -> Result<(), AuthError> {
        store.upsert(self.metadata, self.secrets)?;
        Ok(())
    }
    pub fn persist_cancellable<S: SecretStore>(
        self,
        store: &crate::accounts::AccountStore<S>,
        cancel: &CancellationToken,
    ) -> Result<(), AuthError> {
        store.upsert_cancellable(self.metadata, self.secrets, cancel)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("authentication was cancelled")]
    Cancelled,
    #[error("device code expired before sign-in completed")]
    Expired,
    #[error("authentication service rejected the request ({0})")]
    Rejected(String),
    #[error("authentication service returned an invalid response")]
    InvalidResponse,
    #[error("network request failed during authentication")]
    Network,
    #[error("account storage failed: {0}")]
    Account(#[from] AccountError),
    #[error("account identity does not own Minecraft Java")]
    NoEntitlement,
}

pub struct MicrosoftAuth {
    client: Client,
    config: AuthConfig,
}
impl MicrosoftAuth {
    pub fn new(config: AuthConfig) -> Result<Self, AuthError> {
        if config.client_id.trim().is_empty() {
            return Err(AuthError::Rejected(
                "OAuth client ID is not configured".into(),
            ));
        }
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("CialloCraftLauncher/", env!("CARGO_PKG_VERSION")));
        if let Some(proxy) = &config.proxy {
            builder = builder.proxy(
                reqwest::Proxy::all(proxy)
                    .map_err(|_| AuthError::Rejected("configured HTTP proxy is invalid".into()))?,
            );
        }
        Ok(Self {
            client: builder.build().map_err(|_| AuthError::Network)?,
            config,
        })
    }
    pub async fn begin_device_login(&self) -> Result<DeviceCode, AuthError> {
        self.begin_device_login_inner(None).await
    }
    pub async fn begin_device_login_cancellable(
        &self,
        cancel: &CancellationToken,
    ) -> Result<DeviceCode, AuthError> {
        self.begin_device_login_inner(Some(cancel)).await
    }
    async fn begin_device_login_inner(
        &self,
        cancel: Option<&CancellationToken>,
    ) -> Result<DeviceCode, AuthError> {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(AuthError::Cancelled);
        }
        let request = self
            .client
            .post(self.config.endpoints.device_code.clone())
            .form(&[
                ("client_id", self.config.client_id.as_str()),
                ("scope", MS_SCOPE),
            ])
            .send();
        let response = send_request(request, cancel).await?;
        let v = parse_json(response, cancel).await?;
        let d: DeviceResponse =
            serde_json::from_value(v).map_err(|_| AuthError::InvalidResponse)?;
        if d.expires_in == 0
            || d.device_code.is_empty()
            || d.user_code.is_empty()
            || d.expires_in > 86400
            || !valid_verification_uri(&d.verification_uri)
        {
            return Err(AuthError::InvalidResponse);
        }
        Ok(DeviceCode {
            user_code: d.user_code,
            verification_uri: d.verification_uri,
            message: d.message,
            expires_in: Duration::from_secs(d.expires_in),
            deadline: Instant::now() + Duration::from_secs(d.expires_in),
            code: d.device_code,
            interval: Duration::from_secs(d.interval.unwrap_or(5).max(1)),
        })
    }
    pub async fn complete_device_login(
        &self,
        device: DeviceCode,
        cancel: &CancellationToken,
    ) -> Result<AuthenticatedAccount, AuthError> {
        let mut delay = device.interval;
        let oauth = loop {
            if cancel.is_cancelled() {
                return Err(AuthError::Cancelled);
            }
            if Instant::now() >= device.deadline {
                return Err(AuthError::Expired);
            }
            let remaining = device.deadline.saturating_duration_since(Instant::now());
            tokio::select! {_ = tokio::time::sleep(delay.min(remaining))=>{},_ = wait_cancel(cancel)=>return Err(AuthError::Cancelled)}
            if Instant::now() >= device.deadline {
                return Err(AuthError::Expired);
            }
            let form = [
                ("grant_type", DEVICE_GRANT),
                ("client_id", self.config.client_id.as_str()),
                ("device_code", device.code.as_str()),
            ];
            let future = self
                .client
                .post(self.config.endpoints.token.clone())
                .form(&form)
                .send();
            let response = tokio::select! {
                r = future => r.map_err(|_|AuthError::Network)?,
                _ = wait_cancel(cancel) => return Err(AuthError::Cancelled),
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(device.deadline)) => return Err(AuthError::Expired),
            };
            let success = response.status().is_success();
            // OAuth pending/slow_down responses use HTTP 400. Decode their error
            // identifier before treating other unsuccessful statuses as failures.
            let value = tokio::select! {
                value = parse_body(response, Some(cancel)) => value?,
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(device.deadline)) => return Err(AuthError::Expired),
            };
            if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
                match error {
                    "authorization_pending" => continue,
                    "slow_down" => {
                        delay += Duration::from_secs(5);
                        continue;
                    }
                    "expired_token" => return Err(AuthError::Expired),
                    "authorization_declined" => {
                        return Err(AuthError::Rejected("sign-in was declined".into()));
                    }
                    "bad_verification_code" => {
                        return Err(AuthError::Rejected("device code was rejected".into()));
                    }
                    _ => return Err(AuthError::Rejected("Microsoft sign-in failed".into())),
                }
            }
            if !success {
                return Err(AuthError::Rejected("Microsoft sign-in failed".into()));
            }
            break serde_json::from_value::<OAuthResponse>(value)
                .map_err(|_| AuthError::InvalidResponse)?;
        };
        if cancel.is_cancelled() {
            return Err(AuthError::Cancelled);
        }
        self.minecraft_exchange(oauth, self.config.client_id.clone(), Some(cancel))
            .await
    }
    pub async fn refresh<S: SecretStore>(
        &self,
        store: &crate::accounts::AccountStore<S>,
        id: &str,
    ) -> Result<(), AuthError> {
        self.refresh_account(store, id, &CancellationToken::default())
            .await?;
        Ok(())
    }
    /// Refresh credentials and persist the server-rotated refresh token before doing
    /// downstream Xbox/Minecraft exchanges, so a later service failure cannot lose it.
    pub async fn refresh_account<S: SecretStore>(
        &self,
        store: &crate::accounts::AccountStore<S>,
        id: &str,
        cancel: &CancellationToken,
    ) -> Result<AccountMetadata, AuthError> {
        let refresh_lock = acquire_refresh_lock(store, id, cancel).await?;
        self.refresh_account_locked(store, id, cancel, &refresh_lock)
            .await
    }

    /// Rechecks token expiry after acquiring the account lock so concurrent launchers reuse
    /// the first process's refresh result.
    pub async fn refresh_if_expiring<S: SecretStore>(
        &self,
        store: &crate::accounts::AccountStore<S>,
        id: &str,
        cancel: &CancellationToken,
        skew: Duration,
    ) -> Result<AccountMetadata, AuthError> {
        let refresh_lock = acquire_refresh_lock(store, id, cancel).await?;
        let secrets = store.credentials(id)?.ok_or(AuthError::InvalidResponse)?;
        if secrets.expires_at_unix() > now().saturating_add(skew.as_secs()) {
            return store
                .list()?
                .into_iter()
                .find(|a| a.id == id)
                .ok_or(AuthError::InvalidResponse);
        }
        self.refresh_account_locked(store, id, cancel, &refresh_lock)
            .await
    }

    /// Resolve a saved account into an in-memory launch identity, refreshing it if
    /// necessary. Interfaces do not need to read or map credential values themselves.
    pub async fn launch_identity<S: SecretStore>(
        &self,
        store: &crate::accounts::AccountStore<S>,
        id: &str,
        cancel: &CancellationToken,
    ) -> Result<crate::launch_plan::MicrosoftIdentity, AuthError> {
        let metadata = self
            .refresh_if_expiring(store, id, cancel, Duration::from_secs(60))
            .await?;
        let credentials = store.credentials(id)?.ok_or(AuthError::InvalidResponse)?;
        crate::launch_plan::MicrosoftIdentity::new(
            metadata.username,
            metadata.uuid,
            metadata.xuid.unwrap_or_default(),
            metadata.oauth_client_id.ok_or(AuthError::InvalidResponse)?,
            credentials.access_token().to_owned(),
        )
        .map_err(|_| AuthError::InvalidResponse)
    }

    async fn refresh_account_locked<S: SecretStore>(
        &self,
        store: &crate::accounts::AccountStore<S>,
        id: &str,
        cancel: &CancellationToken,
        refresh_lock: &crate::accounts::AccountRefreshLock,
    ) -> Result<AccountMetadata, AuthError> {
        if cancel.is_cancelled() {
            return Err(AuthError::Cancelled);
        }
        let old = store.credentials(id)?.ok_or(AuthError::InvalidResponse)?;
        let mut metadata = store
            .list()?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or(AuthError::InvalidResponse)?;
        let client_id = metadata.oauth_client_id.clone().ok_or_else(|| {
            AuthError::Rejected("account is missing its OAuth client ID; sign in again".into())
        })?;
        let future = self
            .client
            .post(self.config.endpoints.token.clone())
            .form(&[
                ("client_id", client_id.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", old.refresh_token()),
            ])
            .send();
        let response = tokio::select! { r = future => r.map_err(|_|AuthError::Network)?, _ = wait_cancel(cancel) => return Err(AuthError::Cancelled) };
        let value = parse_json(response, Some(cancel)).await?;
        let mut oauth: OAuthResponse =
            serde_json::from_value(value).map_err(|_| AuthError::InvalidResponse)?;
        oauth
            .refresh_token
            .get_or_insert_with(|| old.refresh_token().to_owned());
        // Preserve the currently usable Minecraft token while atomically saving OAuth rotation.
        let refresh_token = oauth
            .refresh_token
            .clone()
            .unwrap_or_else(|| old.refresh_token().to_owned());
        let rotated = AccountSecrets::new(
            old.access_token().to_owned(),
            refresh_token.clone(),
            old.expires_at_unix(),
        );
        oauth.refresh_token = Some(refresh_token);
        store.upsert_with_refresh_lock(metadata.clone(), rotated, refresh_lock)?;
        if cancel.is_cancelled() {
            return Err(AuthError::Cancelled);
        }
        let renewed = self
            .minecraft_exchange(oauth, client_id, Some(cancel))
            .await?;
        if renewed.metadata.id != id {
            return Err(AuthError::Rejected(
                "refreshed account identity changed".into(),
            ));
        }
        metadata = renewed.metadata;
        store.upsert_with_refresh_lock(metadata.clone(), renewed.secrets, refresh_lock)?;
        Ok(metadata)
    }
    async fn minecraft_exchange(
        &self,
        oauth: OAuthResponse,
        oauth_client_id: String,
        cancel: Option<&CancellationToken>,
    ) -> Result<AuthenticatedAccount, AuthError> {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(AuthError::Cancelled);
        }
        let xbox=post_json(&self.client,&self.config.endpoints.xbox_live,serde_json::json!({"Properties":{"AuthMethod":"RPS","SiteName":"user.auth.xboxlive.com","RpsTicket":format!("d={}",oauth.access_token)},"RelyingParty":"http://auth.xboxlive.com","TokenType":"JWT"}),cancel).await?;
        let x_token = str_field(&xbox, "Token")?;
        let uhs = xbox
            .pointer("/DisplayClaims/xui/0/uhs")
            .and_then(|v| v.as_str())
            .ok_or(AuthError::InvalidResponse)?;
        let xsts=post_json(&self.client,&self.config.endpoints.xsts,serde_json::json!({"Properties":{"SandboxId":"RETAIL","UserTokens":[x_token]},"RelyingParty":"rp://api.minecraftservices.com/","TokenType":"JWT"}),cancel).await?;
        let xsts_token = str_field(&xsts, "Token")?;
        if xsts
            .pointer("/DisplayClaims/xui/0/uhs")
            .and_then(|v| v.as_str())
            .is_some_and(|xsts_uhs| xsts_uhs != uhs)
        {
            return Err(AuthError::InvalidResponse);
        }
        let xu = xsts
            .pointer("/DisplayClaims/xui/0/xid")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let mc = post_json(
            &self.client,
            &self.config.endpoints.minecraft,
            serde_json::json!({"identityToken":format!("XBL3.0 x={};{}",uhs,xsts_token)}),
            cancel,
        )
        .await?;
        let mc_token = str_field(&mc, "access_token")?;
        let ent = self
            .client
            .get(self.config.endpoints.entitlements.clone())
            .bearer_auth(mc_token)
            .send();
        let ent = send_request(ent, cancel).await?;
        let ent = parse_json(ent, cancel).await?;
        let owns_java = ent
            .get("items")
            .and_then(|v| v.as_array())
            .is_some_and(|items| {
                items.iter().any(|item| {
                    matches!(
                        item.get("name").and_then(|v| v.as_str()),
                        Some("product_minecraft" | "game_minecraft")
                    )
                })
            });
        if !owns_java {
            return Err(AuthError::NoEntitlement);
        }
        let prof = self
            .client
            .get(self.config.endpoints.profile.clone())
            .bearer_auth(mc_token)
            .send();
        let prof = send_request(prof, cancel).await?;
        let prof = parse_json(prof, cancel).await?;
        let id = str_field(&prof, "id")?.to_owned();
        let username = str_field(&prof, "name")?.to_owned();
        let expires = now().saturating_add(
            mc.get("expires_in")
                .and_then(|v| v.as_u64())
                .unwrap_or(3600),
        );
        Ok(AuthenticatedAccount {
            metadata: AccountMetadata {
                id,
                username,
                uuid: str_field(&prof, "id")?.into(),
                xuid: xu,
                account_type: "microsoft".into(),
                oauth_client_id: Some(oauth_client_id),
                last_used_unix: Some(now()),
            },
            secrets: AccountSecrets::new(
                mc_token.into(),
                oauth.refresh_token.ok_or(AuthError::InvalidResponse)?,
                expires,
            ),
        })
    }
}

#[derive(Deserialize)]
struct DeviceResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    message: String,
    expires_in: u64,
    interval: Option<u64>,
}
#[derive(Deserialize)]
struct OAuthResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}
async fn post_json(
    c: &Client,
    url: &Url,
    body: serde_json::Value,
    cancel: Option<&CancellationToken>,
) -> Result<serde_json::Value, AuthError> {
    let future = c
        .post(url.clone())
        .header("x-xbl-contract-version", "1")
        .json(&body)
        .send();
    let r = send_request(future, cancel).await?;
    parse_json(r, cancel).await
}
async fn send_request<F>(
    future: F,
    cancel: Option<&CancellationToken>,
) -> Result<reqwest::Response, AuthError>
where
    F: std::future::Future<Output = reqwest::Result<reqwest::Response>>,
{
    match cancel {
        Some(token) => {
            tokio::select! { r = future => r.map_err(|_|AuthError::Network), _ = wait_cancel(token) => Err(AuthError::Cancelled) }
        }
        None => future.await.map_err(|_| AuthError::Network),
    }
}
async fn parse_json(
    r: reqwest::Response,
    cancel: Option<&CancellationToken>,
) -> Result<serde_json::Value, AuthError> {
    if !r.status().is_success() {
        let status = r.status();
        return Err(AuthError::Rejected(
            if status.as_u16() == 401 || status.as_u16() == 403 {
                "credentials were refused".into()
            } else {
                "authentication service returned an error".into()
            },
        ));
    }
    parse_body(r, cancel).await
}
async fn parse_body(
    r: reqwest::Response,
    cancel: Option<&CancellationToken>,
) -> Result<serde_json::Value, AuthError> {
    match cancel {
        Some(token) => {
            tokio::select! { result = r.json() => result.map_err(|_|AuthError::InvalidResponse), _ = wait_cancel(token) => Err(AuthError::Cancelled) }
        }
        None => r.json().await.map_err(|_| AuthError::InvalidResponse),
    }
}
fn str_field<'a>(v: &'a serde_json::Value, k: &str) -> Result<&'a str, AuthError> {
    v.get(k)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .ok_or(AuthError::InvalidResponse)
}
fn valid_verification_uri(s: &str) -> bool {
    Url::parse(s).is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some())
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
async fn wait_cancel(token: &CancellationToken) {
    loop {
        if token.is_cancelled() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await
    }
}
async fn acquire_refresh_lock<S: SecretStore>(
    store: &crate::accounts::AccountStore<S>,
    id: &str,
    cancel: &CancellationToken,
) -> Result<crate::accounts::AccountRefreshLock, AuthError> {
    loop {
        if cancel.is_cancelled() {
            return Err(AuthError::Cancelled);
        }
        if let Some(lock) = store.try_lock_refresh(id)? {
            return Ok(lock);
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(50)) => {},
            _ = wait_cancel(cancel) => return Err(AuthError::Cancelled),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn mock_server(replies: Vec<(&'static str, &'static str)>) -> Url {
        mock_server_observed(replies).0
    }
    fn mock_server_observed(
        replies: Vec<(&'static str, &'static str)>,
    ) -> (Url, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = count.clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            for (expected, body) in replies {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 2048];
                loop {
                    let n = stream.read(&mut chunk).unwrap();
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&bytes);
                    if text.contains("\r\n\r\n") {
                        let header_end = text.find("\r\n\r\n").unwrap() + 4;
                        let length = text[..header_end]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= header_end + length {
                            break;
                        }
                    }
                }
                let req = String::from_utf8_lossy(&bytes);
                let (route, body_fragment) = expected.split_once('|').unwrap_or((expected, ""));
                assert!(
                    req.starts_with(route),
                    "unexpected mock request: {}",
                    req.lines().next().unwrap_or("")
                );
                if !body_fragment.is_empty() {
                    assert!(
                        req.contains(body_fragment),
                        "mock request body did not contain expected form data"
                    );
                }
                let (status, payload) = if let Some((status, payload)) =
                    body.split_once('\n').filter(|(s, _)| s.starts_with('!'))
                {
                    (status.trim_start_matches('!'), payload)
                } else {
                    ("200", body)
                };
                write!(stream, "HTTP/1.1 {} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", status, payload.len(), payload).unwrap();
                observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        (Url::parse(&format!("http://{address}/")).unwrap(), count)
    }

    #[tokio::test]
    async fn device_flow_exchanges_tokens_and_returns_profile_without_leaking_auth_errors() {
        let root = mock_server(vec![
            (
                "POST /device HTTP/",
                r#"{"device_code":"opaque-device","user_code":"ABCD-EFGH","verification_uri":"https://microsoft.com/devicelogin","message":"Enter code","expires_in":60,"interval":1}"#,
            ),
            (
                "POST /token HTTP/",
                "!400\n{\"error\":\"authorization_pending\"}",
            ),
            (
                "POST /token HTTP/",
                r#"{"access_token":"ms-access","refresh_token":"ms-refresh","expires_in":3600}"#,
            ),
            (
                "POST /xbox HTTP/",
                r#"{"Token":"xbl-token","DisplayClaims":{"xui":[{"uhs":"hash"}]}}"#,
            ),
            (
                "POST /xsts HTTP/",
                r#"{"Token":"xsts-token","DisplayClaims":{"xui":[{"xid":"123"}]}}"#,
            ),
            (
                "POST /minecraft HTTP/",
                r#"{"access_token":"mc-token","expires_in":1800}"#,
            ),
            (
                "GET /entitlements HTTP/",
                r#"{"items":[{"name":"product_minecraft"}]}"#,
            ),
            ("GET /profile HTTP/", r#"{"id":"abcd1234","name":"Player"}"#),
        ]);
        let endpoints = AuthEndpoints {
            device_code: root.join("device").unwrap(),
            token: root.join("token").unwrap(),
            xbox_live: root.join("xbox").unwrap(),
            xsts: root.join("xsts").unwrap(),
            minecraft: root.join("minecraft").unwrap(),
            entitlements: root.join("entitlements").unwrap(),
            profile: root.join("profile").unwrap(),
        };
        let mut config = AuthConfig::new("test-client");
        config.endpoints = endpoints;
        let auth = MicrosoftAuth::new(config).unwrap();
        let device = auth.begin_device_login().await.unwrap();
        assert_eq!(device.user_code, "ABCD-EFGH");
        assert!(!format!("{device:?}").contains("opaque-device"));
        let account = auth
            .complete_device_login(device, &CancellationToken::default())
            .await
            .unwrap();
        assert_eq!(account.metadata.username, "Player");
        assert_eq!(account.metadata.xuid.as_deref(), Some("123"));
        assert_eq!(account.secrets.access_token(), "mc-token");
        assert_eq!(account.secrets.refresh_token(), "ms-refresh");
        let err = AuthError::Rejected("Microsoft sign-in failed".into());
        assert!(!err.to_string().contains("ms-access"));
    }

    #[test]
    fn verification_url_requires_https_and_host() {
        assert!(valid_verification_uri("https://microsoft.com/devicelogin"));
        assert!(!valid_verification_uri("http://microsoft.com/devicelogin"));
        assert!(!valid_verification_uri("javascript:alert(1)"));
    }

    #[tokio::test]
    async fn device_flow_stops_for_expiry_and_cancellation_before_network_access() {
        let auth = MicrosoftAuth::new(AuthConfig::new("test-client")).unwrap();
        let expired = DeviceCode {
            user_code: "ABCD".into(),
            verification_uri: "https://microsoft.com/devicelogin".into(),
            message: String::new(),
            expires_in: Duration::ZERO,
            deadline: Instant::now() - Duration::from_secs(1),
            code: "private-device-code".into(),
            interval: Duration::from_secs(1),
        };
        assert!(matches!(
            auth.complete_device_login(expired, &CancellationToken::default())
                .await,
            Err(AuthError::Expired)
        ));
        let cancel = CancellationToken::default();
        cancel.cancel();
        let pending = DeviceCode {
            user_code: "ABCD".into(),
            verification_uri: "https://microsoft.com/devicelogin".into(),
            message: String::new(),
            expires_in: Duration::from_secs(60),
            deadline: Instant::now() + Duration::from_secs(60),
            code: "private-device-code".into(),
            interval: Duration::from_secs(1),
        };
        assert!(matches!(
            auth.complete_device_login(pending, &cancel).await,
            Err(AuthError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn refresh_persists_rotation_and_uses_account_client_id_even_when_exchange_fails() {
        let root = mock_server(vec![
            (
                "POST /token HTTP/|client_id=registered-client&grant_type=refresh_token&refresh_token=old-refresh",
                r#"{"access_token":"new-ms-access","refresh_token":"rotated-refresh"}"#,
            ),
            (
                "POST /xbox HTTP/",
                "!403\n{\"error_description\":\"access_token=secret-ms-token\"}",
            ),
        ]);
        let mut config = AuthConfig::new("different-current-client");
        config.endpoints.token = root.join("token").unwrap();
        config.endpoints.xbox_live = root.join("xbox").unwrap();
        let auth = MicrosoftAuth::new(config).unwrap();
        let directory =
            std::env::temp_dir().join(format!("ccl-auth-refresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let secrets = std::sync::Arc::new(crate::accounts::MemorySecretStore::default());
        let store = crate::accounts::AccountStore::new(directory.join("accounts.json"), secrets);
        let metadata = AccountMetadata {
            id: "abcd1234".into(),
            username: "Player".into(),
            uuid: "abcd1234".into(),
            xuid: Some("123".into()),
            account_type: "microsoft".into(),
            oauth_client_id: Some("registered-client".into()),
            last_used_unix: None,
        };
        store
            .upsert(
                metadata,
                AccountSecrets::new("old-mc-token".into(), "old-refresh".into(), 2),
            )
            .unwrap();
        let err = auth
            .refresh_account(&store, "abcd1234", &CancellationToken::default())
            .await
            .unwrap_err();
        assert!(!err.to_string().contains("secret-ms-token"));
        assert_eq!(
            store
                .credentials("abcd1234")
                .unwrap()
                .unwrap()
                .refresh_token(),
            "rotated-refresh"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn slow_down_is_cancellable_during_extended_poll_delay() {
        let (root, observed) = mock_server_observed(vec![
            (
                "POST /device HTTP/",
                r#"{"device_code":"opaque","user_code":"ABCD","verification_uri":"https://microsoft.com/devicelogin","message":"Enter code","expires_in":60,"interval":1}"#,
            ),
            ("POST /token HTTP/", "!400\n{\"error\":\"slow_down\"}"),
        ]);
        let mut config = AuthConfig::new("test-client");
        config.endpoints.device_code = root.join("device").unwrap();
        config.endpoints.token = root.join("token").unwrap();
        let auth = MicrosoftAuth::new(config).unwrap();
        let device = auth.begin_device_login().await.unwrap();
        let cancellation = CancellationToken::default();
        let cancel_clone = cancellation.clone();
        tokio::spawn(async move {
            while observed.load(std::sync::atomic::Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });
        assert!(matches!(
            tokio::time::timeout(
                Duration::from_secs(5),
                auth.complete_device_login(device, &cancellation)
            )
            .await
            .unwrap(),
            Err(AuthError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn rejects_entitlements_for_unrelated_products() {
        let root = mock_server(vec![
            (
                "POST /device HTTP/",
                r#"{"device_code":"opaque","user_code":"ABCD","verification_uri":"https://microsoft.com/devicelogin","message":"Enter code","expires_in":60,"interval":1}"#,
            ),
            (
                "POST /token HTTP/",
                r#"{"access_token":"ms-access","refresh_token":"ms-refresh"}"#,
            ),
            (
                "POST /xbox HTTP/",
                r#"{"Token":"xbl-token","DisplayClaims":{"xui":[{"uhs":"hash"}]}}"#,
            ),
            (
                "POST /xsts HTTP/",
                r#"{"Token":"xsts-token","DisplayClaims":{"xui":[{"uhs":"hash","xid":"123"}]}}"#,
            ),
            (
                "POST /minecraft HTTP/",
                r#"{"access_token":"mc-token","expires_in":1800}"#,
            ),
            (
                "GET /entitlements HTTP/",
                r#"{"items":[{"name":"product_minecraft_bedrock"}]}"#,
            ),
        ]);
        let mut config = AuthConfig::new("test-client");
        config.endpoints = AuthEndpoints {
            device_code: root.join("device").unwrap(),
            token: root.join("token").unwrap(),
            xbox_live: root.join("xbox").unwrap(),
            xsts: root.join("xsts").unwrap(),
            minecraft: root.join("minecraft").unwrap(),
            entitlements: root.join("entitlements").unwrap(),
            profile: root.join("profile").unwrap(),
        };
        let auth = MicrosoftAuth::new(config).unwrap();
        let device = auth.begin_device_login().await.unwrap();
        assert!(matches!(
            auth.complete_device_login(device, &CancellationToken::default())
                .await,
            Err(AuthError::NoEntitlement)
        ));
    }

    #[tokio::test]
    async fn concurrent_refreshes_serialize_and_second_uses_rotated_token() {
        let root = mock_server(vec![
            (
                "POST /token HTTP/|client_id=registered-client&grant_type=refresh_token&refresh_token=old-refresh",
                r#"{"access_token":"ms-a","refresh_token":"rotated-refresh"}"#,
            ),
            ("POST /xbox HTTP/", "!403\n{\"error\":\"stop\"}"),
            (
                "POST /token HTTP/|client_id=registered-client&grant_type=refresh_token&refresh_token=rotated-refresh",
                "!403\n{\"error\":\"stop\"}",
            ),
        ]);
        let mut config = AuthConfig::new("ignored-current-client");
        config.endpoints.token = root.join("token").unwrap();
        config.endpoints.xbox_live = root.join("xbox").unwrap();
        let auth = MicrosoftAuth::new(config).unwrap();
        let dir = std::env::temp_dir().join(format!("ccl-auth-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let secrets = std::sync::Arc::new(crate::accounts::MemorySecretStore::default());
        let store1 = crate::accounts::AccountStore::new(dir.join("accounts.json"), secrets.clone());
        store1
            .upsert(
                AccountMetadata {
                    id: "abcd1234".into(),
                    username: "Player".into(),
                    uuid: "abcd1234".into(),
                    xuid: None,
                    account_type: "microsoft".into(),
                    oauth_client_id: Some("registered-client".into()),
                    last_used_unix: None,
                },
                AccountSecrets::new("old-mc".into(), "old-refresh".into(), 2),
            )
            .unwrap();
        let store2 = crate::accounts::AccountStore::new(dir.join("accounts.json"), secrets.clone());
        let cancel_a = CancellationToken::default();
        let cancel_b = CancellationToken::default();
        let (a, b) = tokio::join!(
            auth.refresh_account(&store1, "abcd1234", &cancel_a),
            auth.refresh_account(&store2, "abcd1234", &cancel_b)
        );
        assert!(a.is_err() && b.is_err());
        assert_eq!(
            store1
                .credentials("abcd1234")
                .unwrap()
                .unwrap()
                .refresh_token(),
            "rotated-refresh"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn refresh_if_expiring_reuses_a_fresh_account_without_http() {
        let auth = MicrosoftAuth::new(AuthConfig::new("client")).unwrap();
        let dir = std::env::temp_dir().join(format!("ccl-auth-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = crate::accounts::AccountStore::new(
            dir.join("accounts.json"),
            std::sync::Arc::new(crate::accounts::MemorySecretStore::default()),
        );
        let metadata = AccountMetadata {
            id: "abcd1234".into(),
            username: "Player".into(),
            uuid: "abcd1234".into(),
            xuid: None,
            account_type: "microsoft".into(),
            oauth_client_id: Some("client".into()),
            last_used_unix: None,
        };
        store
            .upsert(
                metadata.clone(),
                AccountSecrets::new("minecraft-access".into(), "refresh".into(), now() + 3600),
            )
            .unwrap();
        let returned = auth
            .refresh_if_expiring(
                &store,
                "abcd1234",
                &CancellationToken::default(),
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        assert_eq!(returned, metadata);
        assert_eq!(
            store
                .credentials("abcd1234")
                .unwrap()
                .unwrap()
                .access_token(),
            "minecraft-access"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
    #[tokio::test]
    async fn expires_before_first_poll_and_begin_honors_precancellation() {
        let root = mock_server(vec![(
            "POST /device HTTP/",
            r#"{"device_code":"opaque","user_code":"ABCD","verification_uri":"https://microsoft.com/devicelogin","message":"Enter code","expires_in":1,"interval":5}"#,
        )]);
        let mut config = AuthConfig::new("test-client");
        config.endpoints.device_code = root.join("device").unwrap();
        config.endpoints.token = root.join("token").unwrap();
        let auth = MicrosoftAuth::new(config).unwrap();
        let device = auth.begin_device_login().await.unwrap();
        assert!(matches!(
            tokio::time::timeout(
                Duration::from_secs(2),
                auth.complete_device_login(device, &CancellationToken::default())
            )
            .await
            .unwrap(),
            Err(AuthError::Expired)
        ));
        let token = CancellationToken::default();
        token.cancel();
        assert!(matches!(
            auth.begin_device_login_cancellable(&token).await,
            Err(AuthError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn core_prepares_saved_launch_identity_without_exposing_credentials() {
        let directory = std::env::temp_dir().join(format!(
            "ccl-core-identity-{}-{}",
            std::process::id(),
            now()
        ));
        let store = crate::accounts::AccountStore::new(
            directory.join("accounts.json"),
            std::sync::Arc::new(crate::accounts::MemorySecretStore::default()),
        );
        let id = "0123456789abcdef0123456789abcdef";
        store
            .upsert(
                AccountMetadata {
                    id: id.into(),
                    username: "OnlinePlayer".into(),
                    uuid: id.into(),
                    xuid: Some("123456789".into()),
                    account_type: "microsoft".into(),
                    oauth_client_id: Some("registered-client".into()),
                    last_used_unix: None,
                },
                AccountSecrets::new(
                    "private-access-sentinel".into(),
                    "private-refresh-sentinel".into(),
                    now() + 3600,
                ),
            )
            .unwrap();
        let identity = MicrosoftAuth::new(AuthConfig::new("test-client"))
            .unwrap()
            .launch_identity(&store, id, &CancellationToken::default())
            .await
            .unwrap();
        assert_eq!(identity.username, "OnlinePlayer");
        assert_eq!(identity.uuid, id);
        assert_eq!(identity.client_id, "registered-client");
        assert!(!format!("{identity:?}").contains("private-"));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
