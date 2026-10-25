//! Branch-bound, signed GitHub updates and recoverable executable replacement.
use crate::{CancellationToken, config::DownloadConfig};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub const REPOSITORY: &str = "WinstonMeursault/CialloCraftLauncher";
const PRODUCT: &str = "ccl-cli";
const SIGNATURE_SUFFIX: &str = ".sig";
const MAX_SIGNATURE: u64 = 64 * 1024;
const UPDATE_PUBLIC_KEY: &str = include_str!("update-public-key.pub");
#[cfg(test)]
static TEST_SIGNING_KEY: std::sync::OnceLock<minisign::KeyPair> = std::sync::OnceLock::new();

#[cfg(test)]
fn test_signing_keypair() -> &'static minisign::KeyPair {
    TEST_SIGNING_KEY.get_or_init(|| minisign::KeyPair::generate_unencrypted_keypair().unwrap())
}
const MAX_ARCHIVE: u64 = 128 * 1024 * 1024;
const MAX_EXECUTABLE: u64 = 64 * 1024 * 1024;
// Existing local binaries may contain substantial DWARF debug information. This
// larger bound applies only to the already installed image and its backups.
const MAX_LOCAL_EXECUTABLE: u64 = 256 * 1024 * 1024;
const MAX_METADATA: u64 = 2 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("update cancelled")]
    Cancelled,
    #[error("offline mode: update metadata is not fetched")]
    Offline,
    #[error("GitHub rate limit reached; retry later")]
    RateLimited,
    #[error("update network request failed")]
    Network,
    #[error("invalid update: {0}")]
    Invalid(String),
    #[error("update I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("another updater holds the installation lock")]
    Busy,
    #[error("update failed; recovery status: {0}")]
    Recovery(String),
}
type Result<T> = std::result::Result<T, UpdateError>;
fn invalid(message: &str) -> UpdateError {
    UpdateError::Invalid(message.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Dev,
}
impl Channel {
    pub fn branch(self) -> &'static str {
        match self {
            Self::Stable => "main",
            Self::Dev => "dev",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildIdentity {
    pub version: String,
    pub commit: String,
    pub channel: Channel,
    pub target: String,
}
impl BuildIdentity {
    pub fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").into(),
            commit: option_env!("CCL_BUILD_COMMIT")
                .unwrap_or("unpublished")
                .into(),
            channel: if option_env!("CCL_BUILD_CHANNEL") == Some("stable") {
                Channel::Stable
            } else {
                Channel::Dev
            },
            target: env!("CCL_TARGET").into(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAsset {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateManifest {
    pub schema_version: u32,
    pub product: String,
    pub repository: String,
    pub channel: Channel,
    pub version: String,
    pub commit: String,
    pub branch: String,
    pub target: String,
    pub executable_sha256: String,
    pub executable_size: u64,
    pub data_schema_min: u32,
    pub data_schema_max: u32,
    pub asset: UpdateAsset,
}
impl UpdateManifest {
    pub fn identity(&self) -> BuildIdentity {
        BuildIdentity {
            version: self.version.clone(),
            commit: self.commit.clone(),
            channel: self.channel,
            target: self.target.clone(),
        }
    }
    pub fn validate(&self, channel: Channel, target: &str, tag: &str) -> Result<()> {
        if self.schema_version != 1
            || self.product != PRODUCT
            || self.repository != REPOSITORY
            || self.channel != channel
            || self.branch != channel.branch()
            || self.target != target
            || !hex_string(&self.commit, 40)
            || !hex_string(&self.asset.sha256, 64)
            || !hex_string(&self.executable_sha256, 64)
            || self.executable_size == 0
            || self.executable_size > MAX_EXECUTABLE
            || self.asset.size == 0
            || self.asset.size > MAX_ARCHIVE
            || self.data_schema_min > 1
            || self.data_schema_max < 1
            || self.data_schema_min > self.data_schema_max
            || !valid_version(&self.version)
        {
            return Err(invalid(
                "unsupported manifest, branch, identity or data format",
            ));
        }
        let expected_tag = match channel {
            Channel::Stable => format!("v{}", self.version),
            Channel::Dev => format!("dev-{}", self.commit),
        };
        let expected_name = format!("ccl-{target}.zip");
        let expected_url =
            format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{expected_name}");
        if tag != expected_tag || self.asset.name != expected_name || self.asset.url != expected_url
        {
            return Err(invalid(
                "release asset is not bound to the expected repository and tag",
            ));
        }
        Ok(())
    }
}

/// Verify a Tauri-compatible base64-encoded Minisign signature over exact bytes.
/// The public key is compiled into the updater so downloaded configuration cannot
/// replace the trust root.
pub fn verify_release_signature(
    bytes: &[u8],
    signature: &str,
) -> std::result::Result<(), UpdateError> {
    let key = embedded_update_public_key()?;
    verify_release_signature_with_key(bytes, signature, &key).map(|_| ())
}

/// Verify both the exact payload and Tauri's authenticated version field.
pub fn verify_release_signature_for_version(
    bytes: &[u8],
    signature: &str,
    version: &str,
) -> Result<()> {
    let key = embedded_update_public_key()?;
    let signature = verify_release_signature_with_key(bytes, signature, &key)?;
    verify_signed_version(&signature, version)
}

fn verify_signed_version(signature: &minisign::SignatureBox, version: &str) -> Result<()> {
    let comment = signature
        .trusted_comment()
        .map_err(|_| invalid("missing signed app version"))?;
    let mut versions = comment
        .split('\t')
        .filter_map(|field| field.strip_prefix("version:"));
    if comment.contains(['\r', '\n'])
        || versions.next() != Some(version)
        || versions.next().is_some()
    {
        return Err(invalid("signed app version does not match manifest"));
    }
    Ok(())
}

fn embedded_update_public_key() -> Result<minisign::PublicKey> {
    #[cfg(test)]
    {
        Ok(test_signing_keypair().pk.clone())
    }
    #[cfg(not(test))]
    {
        parse_update_public_key(UPDATE_PUBLIC_KEY)
    }
}

fn parse_update_public_key(contents: &str) -> Result<minisign::PublicKey> {
    let decoded = STANDARD
        .decode(contents.trim())
        .map_err(|_| invalid("invalid embedded update verification key"))?;
    let key_text = std::str::from_utf8(&decoded)
        .map_err(|_| invalid("invalid embedded update verification key"))?;
    minisign::PublicKeyBox::from_string(key_text.trim())
        .and_then(|key| key.into_public_key())
        .map_err(|_| invalid("invalid embedded update verification key"))
}

fn verify_release_signature_with_key(
    bytes: &[u8],
    signature: &str,
    key: &minisign::PublicKey,
) -> Result<minisign::SignatureBox> {
    if signature.len() as u64 > MAX_SIGNATURE {
        return Err(invalid("release signature exceeds size limit"));
    }
    let decoded = STANDARD
        .decode(signature.trim())
        .map_err(|_| invalid("malformed release signature"))?;
    let signature =
        std::str::from_utf8(&decoded).map_err(|_| invalid("malformed release signature"))?;
    let signature = minisign::SignatureBox::from_string(signature)
        .map_err(|_| invalid("malformed release signature"))?;
    minisign::verify(
        key,
        &signature,
        std::io::Cursor::new(bytes),
        true,
        false,
        false,
    )
    .map_err(|_| invalid("release signature verification failed"))?;
    Ok(signature)
}

fn verify_manifest_signature(
    bytes: &[u8],
    signature: &str,
    manifest: &UpdateManifest,
) -> Result<()> {
    let key = embedded_update_public_key()?;
    verify_manifest_signature_with_key(bytes, signature, manifest, &key)
}

fn verify_manifest_signature_with_key(
    bytes: &[u8],
    signature: &str,
    manifest: &UpdateManifest,
    key: &minisign::PublicKey,
) -> Result<()> {
    let signature = verify_release_signature_with_key(bytes, signature, key)?;
    verify_signed_version(&signature, &manifest.version)
}
fn hex_string(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn version_key(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.split('.');
    let result = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    if parts.next().is_some() {
        None
    } else {
        Some(result)
    }
}
fn valid_version(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    version_key(value).is_some()
        && parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
        })
}
#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheck {
    pub channel: Channel,
    pub branch_commit: String,
    pub build_pending: bool,
    pub available: bool,
    pub current: BuildIdentity,
    pub candidate: Option<UpdateManifest>,
    pub unsigned: bool,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<ReleaseAsset>,
}
#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

pub struct UpdateService {
    client: reqwest::Client,
    offline: bool,
    api_root: String,
    context: Option<crate::TaskContext>,
    #[cfg(test)]
    fixture_origin: Option<String>,
}
impl UpdateService {
    pub fn new(config: &DownloadConfig) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .user_agent("CialloCraftLauncher-updater")
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                let url = attempt.url();
                if attempt.previous().len() < 5
                    && url.scheme() == "https"
                    && url.host_str().is_some_and(allowed_host)
                {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }));
        if let Some(proxy) = &config.http_proxy {
            builder =
                builder.proxy(reqwest::Proxy::all(proxy).map_err(|_| invalid("invalid proxy"))?);
        }
        Ok(Self {
            client: builder.build().map_err(|_| UpdateError::Network)?,
            offline: config.offline,
            api_root: "https://api.github.com".into(),
            context: None,
            #[cfg(test)]
            fixture_origin: None,
        })
    }
    pub fn with_task_context(mut self, context: crate::TaskContext) -> Self {
        self.context = Some(context);
        self
    }
    fn progress(&self, completed: u64, total: Option<u64>) -> Result<()> {
        if let Some(context) = &self.context {
            context
                .emit(crate::TaskEvent::Progress {
                    task: "launcher_update".into(),
                    completed,
                    total,
                })
                .map_err(|_| UpdateError::Cancelled)?;
        }
        Ok(())
    }
    async fn bytes(&self, url: &str, max: u64, token: &CancellationToken) -> Result<Vec<u8>> {
        if self.offline {
            return Err(UpdateError::Offline);
        }
        #[cfg(test)]
        let rewritten = self.fixture_origin.as_ref().and_then(|origin| {
            url.strip_prefix("https://github.com")
                .map(|suffix| format!("{origin}{suffix}"))
        });
        #[cfg(test)]
        let url = rewritten.as_deref().unwrap_or(url);

        check_cancel(token)?;
        let mut response = tokio::select! {
            result = self.client.get(url).send() => result.map_err(|_| UpdateError::Network)?,
            _ = cancelled(token) => return Err(UpdateError::Cancelled),
        };
        if matches!(response.status().as_u16(), 403 | 429) {
            return Err(UpdateError::RateLimited);
        }
        if !response.status().is_success() {
            return Err(UpdateError::Network);
        }
        if response.content_length().is_some_and(|size| size > max) {
            return Err(invalid("response exceeds size limit"));
        }
        let total = response.content_length();
        self.progress(0, total)?;
        let mut output = Vec::new();
        loop {
            let chunk = tokio::select! {
                result = response.chunk() => result.map_err(|_| UpdateError::Network)?,
                _ = cancelled(token) => return Err(UpdateError::Cancelled),
            };
            let Some(chunk) = chunk else {
                break;
            };
            if output.len() as u64 + chunk.len() as u64 > max {
                return Err(invalid("response exceeds size limit"));
            }
            output.extend_from_slice(&chunk);
            self.progress(output.len() as u64, total)?;
        }
        Ok(output)
    }
    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        token: &CancellationToken,
    ) -> Result<T> {
        serde_json::from_slice(&self.bytes(url, MAX_METADATA, token).await?)
            .map_err(|_| invalid("malformed GitHub metadata"))
    }
    /// Locate only published builds reachable from the selected branch. Branch
    /// heads without an accepted published build are explicitly reported pending.
    pub async fn check(
        &self,
        channel: Channel,
        current: BuildIdentity,
        token: &CancellationToken,
    ) -> Result<UpdateCheck> {
        let prefix = format!("{}/repos/{REPOSITORY}", self.api_root);
        let head: serde_json::Value = self
            .json(&format!("{prefix}/commits/{}", channel.branch()), token)
            .await?;
        let branch_commit = head["sha"]
            .as_str()
            .filter(|s| hex_string(s, 40))
            .ok_or_else(|| invalid("invalid branch head"))?
            .to_owned();
        let mut published = Vec::new();
        for page in 1..=10 {
            let releases: Vec<Release> = self
                .json(
                    &format!("{prefix}/releases?per_page=100&page={page}"),
                    token,
                )
                .await?;
            let count = releases.len();
            published.extend(releases);
            if count < 100 {
                break;
            }
            if page == 10 {
                return Err(invalid(
                    "too many releases; narrow release history before checking",
                ));
            }
        }
        if channel == Channel::Stable {
            published.sort_by_key(|release| {
                std::cmp::Reverse(release.tag_name.strip_prefix('v').and_then(version_key))
            });
        }
        let mut candidate = None;
        for release in published {
            if release.draft
                || (channel == Channel::Stable && release.prerelease)
                || (channel == Channel::Dev && !release.prerelease)
            {
                continue;
            }
            if (channel == Channel::Dev && !release.tag_name.starts_with("dev-"))
                || (channel == Channel::Stable && !release.tag_name.starts_with('v'))
            {
                continue;
            }
            let name = format!("manifest-{}.json", current.target);
            let Some(asset) = release.assets.iter().find(|a| a.name == name) else {
                continue;
            };
            let signature_name = format!("{name}{SIGNATURE_SUFFIX}");
            let Some(signature_asset) = release.assets.iter().find(|a| a.name == signature_name)
            else {
                continue;
            };
            let expected = format!(
                "https://github.com/{REPOSITORY}/releases/download/{}/{name}",
                release.tag_name
            );
            let expected_signature = format!("{expected}{SIGNATURE_SUFFIX}");
            if asset.browser_download_url != expected
                || signature_asset.browser_download_url != expected_signature
            {
                return Err(invalid("manifest URL escaped selected repository"));
            }
            let manifest_bytes = self.bytes(&expected, MAX_METADATA, token).await?;
            let signature_bytes = self
                .bytes(&expected_signature, MAX_SIGNATURE, token)
                .await?;
            let signature = std::str::from_utf8(&signature_bytes)
                .map_err(|_| invalid("malformed release signature"))?;
            let manifest: UpdateManifest = serde_json::from_slice(&manifest_bytes)
                .map_err(|_| invalid("malformed update manifest"))?;
            verify_manifest_signature(&manifest_bytes, signature, &manifest)?;
            manifest.validate(channel, &current.target, &release.tag_name)?;
            if manifest.commit != branch_commit {
                let compare: serde_json::Value = self
                    .json(
                        &format!("{prefix}/compare/{}...{branch_commit}", manifest.commit),
                        token,
                    )
                    .await?;
                if !matches!(compare["status"].as_str(), Some("ahead" | "identical")) {
                    continue;
                }
            }
            candidate = Some(manifest);
            break;
        }
        let build_pending = candidate.as_ref().is_none_or(|m| m.commit != branch_commit);
        let mut available = candidate
            .as_ref()
            .is_some_and(|m| m.commit != current.commit || m.channel != current.channel);
        if let Some(manifest) = &candidate {
            if current.channel == channel
                && hex_string(&current.commit, 40)
                && manifest.commit != current.commit
            {
                let compare: serde_json::Value = self
                    .json(
                        &format!("{prefix}/compare/{}...{}", current.commit, manifest.commit),
                        token,
                    )
                    .await?;
                available &= compare["status"] == "ahead";
            }
            if current.channel == channel && channel == Channel::Stable {
                available &= version_key(&manifest.version) > version_key(&current.version);
            }
        }
        Ok(UpdateCheck {
            channel,
            branch_commit,
            build_pending,
            available,
            current,
            candidate,
            unsigned: false,
        })
    }
    pub async fn stage(
        &self,
        manifest: &UpdateManifest,
        install: &Installation,
        config: &Path,
        game_root: &Path,
        token: &CancellationToken,
    ) -> Result<UpdateStatus> {
        let tag = match manifest.channel {
            Channel::Stable => format!("v{}", manifest.version),
            Channel::Dev => format!("dev-{}", manifest.commit),
        };
        // A GUI or other frontend may update a separately selected CLI install.
        // Bind the manifest to that executable's target, not the frontend build.
        manifest.validate(manifest.channel, &install.identity()?.target, &tag)?;
        let manifest_name = format!("manifest-{}.json", manifest.target);
        let manifest_url =
            format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{manifest_name}");
        let manifest_bytes = self.bytes(&manifest_url, MAX_METADATA, token).await?;
        let downloaded: UpdateManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|_| invalid("malformed update manifest"))?;
        if serde_json::to_value(&downloaded).ok() != serde_json::to_value(manifest).ok() {
            return Err(invalid(
                "staged manifest differs from signed release manifest",
            ));
        }
        let signature_bytes = self
            .bytes(
                &format!("{manifest_url}{SIGNATURE_SUFFIX}"),
                MAX_SIGNATURE,
                token,
            )
            .await?;
        let signature = std::str::from_utf8(&signature_bytes)
            .map_err(|_| invalid("malformed release signature"))?;
        verify_manifest_signature(&manifest_bytes, signature, manifest)?;
        let prefix = format!("{}/repos/{REPOSITORY}", self.api_root);
        let head: serde_json::Value = self
            .json(
                &format!("{prefix}/commits/{}", manifest.channel.branch()),
                token,
            )
            .await?;
        let head = head["sha"]
            .as_str()
            .filter(|sha| hex_string(sha, 40))
            .ok_or_else(|| invalid("invalid branch head"))?;
        if head != manifest.commit {
            let comparison: serde_json::Value = self
                .json(
                    &format!("{prefix}/compare/{}...{head}", manifest.commit),
                    token,
                )
                .await?;
            if !matches!(comparison["status"].as_str(), Some("ahead" | "identical")) {
                return Err(invalid("candidate commit is not on the selected branch"));
            }
        }
        install.ensure_portable()?;
        let report = crate::compatibility::inspect_compatibility(config, game_root);
        if !report.compatible {
            return Err(invalid(
                "existing data cannot be read; inspect compatibility before updating",
            ));
        }
        let _lock = install.lock()?;
        install.ensure_idle()?;
        let bytes = self
            .bytes(&manifest.asset.url, manifest.asset.size, token)
            .await?;
        if bytes.len() as u64 != manifest.asset.size || digest(&bytes) != manifest.asset.sha256 {
            return Err(invalid("archive size or SHA-256 mismatch"));
        }
        check_cancel(token)?;
        let executable = extract_executable(&bytes, manifest)?;
        let staged = install.directory.join("candidate");
        write_new(&staged, &executable)?;
        executable_mode(&staged)?;
        let health = run_health(&staged, &manifest.identity(), config, game_root);
        if health.is_err() {
            let _ = fs::remove_file(&staged);
            health?;
        }
        let old = read_bounded(&install.target, MAX_LOCAL_EXECUTABLE)?;
        let old_identity = executable_identity(&install.target)?;
        let helper = install.helper_for(&digest(&old));
        if helper.exists() {
            verify_file(&helper, &digest(&old))?;
        } else {
            write_new(&helper, &old)?;
        }
        executable_mode(&helper)?;
        let journal = Journal {
            schema: 1,
            state: UpdateState::Staged,
            nonce: nonce()?,
            target: install.target.clone(),
            old_sha256: digest(&old),
            old_identity,
            new_sha256: manifest.executable_sha256.clone(),
            new_identity: manifest.identity(),
            config: std::path::absolute(config)?,
            game_root: std::path::absolute(game_root)?,
            error: None,
        };
        install.save(&journal)?;
        Ok(journal.status())
    }
}
fn allowed_host(host: &str) -> bool {
    matches!(
        host,
        "github.com"
            | "api.github.com"
            | "release-assets.githubusercontent.com"
            | "objects.githubusercontent.com"
    )
}
async fn cancelled(token: &CancellationToken) {
    while !token.is_cancelled() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
fn check_cancel(token: &CancellationToken) -> Result<()> {
    if token.is_cancelled() {
        Err(UpdateError::Cancelled)
    } else {
        Ok(())
    }
}

fn extract_executable(bytes: &[u8], manifest: &UpdateManifest) -> Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|_| invalid("invalid update ZIP"))?;
    if archive.len() > 256 {
        return Err(invalid("too many ZIP entries"));
    }
    let binary = if manifest.target.contains("windows") {
        "ccl.exe"
    } else {
        "ccl"
    };
    let expected = format!("ccl-{}/{binary}", manifest.target);
    let mut result = None;
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| invalid("unreadable ZIP entry"))?;
        let name = entry
            .name()
            .map_err(|_| invalid("invalid ZIP entry name"))?
            .into_owned();
        if entry.enclosed_name().is_none()
            || name.contains('\\')
            || name.split('/').any(|c| c == ".." || c.contains(':'))
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(invalid("unsafe archive path or symlink"));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| invalid("archive size overflow"))?;
        if total > MAX_ARCHIVE * 2 {
            return Err(invalid("expanded archive too large"));
        }
        if name == expected {
            if result.is_some() || entry.size() != manifest.executable_size {
                return Err(invalid("duplicate or incorrectly sized executable"));
            }
            let mut output = Vec::new();
            (&mut entry)
                .take(manifest.executable_size + 1)
                .read_to_end(&mut output)?;
            if output.len() as u64 != manifest.executable_size
                || digest(&output) != manifest.executable_sha256
            {
                return Err(invalid("executable SHA-256 mismatch"));
            }
            result = Some(output);
        }
    }
    result.ok_or_else(|| invalid("archive has no executable for this platform"))
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn nonce() -> Result<String> {
    let mut value = [0u8; 32];
    getrandom::getrandom(&mut value).map_err(|_| invalid("OS randomness unavailable"))?;
    Ok(value.iter().map(|b| format!("{b:02x}")).collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateState {
    Staged,
    Replacing,
    Checking,
    Committed,
    RollingBack,
    RolledBack,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema: u32,
    state: UpdateState,
    nonce: String,
    target: PathBuf,
    old_sha256: String,
    old_identity: BuildIdentity,
    new_sha256: String,
    new_identity: BuildIdentity,
    config: PathBuf,
    game_root: PathBuf,
    error: Option<String>,
}
impl Journal {
    fn status(&self) -> UpdateStatus {
        UpdateStatus {
            state: self.state,
            current: if matches!(self.state, UpdateState::Committed) {
                self.new_identity.clone()
            } else {
                self.old_identity.clone()
            },
            candidate: self.new_identity.clone(),
            error: self.error.clone(),
            unsigned: false,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    pub state: UpdateState,
    pub current: BuildIdentity,
    pub candidate: BuildIdentity,
    pub error: Option<String>,
    pub unsigned: bool,
}
/// One installation; updates never use the selected game's directory as a target.
pub struct Installation {
    target: PathBuf,
    directory: PathBuf,
}
impl Installation {
    pub fn open(target: &Path) -> Result<Self> {
        if fs::symlink_metadata(target)?.file_type().is_symlink() || !target.is_file() {
            return Err(invalid("update target must be a regular executable"));
        }
        let target = target.canonicalize()?;
        let parent = target
            .parent()
            .ok_or_else(|| invalid("target has no parent"))?;
        let directory = parent.join(".ccl-update");
        if directory.exists() && fs::symlink_metadata(&directory)?.file_type().is_symlink() {
            return Err(invalid("update directory cannot be a symlink"));
        }
        Ok(Self { target, directory })
    }
    fn open_recovery(target: &Path) -> Result<Self> {
        if target.exists() {
            return Self::open(target);
        }
        reject_symlink(target)?;
        let parent = target
            .parent()
            .ok_or_else(|| invalid("missing installation parent"))?
            .canonicalize()?;
        let name = target
            .file_name()
            .ok_or_else(|| invalid("missing executable name"))?;
        let directory = parent.join(".ccl-update");
        reject_symlink(&directory)?;
        Ok(Self {
            target: parent.join(name),
            directory,
        })
    }
    pub fn channel(&self) -> Result<Channel> {
        let path = self.directory.join("channel.json");
        if !path.exists() {
            return Ok(BuildIdentity::current().channel);
        }
        serde_json::from_slice(&read_bounded(&path, 128)?)
            .map_err(|_| invalid("invalid saved update channel"))
    }
    /// Read and validate the build identity reported by this installation's executable.
    pub fn identity(&self) -> Result<BuildIdentity> {
        executable_identity(&self.target)
    }
    pub fn set_channel(&self, channel: Channel) -> Result<()> {
        let _lock = self.lock()?;
        let staged = self.directory.join("channel.next");
        remove_regular_if_exists(&staged)?;
        write_new(&staged, &serde_json::to_vec(&channel).unwrap())?;
        publish(&staged, &self.directory.join("channel.json"), None)?;
        sync_dir(&self.directory)
    }
    /// Recover a known installation using a surviving CCL or helper executable.
    /// The journal remains bound to this exact target path.
    pub fn for_recovery(target: &Path) -> Result<Self> {
        Self::open_recovery(target)
    }
    pub fn for_current_process() -> Result<Self> {
        Self::open(&std::env::current_exe()?)
    }
    fn helper_for(&self, hash: &str) -> PathBuf {
        self.directory.join(format!(
            "helper-{hash}{}",
            if cfg!(windows) { ".exe" } else { "" }
        ))
    }
    fn helper(&self) -> Result<PathBuf> {
        Ok(self.helper_for(&self.load()?.old_sha256))
    }
    fn lock(&self) -> Result<File> {
        fs::create_dir_all(&self.directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        }
        let path = self.directory.join("lock");
        reject_symlink(&path)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        file.try_lock_exclusive().map_err(|_| UpdateError::Busy)?;
        Ok(file)
    }
    fn save(&self, journal: &Journal) -> Result<()> {
        let temporary = self.directory.join("transaction.next");
        remove_regular_if_exists(&temporary)?;
        write_new(
            &temporary,
            &serde_json::to_vec_pretty(journal)
                .map_err(|_| invalid("cannot encode update state"))?,
        )?;
        let path = self.directory.join("transaction.json");
        reject_symlink(&path)?;
        publish(&temporary, &path, None)?;
        sync_dir(&self.directory)?;
        Ok(())
    }
    fn load(&self) -> Result<Journal> {
        let path = self.directory.join("transaction.json");
        reject_symlink(&path)?;
        let data = read_bounded(&path, 128 * 1024)?;
        let journal: Journal =
            serde_json::from_slice(&data).map_err(|_| invalid("invalid update journal"))?;
        if journal.schema != 1
            || journal.target != self.target
            || !hex_string(&journal.nonce, 64)
            || !hex_string(&journal.old_sha256, 64)
            || !hex_string(&journal.new_sha256, 64)
        {
            return Err(invalid("update journal does not match installation"));
        }
        Ok(journal)
    }
    fn ensure_portable(&self) -> Result<()> {
        let text = self.target.to_string_lossy().replace('\\', "/");
        if ["/nix/store/", "/Cellar/", "/Caskroom/", "/snap/"]
            .iter()
            .any(|part| text.contains(part))
            || ["/usr/bin/", "/usr/local/Cellar/", "/bin/"]
                .iter()
                .any(|prefix| text.starts_with(prefix))
        {
            return Err(invalid(
                "package-managed installation: update through its package manager",
            ));
        }
        if fs::metadata(self.target.parent().unwrap())?
            .permissions()
            .readonly()
        {
            return Err(invalid(
                "installation is read-only; use a writable portable installation or its package manager",
            ));
        }
        Ok(())
    }
    fn ensure_idle(&self) -> Result<()> {
        if self.directory.join("transaction.json").exists()
            && !matches!(
                self.load()?.state,
                UpdateState::Committed | UpdateState::RolledBack
            )
        {
            return Err(invalid("unfinished update; recover it first"));
        }
        remove_regular_if_exists(&self.directory.join("candidate"))?;
        Ok(())
    }
    pub fn status(&self) -> Result<Option<UpdateStatus>> {
        if !self.directory.join("transaction.json").exists() {
            return Ok(None);
        }
        let _lock = self.lock()?;
        Ok(Some(self.load()?.status()))
    }
    /// Start the validated old executable as an isolated helper. The caller must
    /// close stdin/exit promptly, allowing Windows to release its image handle.
    pub fn launch_helper(&self) -> Result<u32> {
        let _lock = self.lock()?;
        let journal = self.load()?;
        verify_file(&self.helper()?, &journal.old_sha256)?;
        let child = Command::new(self.helper()?)
            .arg("__update-helper")
            .arg(&self.target)
            .arg(&journal.nonce)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(child.id())
    }
    pub fn prepare_rollback(&self) -> Result<UpdateStatus> {
        let _lock = self.lock()?;
        let mut journal = self.load()?;
        if journal.state != UpdateState::Committed {
            return Err(invalid("no committed update to roll back"));
        }
        verify_file(&self.directory.join("previous"), &journal.old_sha256)?;
        let report =
            crate::compatibility::inspect_compatibility(&journal.config, &journal.game_root);
        if !report.compatible {
            return Err(invalid("rollback data compatibility check failed"));
        }
        run_health(
            &self.directory.join("previous"),
            &journal.old_identity,
            &journal.config,
            &journal.game_root,
        )?;
        journal.state = UpdateState::RollingBack;
        journal.error = None;
        // The retained old helper understands this transaction protocol.
        self.save(&journal)?;
        Ok(journal.status())
    }
}
/// Entry point exclusively for a staged helper. It cannot download or select an
/// arbitrary command. Its own image and the transaction nonce must match.
pub fn execute_helper(target: &Path, supplied_nonce: &str) -> Result<UpdateStatus> {
    let install = Installation::open_recovery(target)?;
    let _lock = install.lock()?;
    let mut journal = install.load()?;
    if supplied_nonce != journal.nonce
        || std::env::current_exe()?.canonicalize()? != install.helper()?.canonicalize()?
    {
        return Err(invalid("invalid update helper handoff"));
    }
    verify_file(&install.helper()?, &journal.old_sha256)?;
    if matches!(
        journal.state,
        UpdateState::Committed | UpdateState::RolledBack
    ) {
        return Ok(journal.status());
    }
    if journal.state == UpdateState::Failed {
        restore(&install, &journal)?;
        journal.state = UpdateState::RolledBack;
        install.save(&journal)?;
        return Ok(journal.status());
    }
    let result = execute_transaction(&install, &mut journal);
    if let Err(error) = result {
        journal.error = Some(error.to_string());
        if restore(&install, &journal).is_ok() {
            journal.state = UpdateState::RolledBack;
        } else {
            journal.state = UpdateState::Failed;
        }
        install.save(&journal)?;
        return Err(UpdateError::Recovery(format!("{:?}", journal.state)));
    }
    Ok(journal.status())
}
#[cfg(test)]
thread_local! {static FILESYSTEM_FAULT:std::cell::RefCell<Option<(&'static str,std::io::ErrorKind)>>=const {std::cell::RefCell::new(None)};}
fn filesystem_boundary(_point: &str) -> Result<()> {
    #[cfg(test)]
    FILESYSTEM_FAULT.with(|fault| -> Result<()> {
        if let Some((point, kind)) = *fault.borrow()
            && point == _point
        {
            return Err(std::io::Error::new(kind, "injected filesystem failure").into());
        }
        Ok(())
    })?;
    Ok(())
}
fn execute_transaction(install: &Installation, journal: &mut Journal) -> Result<()> {
    if journal.state == UpdateState::RollingBack {
        restore(install, journal)?;
        journal.state = UpdateState::RolledBack;
        install.save(journal)?;
        return Ok(());
    }
    if journal.state == UpdateState::Staged {
        verify_file(&install.target, &journal.old_sha256)?;
        verify_file(&install.directory.join("candidate"), &journal.new_sha256)?;
        // A copy preserves the old image while candidate publication remains atomic.
        let previous = install.directory.join("previous");
        remove_regular_if_exists(&previous)?;
        filesystem_boundary("backup")?;
        write_new(
            &previous,
            &read_bounded(&install.target, MAX_LOCAL_EXECUTABLE)?,
        )?;
        executable_mode(&previous)?;
        journal.state = UpdateState::Replacing;
        install.save(journal)?;
    }
    if journal.state == UpdateState::Replacing {
        if verify_file(&install.target, &journal.new_sha256).is_err() {
            verify_file(&install.directory.join("candidate"), &journal.new_sha256)?;
            filesystem_boundary("publish")?;
            publish_retry(&install.directory.join("candidate"), &install.target)?;
            filesystem_boundary("after_publish")?;
        }
        journal.state = UpdateState::Checking;
        install.save(journal)?;
    }
    if journal.state == UpdateState::Checking {
        verify_file(&install.target, &journal.new_sha256)?;
        run_health(
            &install.target,
            &journal.new_identity,
            &journal.config,
            &journal.game_root,
        )?;
        filesystem_boundary("commit")?;
        journal.state = UpdateState::Committed;
        install.save(journal)?;
    }
    Ok(())
}
fn restore(install: &Installation, journal: &Journal) -> Result<()> {
    // Before replacement, the original image itself is sufficient recovery.
    if verify_file(&install.target, &journal.old_sha256).is_ok() {
        return Ok(());
    }
    let previous = install.directory.join("previous");
    // A crash before the Staged -> Replacing transition may leave no backup.
    // The helper is a byte-identical, verified copy of the original executable.
    let source = if verify_file(&previous, &journal.old_sha256).is_ok() {
        previous
    } else {
        let helper = install.helper()?;
        verify_file(&helper, &journal.old_sha256)?;
        helper
    };
    let staged = install.directory.join("restore");
    remove_regular_if_exists(&staged)?;
    write_new(&staged, &read_bounded(&source, MAX_LOCAL_EXECUTABLE)?)?;
    executable_mode(&staged)?;
    publish_retry(&staged, &install.target)?;
    run_health(
        &install.target,
        &journal.old_identity,
        &journal.config,
        &journal.game_root,
    )
}
fn run_health(
    executable: &Path,
    identity: &BuildIdentity,
    config: &Path,
    game: &Path,
) -> Result<()> {
    let token = nonce()?;
    let bytes = run_bounded(
        Command::new(executable)
            .arg("__update-health")
            .arg(&token)
            .arg(config)
            .arg(game),
    )
    .map_err(safe_health_error)?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| invalid("invalid health check response"))?;
    if value["nonce"] != token {
        return Err(invalid("health check response nonce mismatch"));
    }
    if value["compatible"] != true {
        return Err(invalid("new executable reports incompatible data"));
    }
    let reported = serde_json::from_value::<BuildIdentity>(value["build"].clone())
        .map_err(|_| invalid("new executable returned an invalid build identity"))?;
    if &reported != identity {
        return Err(invalid(
            "new executable reported a different build identity",
        ));
    }
    Ok(())
}
fn safe_health_error(error: UpdateError) -> UpdateError {
    match error {
        UpdateError::Io(source) => invalid(&format!(
            "health subprocess I/O failed (kind {:?}, OS error {:?})",
            source.kind(),
            source.raw_os_error()
        )),
        other => other,
    }
}
fn executable_identity(executable: &Path) -> Result<BuildIdentity> {
    let data = run_bounded(Command::new(executable).arg("__build-identity"))?;
    serde_json::from_slice(&data).map_err(|_| invalid("build identity unavailable"))
}
fn run_bounded(command: &mut Command) -> Result<Vec<u8>> {
    // File output prevents an unhealthy child filling a pipe and deadlocking.
    let directory = std::env::temp_dir();
    let path = directory.join(format!("ccl-health-{}", nonce()?));
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&path)?;
    let result = (|| {
        let deadline = Instant::now() + Duration::from_secs(10);
        // A newly published executable may briefly be open for writing at its new
        // destination. Linux reports ETXTBSY for exec until that writer closes. Retry
        // only that condition; the executable must still pass the full health check.
        let mut child = loop {
            match command
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .stdout(Stdio::from(file.try_clone()?))
                .spawn()
            {
                Ok(child) => break child,
                Err(error) if executable_text_busy(&error) => {
                    if Instant::now() >= deadline {
                        return Err(invalid(
                            "executable remained write-locked beyond health-check deadline (OS error 26)",
                        ));
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => {
                    return Err(invalid(&format!(
                        "executable subprocess could not start (kind {:?}, OS error {:?})",
                        error.kind(),
                        error.raw_os_error()
                    )));
                }
            }
        };
        loop {
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(invalid("health subprocess failed"));
                }
                break;
            }
            if Instant::now() >= deadline || file.metadata()?.len() > 128 * 1024 {
                let _ = child.kill();
                let _ = child.wait();
                return Err(invalid("health subprocess exceeded limits"));
            }
            thread::sleep(Duration::from_millis(25));
        }
        read_bounded(&path, 128 * 1024)
    })();
    drop(file);
    let _ = fs::remove_file(path);
    result
}
fn executable_text_busy(error: &std::io::Error) -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        error.raw_os_error() == Some(26) // ETXTBSY
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = error;
        false
    }
}
fn verify_file(path: &Path, hash: &str) -> Result<()> {
    reject_symlink(path)?;
    if digest(&read_bounded(path, MAX_LOCAL_EXECUTABLE)?) != hash {
        return Err(invalid("update file SHA-256 mismatch"));
    }
    Ok(())
}
fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>> {
    reject_symlink(path)?;
    let mut file = File::open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > max {
        return Err(invalid("file exceeds limit or is not regular"));
    }
    let mut bytes = Vec::new();
    (&mut file).take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(invalid("file grew beyond limit"));
    }
    Ok(bytes)
}
fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(invalid("update paths cannot be symlinks"))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
fn remove_regular_if_exists(path: &Path) -> Result<()> {
    reject_symlink(path)?;
    if path.exists() {
        if !path.is_file() {
            return Err(invalid("expected regular file"));
        }
        fs::remove_file(path)?;
    }
    Ok(())
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn executable_mode(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn retryable_publication(error: &UpdateError) -> bool {
    matches!(error,UpdateError::Io(source) if source.kind()==std::io::ErrorKind::PermissionDenied || matches!(source.raw_os_error(),Some(5|32|33)))
}
fn publish_retry(from: &Path, to: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match publish(from, to, None) {
            Ok(()) => {
                sync_dir(to.parent().unwrap())?;
                return Ok(());
            }
            Err(e) if cfg!(windows) && retryable_publication(&e) && Instant::now() < deadline => {
                let _ = e;
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e),
        }
    }
}
fn publish(from: &Path, to: &Path, _backup: Option<&Path>) -> Result<()> {
    reject_symlink(from)?;
    reject_symlink(to)?;
    #[cfg(not(windows))]
    fs::rename(from, to)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, ReplaceFileW,
        };
        let source: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        let ok = unsafe {
            if to.exists() {
                ReplaceFileW(
                    target.as_ptr(),
                    source.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            } else {
                MoveFileExW(
                    source.as_ptr(),
                    target.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            }
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        collections::HashMap,
        net::TcpListener,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
    };

    fn signed_bytes(bytes: &[u8], version: &str) -> String {
        let trusted_comment = format!("timestamp:1\tfile:fixture.json\tversion:{version}");
        let signature = minisign::sign(
            None,
            &test_signing_keypair().sk,
            std::io::Cursor::new(bytes),
            Some(&trusted_comment),
            None,
        )
        .unwrap();
        STANDARD.encode(signature.to_string().as_bytes())
    }

    #[cfg(not(windows))]
    fn manifest_signature_routes(manifest: &UpdateManifest) -> HashMap<String, (u16, Vec<u8>)> {
        let tag = match manifest.channel {
            Channel::Stable => format!("v{}", manifest.version),
            Channel::Dev => format!("dev-{}", manifest.commit),
        };
        let name = format!("manifest-{}.json", manifest.target);
        let bytes = serde_json::to_vec(manifest).unwrap();
        let signature = signed_bytes(&bytes, &manifest.version);
        HashMap::from([
            (
                format!("/{REPOSITORY}/releases/download/{tag}/{name}"),
                (200, bytes),
            ),
            (
                format!("/{REPOSITORY}/releases/download/{tag}/{name}{SIGNATURE_SUFFIX}"),
                (200, signature.into_bytes()),
            ),
        ])
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("ccl-update-test-{}", nonce().unwrap()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn manifest(channel: Channel) -> UpdateManifest {
        let commit = "b".repeat(40);
        let target = BuildIdentity::current().target;
        let tag = if channel == Channel::Dev {
            format!("dev-{commit}")
        } else {
            "v0.1.0".into()
        };
        UpdateManifest {
            schema_version: 1,
            product: PRODUCT.into(),
            repository: REPOSITORY.into(),
            channel,
            version: "0.1.0".into(),
            commit,
            branch: channel.branch().into(),
            target: target.clone(),
            executable_sha256: digest(b"new"),
            executable_size: 3,
            data_schema_min: 1,
            data_schema_max: 1,
            asset: UpdateAsset {
                name: format!("ccl-{target}.zip"),
                size: 3,
                sha256: digest(b"zip"),
                url: format!(
                    "https://github.com/{REPOSITORY}/releases/download/{tag}/ccl-{target}.zip"
                ),
            },
        }
    }
    struct Server {
        origin: String,
        stop: Arc<AtomicBool>,
        requests: Arc<Mutex<Vec<String>>>,
        thread: Option<thread::JoinHandle<()>>,
    }
    impl Server {
        fn new(routes: HashMap<String, (u16, Vec<u8>)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let received = requests.clone();
            let worker = thread::spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    if let Ok((mut stream, _)) = listener.accept() {
                        // Accepted sockets do not inherit the listener mode
                        // consistently across platforms. Ensure header reads
                        // block instead of treating an early WouldBlock as EOF.
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(10)))
                            .unwrap();
                        let mut request = Vec::new();
                        let mut chunk = [0u8; 1024];
                        while request.len() < 16 * 1024
                            && !request.windows(4).any(|bytes| bytes == b"\r\n\r\n")
                        {
                            match stream.read(&mut chunk) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => request.extend_from_slice(&chunk[..n]),
                            }
                        }
                        if !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                            received
                                .lock()
                                .unwrap()
                                .push(format!("incomplete request ({} bytes)", request.len()));
                            // Don't turn a disconnected or incomplete request into
                            // an artificial 404 response for the root path.
                            continue;
                        }
                        let request = String::from_utf8_lossy(&request);
                        let path = request.split_whitespace().nth(1).unwrap_or("/");
                        received.lock().unwrap().push(path.to_owned());
                        let (status, bytes) =
                            routes.get(path).cloned().unwrap_or((404, Vec::new()));
                        let _ = write!(
                            stream,
                            "HTTP/1.1 {status} Status\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            bytes.len()
                        );
                        let _ = stream.write_all(&bytes);
                    } else {
                        thread::sleep(Duration::from_millis(5));
                    }
                }
            });
            Self {
                origin,
                stop,
                requests,
                thread: Some(worker),
            }
        }
        fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
        fn service(&self) -> UpdateService {
            let mut service = UpdateService::new(&DownloadConfig::default()).unwrap();
            // Reqwest reads proxy variables from the environment. A loopback
            // fixture must connect directly regardless of runner NO_PROXY rules.
            service.client = reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap();
            service.api_root = self.origin.clone();
            service.fixture_origin = Some(self.origin.clone());
            service
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self.thread.take() {
                worker.join().unwrap();
            }
        }
    }
    fn routes(channel: Channel, head: &str) -> HashMap<String, (u16, Vec<u8>)> {
        let manifest = manifest(channel);
        let tag = if channel == Channel::Dev {
            format!("dev-{}", manifest.commit)
        } else {
            format!("v{}", manifest.version)
        };
        let name = format!("manifest-{}.json", manifest.target);
        let url = format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{name}");
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let signature = signed_bytes(&manifest_bytes, &manifest.version);
        let signature_url = format!("{url}{SIGNATURE_SUFFIX}");
        HashMap::from([
            (format!("/repos/{REPOSITORY}/commits/{}",channel.branch()),(200,serde_json::to_vec(&json!({"sha":head})).unwrap())),
            (format!("/repos/{REPOSITORY}/releases?per_page=100&page=1"),(200,serde_json::to_vec(&json!([{"tag_name":tag,"draft":false,"prerelease":channel==Channel::Dev,"assets":[{"name":name,"browser_download_url":url},{"name":format!("{name}{SIGNATURE_SUFFIX}"),"browser_download_url":signature_url}]}])).unwrap())),
            (format!("/{REPOSITORY}/releases/download/{tag}/{name}"),(200,manifest_bytes)),
            (format!("/{REPOSITORY}/releases/download/{tag}/{name}{SIGNATURE_SUFFIX}"),(200,signature.into_bytes())),
            (format!("/repos/{REPOSITORY}/compare/{}...{head}",manifest.commit),(200,serde_json::to_vec(&json!({"status":"ahead"})).unwrap())),
        ])
    }
    #[test]
    fn rejects_wrong_repository_branch_target_hash_and_version() {
        let good = manifest(Channel::Dev);
        let tag = format!("dev-{}", good.commit);
        good.validate(Channel::Dev, &good.target, &tag).unwrap();
        let mut variants = vec![];
        let mut bad = good.clone();
        bad.repository = "someone/else".into();
        variants.push(bad);
        let mut bad = good.clone();
        bad.branch = "main".into();
        variants.push(bad);
        let mut bad = good.clone();
        bad.asset.url = "https://example.com/binary".into();
        variants.push(bad);
        let mut bad = good.clone();
        bad.executable_sha256 = "z".repeat(64);
        variants.push(bad);
        let mut bad = good.clone();
        bad.version = "../evil".into();
        variants.push(bad);
        let mut bad = good.clone();
        bad.data_schema_min = 2;
        variants.push(bad);
        for bad in variants {
            assert!(bad.validate(Channel::Dev, &good.target, &tag).is_err())
        }
        assert!(good.validate(Channel::Stable, &good.target, &tag).is_err());
        assert!(good.validate(Channel::Dev, "other-target", &tag).is_err());
    }

    #[test]
    fn signed_manifest_rejects_corruption_and_version_mismatch() {
        let manifest = manifest(Channel::Dev);
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let signature = signed_bytes(&bytes, &manifest.version);
        let key = test_signing_keypair().pk.clone();
        verify_manifest_signature_with_key(&bytes, &signature, &manifest, &key).unwrap();

        let mut corrupted = bytes.clone();
        corrupted[0] ^= 1;
        assert!(
            verify_manifest_signature_with_key(&corrupted, &signature, &manifest, &key).is_err()
        );

        let wrong_version_signature = signed_bytes(&bytes, "9.9.9");
        assert!(
            verify_manifest_signature_with_key(&bytes, &wrong_version_signature, &manifest, &key)
                .is_err()
        );
    }

    #[test]
    fn embedded_public_key_uses_tauri_base64_export_format() {
        let key = parse_update_public_key(UPDATE_PUBLIC_KEY)
            .expect("tracked Tauri-exported update public key should parse");
        assert!(!key.keynum().is_empty());
    }

    #[tokio::test]
    async fn offline_and_cancellation_stop_before_http() {
        let service = UpdateService::new(&DownloadConfig {
            offline: true,
            ..Default::default()
        })
        .unwrap();
        assert!(matches!(
            service
                .check(
                    Channel::Dev,
                    BuildIdentity::current(),
                    &CancellationToken::default()
                )
                .await,
            Err(UpdateError::Offline)
        ));
        let token = CancellationToken::default();
        token.cancel();
        assert!(matches!(
            UpdateService::new(&Default::default())
                .unwrap()
                .check(Channel::Dev, BuildIdentity::current(), &token)
                .await,
            Err(UpdateError::Cancelled)
        ));
    }
    #[tokio::test]
    async fn selects_published_development_commit_and_reports_pending_head() {
        let server = Server::new(routes(Channel::Dev, &"c".repeat(40)));
        let current = BuildIdentity {
            commit: "unpublished".into(),
            ..BuildIdentity::current()
        };
        let result = server
            .service()
            .check(Channel::Dev, current, &CancellationToken::default())
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "update check failed: {error:?}; fixture requests: {:?}",
                    server.requests()
                )
            });
        assert!(result.build_pending && result.available);
        assert_eq!(result.candidate.unwrap().commit, "b".repeat(40));
    }
    #[tokio::test]
    async fn no_automatic_downgrade_to_an_older_published_build() {
        let mut routes = routes(Channel::Dev, &"c".repeat(40));
        routes.insert(
            format!(
                "/repos/{REPOSITORY}/compare/{}...{}",
                "c".repeat(40),
                "b".repeat(40)
            ),
            (200, br#"{"status":"behind"}"#.to_vec()),
        );
        let server = Server::new(routes);
        let current = BuildIdentity {
            commit: "c".repeat(40),
            channel: Channel::Dev,
            ..BuildIdentity::current()
        };
        let result = server
            .service()
            .check(Channel::Dev, current, &CancellationToken::default())
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "update check failed: {error:?}; fixture requests: {:?}",
                    server.requests()
                )
            });
        assert!(!result.available);
        assert!(result.build_pending);
    }
    #[tokio::test]
    async fn rate_limit_is_distinct_from_no_update() {
        let server = Server::new(HashMap::from([(
            format!("/repos/{REPOSITORY}/commits/dev"),
            (429, b"rate limited".to_vec()),
        )]));
        let result = server
            .service()
            .check(
                Channel::Dev,
                BuildIdentity::current(),
                &CancellationToken::default(),
            )
            .await;
        assert!(
            matches!(result, Err(UpdateError::RateLimited)),
            "expected a rate limit response, got {result:?}; fixture requests: {:?}",
            server.requests()
        );
    }
    fn zip_files(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, data) in files {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
    #[test]
    fn archive_rejects_traversal_and_corrupt_executable() {
        let mut manifest = manifest(Channel::Dev);
        let binary = if cfg!(windows) { "ccl.exe" } else { "ccl" };
        let name = format!("ccl-{}/{binary}", manifest.target);
        let data = zip_files(&[(&name, b"new")]);
        assert_eq!(extract_executable(&data, &manifest).unwrap(), b"new");
        let bad = zip_files(&[("../escape", b"bad"), (&name, b"new")]);
        assert!(extract_executable(&bad, &manifest).is_err());
        manifest.executable_sha256 = digest(b"old");
        assert!(extract_executable(&data, &manifest).is_err());
    }
    #[test]
    fn update_lock_prevents_parallel_mutation_and_channel_is_persistent() {
        let temp = Temp::new();
        let target = temp.0.join("ccl");
        fs::write(&target, b"test").unwrap();
        let install = Installation::open(&target).unwrap();
        install.set_channel(Channel::Stable).unwrap();
        assert_eq!(install.channel().unwrap(), Channel::Stable);
        let _lock = install.lock().unwrap();
        assert!(matches!(
            install.set_channel(Channel::Dev),
            Err(UpdateError::Busy)
        ));
    }
    #[cfg(unix)]
    fn shell(identity: &BuildIdentity, healthy: bool) -> Vec<u8> {
        let payload = serde_json::to_string(identity).unwrap();
        format!("#!/bin/sh\nif [ \"$1\" = __build-identity ]; then printf '%s\\n' '{payload}'; exit 0; fi\nif [ \"$1\" = __update-health ]; then printf '{{\"nonce\":\"%s\",\"build\":{payload},\"compatible\":{healthy}}}\\n' \"$2\"; exit 0; fi\nexit 1\n").into_bytes()
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn health_subprocess_retries_a_temporarily_write_locked_executable() {
        let temp = Temp::new();
        let identity = BuildIdentity::current();
        let executable = temp.0.join("ccl");
        fs::write(&executable, shell(&identity, true)).unwrap();
        executable_mode(&executable).unwrap();
        let writer = OpenOptions::new().write(true).open(&executable).unwrap();
        let error = match Command::new(&executable).arg("__build-identity").spawn() {
            Ok(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Linux executed a write-locked executable instead of returning ETXTBSY");
            }
            Err(error) => error,
        };
        assert_eq!(error.raw_os_error(), Some(26));

        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        let executable_for_worker = executable.clone();
        let worker = thread::spawn(move || {
            let _ = started_tx.send(());
            let result = run_bounded(Command::new(executable_for_worker).arg("__build-identity"))
                .and_then(|output| {
                    serde_json::from_slice::<BuildIdentity>(&output)
                        .map_err(|_| invalid("health fixture returned invalid identity"))
                });
            let _ = result_tx.send(result);
        });
        started_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("health worker did not start");
        assert!(matches!(
            result_rx.recv_timeout(Duration::from_millis(500)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));

        drop(writer);
        let reported = result_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("health worker did not finish after the writer closed")
            .unwrap();
        worker.join().unwrap();
        assert_eq!(reported, identity);
    }
    #[cfg(unix)]
    fn transaction() -> (Temp, Installation, Journal) {
        let temp = Temp::new();
        let target = temp.0.join("ccl");
        let old_identity = BuildIdentity {
            commit: "a".repeat(40),
            ..BuildIdentity::current()
        };
        let new_identity = BuildIdentity {
            commit: "b".repeat(40),
            ..old_identity.clone()
        };
        let old = shell(&old_identity, true);
        let new = shell(&new_identity, true);
        fs::write(&target, &old).unwrap();
        executable_mode(&target).unwrap();
        let install = Installation::open(&target).unwrap();
        let lock = install.lock().unwrap();
        write_new(&install.directory.join("candidate"), &new).unwrap();
        executable_mode(&install.directory.join("candidate")).unwrap();
        let journal = Journal {
            schema: 1,
            state: UpdateState::Staged,
            nonce: nonce().unwrap(),
            target: install.target.clone(),
            old_sha256: digest(&old),
            old_identity,
            new_sha256: digest(&new),
            new_identity,
            config: temp.0.join("config.json"),
            game_root: temp.0.join("game"),
            error: None,
        };
        install.save(&journal).unwrap();
        drop(lock);
        (temp, install, journal)
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn stages_verified_network_archive_without_mutating_target_then_commits() {
        let temp = Temp::new();
        let old_identity = BuildIdentity {
            commit: "a".repeat(40),
            ..BuildIdentity::current()
        };
        let old = shell(&old_identity, true);
        let target = temp.0.join("ccl");
        fs::write(&target, &old).unwrap();
        executable_mode(&target).unwrap();
        let install = Installation::open(&target).unwrap();
        let mut manifest = manifest(Channel::Dev);
        let new = shell(&manifest.identity(), true);
        let name = format!("ccl-{}/ccl", manifest.target);
        let archive = zip_files(&[(&name, &new)]);
        manifest.executable_size = new.len() as u64;
        manifest.executable_sha256 = digest(&new);
        manifest.asset.size = archive.len() as u64;
        manifest.asset.sha256 = digest(&archive);
        let route = manifest
            .asset
            .url
            .strip_prefix("https://github.com")
            .unwrap()
            .to_owned();
        let mut stage_routes = manifest_signature_routes(&manifest);
        stage_routes.insert(route, (200, archive));
        stage_routes.insert(
            format!("/repos/{REPOSITORY}/commits/dev"),
            (
                200,
                serde_json::to_vec(&json!({"sha":manifest.commit})).unwrap(),
            ),
        );
        let server = Server::new(stage_routes);
        let config = temp.0.join("config.json");
        let game = temp.0.join("game");
        let status = server
            .service()
            .stage(
                &manifest,
                &install,
                &config,
                &game,
                &CancellationToken::default(),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "update staging failed: {error:?}; fixture requests: {:?}",
                    server.requests()
                )
            });
        assert_eq!(status.state, UpdateState::Staged);
        assert_eq!(fs::read(&target).unwrap(), old);
        let mut journal = install.load().unwrap();
        execute_transaction(&install, &mut journal).unwrap();
        assert_eq!(journal.state, UpdateState::Committed);
        assert_eq!(fs::read(&target).unwrap(), new);
        install.prepare_rollback().unwrap();
        let mut journal = install.load().unwrap();
        execute_transaction(&install, &mut journal).unwrap();
        assert_eq!(fs::read(&target).unwrap(), old);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn corrupt_archive_never_replaces_existing_target() {
        let (_temp, install, mut journal) = transaction();
        // Finish initial fixture so this call may stage another transaction.
        execute_transaction(&install, &mut journal).unwrap();
        let before = fs::read(&install.target).unwrap();
        let manifest = manifest(Channel::Dev);
        let path = manifest
            .asset
            .url
            .strip_prefix("https://github.com")
            .unwrap()
            .to_owned();
        let mut stage_routes = manifest_signature_routes(&manifest);
        stage_routes.insert(path, (200, b"bad".to_vec()));
        stage_routes.insert(
            format!("/repos/{REPOSITORY}/commits/dev"),
            (
                200,
                serde_json::to_vec(&json!({"sha":manifest.commit})).unwrap(),
            ),
        );
        let server = Server::new(stage_routes);
        assert!(
            server
                .service()
                .stage(
                    &manifest,
                    &install,
                    &journal.config,
                    &journal.game_root,
                    &CancellationToken::default()
                )
                .await
                .is_err()
        );
        assert_eq!(fs::read(&install.target).unwrap(), before);
    }
    #[cfg(unix)]
    #[test]
    fn interrupted_publication_recovers_when_candidate_already_moved() {
        let (_temp, install, mut journal) = transaction();
        let original = fs::read(&install.target).unwrap();
        write_new(&install.directory.join("previous"), &original).unwrap();
        publish(&install.directory.join("candidate"), &install.target, None).unwrap();
        journal.state = UpdateState::Replacing;
        install.save(&journal).unwrap();
        execute_transaction(&install, &mut journal).unwrap();
        assert_eq!(journal.state, UpdateState::Committed);
        assert_eq!(
            digest(&fs::read(&install.target).unwrap()),
            journal.new_sha256
        );
        journal.state = UpdateState::RollingBack;
        execute_transaction(&install, &mut journal).unwrap();
        assert_eq!(journal.state, UpdateState::RolledBack);
        assert_eq!(fs::read(&install.target).unwrap(), original);
    }
    #[cfg(unix)]
    #[test]
    fn failed_health_check_restores_previous_program() {
        let (_temp, install, mut journal) = transaction();
        let original = fs::read(&install.target).unwrap();
        let bad = shell(&journal.new_identity, false);
        fs::write(install.directory.join("candidate"), &bad).unwrap();
        journal.new_sha256 = digest(&bad);
        assert!(matches!(
            execute_transaction(&install, &mut journal),
            Err(UpdateError::Invalid(message)) if message.contains("incompatible data")
        ));
        restore(&install, &journal).unwrap();
        assert_eq!(fs::read(&install.target).unwrap(), original);
    }
    #[cfg(unix)]
    #[test]
    fn rollback_refuses_when_retained_program_reports_data_incompatible() {
        let (_temp, install, mut journal) = transaction();
        // Model a newer data format that only the candidate reader supports.
        let previous = shell(&journal.old_identity, false);
        fs::write(&install.target, &previous).unwrap();
        journal.old_sha256 = digest(&previous);
        execute_transaction(&install, &mut journal).unwrap();
        assert!(install.prepare_rollback().is_err());
        assert_eq!(install.load().unwrap().state, UpdateState::Committed);
        verify_file(&install.target, &journal.new_sha256).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn storage_and_permission_failures_preserve_a_runnable_old_program() {
        for point in ["backup", "publish", "after_publish", "commit"] {
            for kind in [
                std::io::ErrorKind::StorageFull,
                std::io::ErrorKind::PermissionDenied,
            ] {
                let (_temp, install, mut journal) = transaction();
                FILESYSTEM_FAULT.with(|fault| *fault.borrow_mut() = Some((point, kind)));
                let failure = execute_transaction(&install, &mut journal);
                FILESYSTEM_FAULT.with(|fault| *fault.borrow_mut() = None);
                assert!(
                    matches!(failure,Err(UpdateError::Io(ref error)) if error.kind()==kind),
                    "boundary {point}"
                );
                restore(&install, &journal).unwrap();
                verify_file(&install.target, &journal.old_sha256).unwrap();
                run_health(
                    &install.target,
                    &journal.old_identity,
                    &journal.config,
                    &journal.game_root,
                )
                .unwrap();
            }
        }
    }
    #[cfg(unix)]
    #[test]
    fn restart_after_publication_before_commit_does_not_reapply_or_lose_backup() {
        for point in ["after_publish", "commit"] {
            let (_temp, install, mut journal) = transaction();
            FILESYSTEM_FAULT
                .with(|fault| *fault.borrow_mut() = Some((point, std::io::ErrorKind::StorageFull)));
            assert!(execute_transaction(&install, &mut journal).is_err());
            FILESYSTEM_FAULT.with(|fault| *fault.borrow_mut() = None);
            let mut recovered = install.load().unwrap();
            execute_transaction(&install, &mut recovered).unwrap();
            assert_eq!(recovered.state, UpdateState::Committed);
            verify_file(&install.target, &journal.new_sha256).unwrap();
            verify_file(&install.directory.join("previous"), &journal.old_sha256).unwrap();
        }
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_update_directory_and_files() {
        use std::os::unix::fs::symlink;
        let temp = Temp::new();
        let target = temp.0.join("ccl");
        fs::write(&target, b"old").unwrap();
        let outside = temp.0.join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, temp.0.join(".ccl-update")).unwrap();
        assert!(Installation::open(&target).is_err());
        let link = temp.0.join("alias");
        symlink(&target, &link).unwrap();
        assert!(Installation::open(&link).is_err());
    }
    #[cfg(unix)]
    #[test]
    #[ignore = "opt-in deterministic update/rollback performance measurement"]
    fn benchmark_update_transactions() {
        let mut samples = Vec::new();
        let mut bytes = 0;
        for _ in 0..7 {
            let (_temp, install, mut journal) = transaction();
            bytes = fs::metadata(&install.target).unwrap().len();
            let start = Instant::now();
            execute_transaction(&install, &mut journal).unwrap();
            journal.state = UpdateState::RollingBack;
            execute_transaction(&install, &mut journal).unwrap();
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "CCL_UPDATE_BENCHMARK={}",
            json!({"metric":"verified_update_and_rollback_ms","median_ms":samples[3],"samples_ms":samples,"executable_bytes":bytes,"fixture":"deterministic shell health protocol; real state machine and disk publication"})
        );
    }
}
