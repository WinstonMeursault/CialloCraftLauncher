//! Signed desktop package updates. The transaction helper runs before WebView startup.
mod transaction;

use crate::{GuiError, GuiResult};
use ccl_core::{
    CancellationToken,
    config::DownloadConfig,
    update::{BuildIdentity, Channel, REPOSITORY},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, time::Duration};
pub use transaction::{Installation, TransactionStatus, acknowledge_health, helper_entry};

const MAX_METADATA: u64 = 1024 * 1024;
const MAX_PACKAGE: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageAsset {
    pub format: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuiManifest {
    pub schema_version: u32,
    pub repository: String,
    pub product: String,
    pub channel: Channel,
    pub version: String,
    pub commit: String,
    pub branch: String,
    pub target: String,
    pub data_schema_min: u32,
    pub data_schema_max: u32,
    pub os_signed: bool,
    pub asset: PackageAsset,
}
impl GuiManifest {
    pub fn tag(&self) -> String {
        match self.channel {
            Channel::Stable => format!("v{}", self.version),
            Channel::Dev => format!("dev-{}", self.commit),
        }
    }
    pub fn validate(&self, channel: Channel, target: &str, tag: &str) -> GuiResult<()> {
        let (format, suffix) = match target {
            "aarch64-apple-darwin" => ("app.tar.gz", ".app.tar.gz"),
            "x86_64-pc-windows-msvc" => ("nsis", ".exe"),
            "x86_64-unknown-linux-gnu" => ("appimage", ".AppImage"),
            _ => return Err(error("Unsupported desktop update target")),
        };
        let expected_name = format!("ccl-gui-{target}{suffix}");
        let version = semver::Version::parse(&self.version)
            .map_err(|_| error("Invalid GUI update version"))?;
        if self.schema_version != 1
            || self.repository != REPOSITORY
            || self.product != "ccl-gui"
            || self.channel != channel
            || self.target != target
            || self.branch != channel.branch()
            || !is_hex(&self.commit, 40)
            || !is_hex(&self.asset.sha256, 64)
            || self.data_schema_min > 1
            || self.data_schema_max < 1
            || self.data_schema_min > self.data_schema_max
            || self.asset.size == 0
            || self.asset.size > MAX_PACKAGE
            || self.asset.format != format
            || self.asset.name != expected_name
            || self.tag() != tag
            || (channel == Channel::Stable
                && (!version.pre.is_empty() || !version.build.is_empty()))
            || self.asset.url
                != format!(
                    "https://github.com/{REPOSITORY}/releases/download/{tag}/{expected_name}"
                )
        {
            return Err(error(
                "GUI manifest identity, package, channel or data format mismatch",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Status,
    Check { channel: Channel },
    Apply { channel: Channel },
    Rollback,
    Recover,
}
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub available: bool,
    pub build_pending: bool,
    pub branch_commit: String,
    pub candidate: Option<GuiManifest>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub build: BuildIdentity,
    pub selected_channel: Channel,
    pub supported: bool,
    pub reason: Option<String>,
    pub status: Option<TransactionStatus>,
    pub check: Option<Check>,
    pub helper_pid: Option<u32>,
    pub restart_required: bool,
}
pub fn current_identity() -> BuildIdentity {
    BuildIdentity {
        version: env!("CARGO_PKG_VERSION").into(),
        ..BuildIdentity::current()
    }
}

pub struct Service {
    client: reqwest::Client,
    offline: bool,
}
impl Service {
    pub fn new(config: &DownloadConfig) -> GuiResult<Self> {
        let mut builder = reqwest::Client::builder()
            .user_agent("CialloCraftLauncher-desktop-updater")
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() < 5 && trusted_url(attempt.url().as_str()) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }));
        if let Some(proxy) = &config.http_proxy {
            builder = builder
                .proxy(reqwest::Proxy::all(proxy).map_err(|_| error("Invalid updater proxy"))?);
        }
        Ok(Self {
            client: builder.build().map_err(network_error)?,
            offline: config.offline,
        })
    }
    async fn bytes(&self, url: &str, limit: u64, cancel: &CancellationToken) -> GuiResult<Vec<u8>> {
        if self.offline {
            return Err(GuiError::new(
                "network_offline",
                "Offline mode: desktop update requires network access",
            ));
        }
        if !trusted_url(url) {
            return Err(error("Untrusted update URL"));
        }
        cancelled(cancel)?;
        let mut response = tokio::select! { r=self.client.get(url).send()=>r.map_err(network_error)?, _=wait_cancel(cancel)=>return Err(error("Update cancelled")) };
        if matches!(response.status().as_u16(), 403 | 429) {
            return Err(GuiError::new(
                "rate_limited",
                "GitHub rate limit reached; retry later",
            ));
        }
        if !response.status().is_success() {
            return Err(network_error("Update request failed"));
        }
        if response.content_length().is_some_and(|n| n > limit) {
            return Err(error("Update response exceeds size limit"));
        }
        let mut out = Vec::new();
        loop {
            let chunk = tokio::select! {r=response.chunk()=>r.map_err(network_error)?, _=wait_cancel(cancel)=>return Err(error("Update cancelled"))};
            let Some(chunk) = chunk else { break };
            if out.len() as u64 + chunk.len() as u64 > limit {
                return Err(error("Update response exceeds size limit"));
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }
    async fn json(&self, url: &str, cancel: &CancellationToken) -> GuiResult<serde_json::Value> {
        serde_json::from_slice(&self.bytes(url, MAX_METADATA, cancel).await?)
            .map_err(|_| error("Malformed GitHub metadata"))
    }
    async fn reachable(
        &self,
        commit: &str,
        head: &str,
        cancel: &CancellationToken,
    ) -> GuiResult<bool> {
        if commit == head {
            return Ok(true);
        }
        let data = self
            .json(
                &format!("https://api.github.com/repos/{REPOSITORY}/compare/{commit}...{head}"),
                cancel,
            )
            .await?;
        Ok(matches!(
            data["status"].as_str(),
            Some("ahead" | "identical")
        ))
    }
    async fn signed_manifest(
        &self,
        channel: Channel,
        target: &str,
        tag: &str,
        cancel: &CancellationToken,
    ) -> GuiResult<(GuiManifest, Vec<u8>, String)> {
        let url = format!(
            "https://github.com/{REPOSITORY}/releases/download/{tag}/gui-manifest-{target}.json"
        );
        let bytes = self.bytes(&url, MAX_METADATA, cancel).await?;
        let sig = String::from_utf8(self.bytes(&format!("{url}.sig"), 8192, cancel).await?)
            .map_err(|_| error("Invalid signature encoding"))?;
        ccl_core::update::verify_release_signature(&bytes, &sig)
            .map_err(|_| error("Desktop manifest signature verification failed"))?;
        let manifest: GuiManifest = serde_json::from_slice(&bytes)
            .map_err(|_| error("Malformed signed desktop manifest"))?;
        ccl_core::update::verify_release_signature_for_version(&bytes, &sig, &manifest.version)
            .map_err(|_| error("Signed GUI version does not match manifest"))?;
        manifest.validate(channel, target, tag)?;
        Ok((manifest, bytes, sig))
    }
    pub async fn check(&self, channel: Channel, cancel: &CancellationToken) -> GuiResult<Check> {
        let current = current_identity();
        let prefix = format!("https://api.github.com/repos/{REPOSITORY}");
        let head = self
            .json(&format!("{prefix}/commits/{}", channel.branch()), cancel)
            .await?;
        let head = head["sha"]
            .as_str()
            .filter(|s| is_hex(s, 40))
            .ok_or_else(|| error("Invalid branch head"))?
            .to_string();
        let name = format!("gui-manifest-{}.json", current.target);
        let mut releases = Vec::new();
        for page in 1..=10 {
            let data = self
                .json(
                    &format!("{prefix}/releases?per_page=100&page={page}"),
                    cancel,
                )
                .await?;
            let entries = data
                .as_array()
                .ok_or_else(|| error("Invalid releases response"))?;
            releases.extend(entries.iter().cloned());
            if entries.len() < 100 {
                break;
            }
            if page == 10 {
                return Err(error("Release history exceeds updater limit"));
            }
        }
        if channel == Channel::Stable {
            releases.sort_by_key(|r| {
                std::cmp::Reverse(
                    r["tag_name"]
                        .as_str()
                        .and_then(|s| s.strip_prefix('v'))
                        .and_then(|s| semver::Version::parse(s).ok()),
                )
            });
        }
        for release in releases {
            if release["draft"] == true
                || release["prerelease"].as_bool() != Some(channel == Channel::Dev)
            {
                continue;
            }
            let Some(tag) = release["tag_name"].as_str() else {
                continue;
            };
            if !(match channel {
                Channel::Stable => tag
                    .strip_prefix('v')
                    .is_some_and(|v| semver::Version::parse(v).is_ok()),
                Channel::Dev => tag.strip_prefix("dev-").is_some_and(|s| is_hex(s, 40)),
            }) {
                continue;
            }
            if !release["assets"]
                .as_array()
                .is_some_and(|a| a.iter().any(|a| a["name"] == name))
            {
                continue;
            }
            let (manifest, _, _) = self
                .signed_manifest(channel, &current.target, tag, cancel)
                .await?;
            if !self.reachable(&manifest.commit, &head, cancel).await? {
                continue;
            }
            let mut available =
                manifest.commit != current.commit || manifest.channel != current.channel;
            if current.channel == channel {
                if is_hex(&current.commit, 40) {
                    available &= self
                        .reachable(&current.commit, &manifest.commit, cancel)
                        .await?;
                }
                if channel == Channel::Stable {
                    available &= semver::Version::parse(&manifest.version).ok()
                        > semver::Version::parse(&current.version).ok();
                }
            }
            return Ok(Check {
                available,
                build_pending: manifest.commit != head,
                branch_commit: head,
                candidate: Some(manifest),
            });
        }
        Ok(Check {
            available: false,
            build_pending: true,
            branch_commit: head,
            candidate: None,
        })
    }
    pub async fn execute(
        &self,
        request: Request,
        state: PathBuf,
        cancel: CancellationToken,
    ) -> GuiResult<Response> {
        let build = current_identity();
        let install = match Installation::current(&state) {
            Ok(i) => i,
            Err(e) if matches!(request, Request::Status) => {
                return Ok(Response {
                    selected_channel: build.channel,
                    build,
                    supported: false,
                    reason: Some(e.message),
                    status: None,
                    check: None,
                    helper_pid: None,
                    restart_required: false,
                });
            }
            Err(e) => return Err(e),
        };
        let mut response = Response {
            selected_channel: install.channel()?,
            build,
            supported: true,
            reason: None,
            status: install.status()?,
            check: None,
            helper_pid: None,
            restart_required: false,
        };
        match request {
            Request::Status => {}
            Request::Check { channel } => {
                install.set_channel(channel)?;
                response.selected_channel = channel;
                response.check = Some(self.check(channel, &cancel).await?)
            }
            Request::Apply { channel } => {
                install.set_channel(channel)?;
                response.selected_channel = channel;
                let check = self.check(channel, &cancel).await?;
                if !check.available {
                    return Err(error("No desktop update is available"));
                }
                let candidate = check
                    .candidate
                    .ok_or_else(|| error("No desktop update available"))?;
                let (manifest, raw, sig) = self
                    .signed_manifest(channel, &response.build.target, &candidate.tag(), &cancel)
                    .await?;
                if manifest.commit != candidate.commit {
                    return Err(error("Published manifest changed during update"));
                }
                let package = self
                    .bytes(&manifest.asset.url, manifest.asset.size, &cancel)
                    .await?;
                if package.len() as u64 != manifest.asset.size
                    || digest(&package) != manifest.asset.sha256
                {
                    return Err(error("Desktop package SHA-256 or size mismatch"));
                }
                cancelled(&cancel)?;
                install.stage(&manifest, &raw, &sig, &package)?;
                response.helper_pid = Some(install.launch("apply")?);
                response.restart_required = true;
                response.status = install.status()?;
            }
            Request::Rollback | Request::Recover => {
                let operation = if matches!(request, Request::Rollback) {
                    "rollback"
                } else {
                    "recover"
                };
                install.prepare(operation)?;
                response.helper_pid = Some(install.launch(operation)?);
                response.restart_required = true;
                response.status = install.status()?;
            }
        }
        Ok(response)
    }
}
fn trusted_url(raw: &str) -> bool {
    reqwest::Url::parse(raw).is_ok_and(|u| {
        u.scheme() == "https"
            && u.username().is_empty()
            && u.password().is_none()
            && u.port().is_none()
            && matches!(
                u.host_str(),
                Some(
                    "api.github.com"
                        | "github.com"
                        | "objects.githubusercontent.com"
                        | "release-assets.githubusercontent.com"
                        | "github-releases.githubusercontent.com"
                )
            )
    })
}
fn is_hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn error(message: impl Into<String>) -> GuiError {
    GuiError::new("gui_update", message)
}
fn network_error(_: impl std::fmt::Display) -> GuiError {
    GuiError::new(
        "gui_update_network",
        "Desktop update request failed; check connectivity and proxy settings",
    )
}
fn cancelled(c: &CancellationToken) -> GuiResult<()> {
    if c.is_cancelled() {
        Err(error("Desktop update cancelled"))
    } else {
        Ok(())
    }
}
async fn wait_cancel(c: &CancellationToken) {
    while !c.is_cancelled() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod tests;
