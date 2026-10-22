//! Mod loader installation services.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    TaskContext,
    config::DownloadConfig,
    download::{DownloadError, DownloadManager, DownloadRequest},
    install::{InstallError, InstallOptions, InstallService},
    metadata::{self, VersionMetadata},
    network::HttpClient,
    source::{DownloadKind, url_candidates},
};

const FABRIC_META: &str = "https://meta.fabricmc.net";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    Fabric,
    Forge,
    NeoForge,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LoaderRequest {
    pub kind: LoaderKind,
    pub minecraft: String,
    /// Loader release version, e.g. `0.16.10` for Fabric.
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LoaderVersion {
    pub version: String,
    pub stable: bool,
}

#[derive(Debug, Error)]
pub enum LoaderError {
    #[error(transparent)]
    Install(#[from] InstallError),
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error(transparent)]
    Network(#[from] reqwest::Error),
    #[error("file operation failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Metadata(#[from] metadata::MetadataError),
    #[error("unsupported loader/version: {0}")]
    Unsupported(String),
    #[error("loader installation conflict: {0}")]
    Conflict(String),
    #[error("loader installation cancelled")]
    Cancelled,
}

#[derive(Clone)]
pub struct LoaderService {
    config: DownloadConfig,
    http: HttpClient,
    fabric_meta_url: String,
    manifest_url: String,
}

impl LoaderService {
    pub fn new(config: DownloadConfig) -> Result<Self, LoaderError> {
        Ok(Self {
            http: HttpClient::new(&config).map_err(core_network_error)?,
            config,
            fabric_meta_url: FABRIC_META.into(),
            manifest_url: crate::install::VERSION_MANIFEST_URL.into(),
        })
    }

    /// Override metadata endpoints for private mirrors and local service fixtures.
    pub fn with_fabric_endpoints(
        mut self,
        meta_url: impl Into<String>,
        manifest_url: impl Into<String>,
    ) -> Self {
        self.fabric_meta_url = meta_url.into().trim_end_matches('/').to_owned();
        self.manifest_url = manifest_url.into();
        self
    }

    pub async fn versions(
        &self,
        kind: LoaderKind,
        minecraft: &str,
    ) -> Result<Vec<LoaderVersion>, LoaderError> {
        match kind {
            LoaderKind::Fabric => {
                fabric_versions(&self.config, &self.http, &self.fabric_meta_url, minecraft).await
            }
            LoaderKind::Forge | LoaderKind::NeoForge => {
                forge_versions(&self.config, kind, minecraft).await
            }
        }
    }

    pub async fn install(
        &self,
        request: &LoaderRequest,
        options: &InstallOptions,
        context: Option<TaskContext>,
    ) -> Result<String, LoaderError> {
        check_cancelled(&context)?;
        match request.kind {
            LoaderKind::Fabric => {
                install_fabric(
                    &self.config,
                    &self.http,
                    &self.fabric_meta_url,
                    &self.manifest_url,
                    request,
                    options,
                    context,
                )
                .await
            }
            LoaderKind::Forge | LoaderKind::NeoForge => {
                forge_install(&self.config, request, options, context).await
            }
        }
    }
}

impl LoaderError {
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self,
            Self::Cancelled
                | Self::Download(DownloadError::Cancelled)
                | Self::Install(InstallError::Cancelled)
                | Self::Install(InstallError::Download(DownloadError::Cancelled))
        )
    }
}

fn core_network_error(error: crate::Error) -> LoaderError {
    match error {
        crate::Error::Network(error) => LoaderError::Network(error),
        other => LoaderError::Unsupported(other.to_string()),
    }
}

async fn fabric_versions(
    config: &DownloadConfig,
    http: &HttpClient,
    meta_url: &str,
    minecraft: &str,
) -> Result<Vec<LoaderVersion>, LoaderError> {
    if minecraft.trim().is_empty() {
        return Err(LoaderError::Unsupported(
            "Minecraft version is empty".into(),
        ));
    }
    #[derive(Deserialize)]
    struct Entry {
        loader: LoaderEntry,
    }
    #[derive(Deserialize)]
    struct LoaderEntry {
        version: String,
        #[serde(default)]
        stable: bool,
    }
    let url = format!(
        "{meta_url}/v2/versions/loader/{}",
        encode_segment(minecraft)
    );
    let mut errors = Vec::new();
    for candidate in url_candidates(&url, DownloadKind::VersionList, config) {
        match http.get_json::<Vec<Entry>>(&candidate).await {
            Ok(entries) => {
                return Ok(entries
                    .into_iter()
                    .map(|e| LoaderVersion {
                        version: e.loader.version,
                        stable: e.loader.stable,
                    })
                    .collect());
            }
            Err(e) => errors.push(format!("{candidate}: {e}")),
        }
    }
    Err(LoaderError::Unsupported(format!(
        "Fabric versions unavailable: {}",
        errors.join("; ")
    )))
}

async fn install_fabric(
    config: &DownloadConfig,
    http: &HttpClient,
    meta_url: &str,
    manifest_url: &str,
    request: &LoaderRequest,
    options: &InstallOptions,
    context: Option<TaskContext>,
) -> Result<String, LoaderError> {
    safe_segment(&request.minecraft)?;
    safe_segment(&request.version)?;
    ensure_install_roots_safe(&options.game_directory, &request.minecraft)?;
    let available = fabric_versions(config, http, meta_url, &request.minecraft).await?;
    if !available.iter().any(|v| v.version == request.version) {
        return Err(LoaderError::Unsupported(format!(
            "Fabric {} does not support Minecraft {}",
            request.version, request.minecraft
        )));
    }

    // The base install populates vanilla client/assets/libraries in the isolated staging directory.
    InstallService::new(config.clone())?
        .with_manifest_url(manifest_url)
        .install(&request.minecraft, options, context.clone())
        .await?;
    check_cancelled(&context)?;

    let url = format!(
        "{meta_url}/v2/versions/loader/{}/{}/profile/json",
        encode_segment(&request.minecraft),
        encode_segment(&request.version)
    );
    let mut failures = Vec::new();
    let mut profile = None;
    for candidate in url_candidates(&url, DownloadKind::File, config) {
        match http.get_json::<serde_json::Value>(&candidate).await {
            Ok(value) => match VersionMetadata::parse(&value.to_string()) {
                Ok(metadata) => {
                    profile = Some(metadata);
                    break;
                }
                Err(e) => failures.push(format!("{candidate}: {e}")),
            },
            Err(e) => failures.push(format!("{candidate}: {e}")),
        }
    }
    let mut profile = profile.ok_or_else(|| {
        LoaderError::Unsupported(format!(
            "Fabric profile unavailable: {}",
            failures.join("; ")
        ))
    })?;
    let id = validate_fabric_profile(&profile, &request.minecraft)?;
    install_libraries(
        config,
        &options.game_directory,
        &mut profile,
        options,
        &context,
    )
    .await?;
    check_cancelled(&context)?;
    let version_dir = options.game_directory.join("versions").join(&id);
    fs::create_dir_all(&version_dir).map_err(|source| io(&version_dir, source))?;
    let path = version_dir.join(format!("{id}.json"));
    if path.exists() {
        let existing = fs::read(&path).map_err(|source| io(&path, source))?;
        let incoming =
            serde_json::to_vec_pretty(&profile).map_err(metadata::MetadataError::from)?;
        if existing != incoming {
            return Err(LoaderError::Conflict(format!(
                "version metadata already exists with different contents: {}",
                path.display()
            )));
        }
    } else {
        let bytes = serde_json::to_vec_pretty(&profile).map_err(metadata::MetadataError::from)?;
        fs::write(&path, bytes).map_err(|source| io(&path, source))?;
    }
    Ok(id)
}

fn validate_fabric_profile(
    profile: &VersionMetadata,
    minecraft: &str,
) -> Result<String, LoaderError> {
    let id = profile.id.clone().ok_or_else(|| {
        LoaderError::Metadata(metadata::MetadataError::Invalid(
            "Fabric profile has no id".into(),
        ))
    })?;
    safe_segment(&id)?;
    if profile.inherits_from.as_deref() != Some(minecraft) {
        return Err(LoaderError::Conflict(format!(
            "Fabric profile base {:?} does not match requested Minecraft {minecraft}",
            profile.inherits_from
        )));
    }
    if profile.main_class.as_deref().is_none_or(str::is_empty) {
        return Err(LoaderError::Metadata(metadata::MetadataError::Invalid(
            "Fabric profile has no mainClass".into(),
        )));
    }
    Ok(id)
}

/// Downloads a profile's ordinary client libraries with source preference and SHA-1 checks.
pub(crate) async fn install_libraries(
    config: &DownloadConfig,
    game_dir: &Path,
    profile: &mut VersionMetadata,
    options: &InstallOptions,
    context: &Option<TaskContext>,
) -> Result<(), LoaderError> {
    let manager = DownloadManager::new(config)?;
    for library in &mut profile.libraries {
        check_cancelled(context)?;
        // Forge/Fabric profiles can contain platform-specific rules. Each installer writes a
        // client profile, so respect the profile's normal rule semantics for this platform.
        let features = std::collections::HashMap::new();
        let rule = metadata::RuleContext {
            os: &options.os,
            os_version: &options.os_version,
            arch: &options.arch,
            features: &features,
        };
        if !metadata::library_allowed(&library.rules, &rule) {
            continue;
        }
        let (relative, url, sha1, size, maven_form) = if let Some(artifact) =
            &library.downloads.artifact
        {
            let path = artifact.path.as_deref().ok_or_else(|| {
                LoaderError::Unsupported(format!(
                    "loader library {} has no artifact path",
                    library.name
                ))
            })?;
            let url = artifact
                .url
                .as_deref()
                .or(library.url.as_deref())
                .ok_or_else(|| {
                    LoaderError::Unsupported(format!("loader library {} has no URL", library.name))
                })?;
            (
                safe_relative(path)?,
                url.to_owned(),
                artifact.sha1.clone(),
                artifact.size,
                false,
            )
        } else {
            let (relative, base, version, filename) =
                maven_artifact(&library.name, library.url.as_deref())?;
            let sha1 = library
                .extra
                .get("sha1")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned);
            let size = library
                .extra
                .get("size")
                .and_then(serde_json::Value::as_u64);
            let group_path = library
                .name
                .split(':')
                .next()
                .unwrap_or_default()
                .replace('.', "/");
            (
                relative,
                format!(
                    "{base}{group_path}/{}/{version}/{filename}",
                    library.name.split(':').nth(1).unwrap_or_default()
                ),
                sha1,
                size,
                true,
            )
        };
        let sha1 = match sha1 {
            Some(hash) => hash,
            None => fetch_maven_sha1(config, &format!("{url}.sha1"))
                .await
                .map_err(|e| match e {
                    LoaderError::Unsupported(message) => LoaderError::Unsupported(format!(
                        "loader library {} checksum unavailable: {message}",
                        library.name
                    )),
                    other => other,
                })?,
        };
        if sha1.len() != 40 || !sha1.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(LoaderError::Unsupported(format!(
                "loader library {} has invalid SHA-1",
                library.name
            )));
        }
        let mut req = DownloadRequest::new(
            url_candidates(&url, DownloadKind::File, config),
            game_dir.join("libraries").join(relative.clone()),
        )
        .with_sha1(&sha1);
        if let Some(size) = size {
            req = req.with_size(size);
        }
        ensure_path_has_no_symlinks(&req.destination, game_dir)?;
        if let Some(ctx) = context {
            req = req.with_task_context(ctx.clone());
        }
        manager.download(req).await?;
        if maven_form {
            let artifact = metadata::DownloadInfo {
                path: Some(relative.to_string_lossy().replace('\\', "/")),
                url: Some(url),
                sha1: Some(sha1),
                size,
                ..Default::default()
            };
            library.downloads.artifact = Some(artifact);
        } else if let Some(artifact) = &mut library.downloads.artifact
            && artifact.sha1.is_none()
        {
            artifact.sha1 = Some(sha1);
        }
    }
    Ok(())
}

fn maven_artifact(
    name: &str,
    repository: Option<&str>,
) -> Result<(PathBuf, String, String, String), LoaderError> {
    let fields: Vec<_> = name.split(':').collect();
    if fields.len() < 3 || fields.len() > 4 || fields.iter().any(|part| part.is_empty()) {
        return Err(LoaderError::Unsupported(format!(
            "invalid Maven coordinate: {name}"
        )));
    }
    let group = fields[0].replace('.', "/");
    let artifact = fields[1];
    let version = fields[2];
    let classifier = fields.get(3).map(|s| format!("-{s}")).unwrap_or_default();
    let filename = format!("{artifact}-{version}{classifier}.jar");
    let path = safe_relative(&format!("{group}/{artifact}/{version}/{filename}"))?;
    let base = repository.unwrap_or("https://maven.fabricmc.net/");
    let base = if base.ends_with('/') {
        base.to_owned()
    } else {
        format!("{base}/")
    };
    Ok((path, base, version.to_owned(), filename))
}

async fn fetch_maven_sha1(config: &DownloadConfig, url: &str) -> Result<String, LoaderError> {
    let mut builder = reqwest::Client::builder()
        .user_agent(concat!("CialloCraftLauncher/", env!("CARGO_PKG_VERSION")));
    if let Some(proxy) = &config.http_proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
    }
    let client = builder.build()?;
    let mut failures = Vec::new();
    let mut network_error = None;
    for candidate in url_candidates(url, DownloadKind::File, config) {
        match client.get(&candidate).send().await {
            Ok(response) => match response.error_for_status() {
                Ok(response) => match response.text().await {
                    Ok(body) => {
                        let hash = body.split_whitespace().next().unwrap_or_default();
                        if hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                            return Ok(hash.to_ascii_lowercase());
                        }
                        failures.push(format!("{candidate}: invalid SHA-1 sidecar"));
                    }
                    Err(error) => {
                        failures.push(format!("{candidate}: {error}"));
                        network_error = Some(error);
                    }
                },
                Err(error) => {
                    failures.push(format!("{candidate}: {error}"));
                    network_error = Some(error);
                }
            },
            Err(error) => {
                failures.push(format!("{candidate}: {error}"));
                network_error = Some(error);
            }
        }
    }
    if let Some(error) = network_error {
        return Err(LoaderError::Network(error));
    }
    Err(LoaderError::Unsupported(failures.join("; ")))
}

fn check_cancelled(context: &Option<TaskContext>) -> Result<(), LoaderError> {
    if context
        .as_ref()
        .is_some_and(|c| c.cancellation.is_cancelled())
    {
        Err(LoaderError::Cancelled)
    } else {
        Ok(())
    }
}
fn encode_segment(value: &str) -> String {
    urlencoding(value)
}
fn urlencoding(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn safe_segment(value: &str) -> Result<(), LoaderError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains(['/', '\\', ':'])
        || value.chars().any(char::is_control)
    {
        return Err(LoaderError::Unsupported(format!(
            "unsafe version component: {value}"
        )));
    }
    Ok(())
}
fn safe_relative(value: &str) -> Result<PathBuf, LoaderError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains(['\\', ':'])
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(LoaderError::Unsupported(format!(
            "unsafe library path: {value}"
        )));
    }
    Ok(path.to_path_buf())
}

fn ensure_install_roots_safe(game_dir: &Path, minecraft: &str) -> Result<(), LoaderError> {
    for path in [
        game_dir.to_path_buf(),
        game_dir.join("versions"),
        game_dir.join("versions").join(minecraft),
        game_dir.join("libraries"),
        game_dir.join("assets"),
        game_dir.join("assets/indexes"),
        game_dir.join("assets/objects"),
    ] {
        if let Ok(metadata) = fs::symlink_metadata(&path)
            && metadata.file_type().is_symlink()
        {
            return Err(LoaderError::Conflict(format!(
                "installation path is a symbolic link: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn ensure_path_has_no_symlinks(destination: &Path, root: &Path) -> Result<(), LoaderError> {
    let relative = destination.strip_prefix(root).map_err(|_| {
        LoaderError::Conflict(format!(
            "artifact path escapes game directory: {}",
            destination.display()
        ))
    })?;
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&path)
            && metadata.file_type().is_symlink()
        {
            return Err(LoaderError::Conflict(format!(
                "installation path is a symbolic link: {}",
                path.display()
            )));
        }
    }
    Ok(())
}
fn io(path: &Path, source: std::io::Error) -> LoaderError {
    LoaderError::Io {
        path: path.to_path_buf(),
        source,
    }
}

// Implemented by the Forge loader owner in the same module.
async fn forge_versions(
    config: &DownloadConfig,
    kind: LoaderKind,
    minecraft: &str,
) -> Result<Vec<LoaderVersion>, LoaderError> {
    crate::forge::versions(config, kind, minecraft).await
}
async fn forge_install(
    config: &DownloadConfig,
    request: &LoaderRequest,
    options: &InstallOptions,
    context: Option<TaskContext>,
) -> Result<String, LoaderError> {
    crate::forge::install(config, request, options, context).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: &str = r#"{
      "id":"fabric-loader-0.16.10-1.21.4",
      "inheritsFrom":"1.21.4",
      "mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient",
      "arguments":{"game":[]},
      "libraries":[{"name":"net.fabricmc:fabric-loader:0.16.10","url":"https://maven.fabricmc.net/","sha1":"0123456789012345678901234567890123456789","size":42}]
    }"#;

    #[test]
    fn accepts_fabric_meta_client_profile_and_maven_library_shape() {
        let profile = VersionMetadata::parse(PROFILE).unwrap();
        assert_eq!(
            validate_fabric_profile(&profile, "1.21.4").unwrap(),
            "fabric-loader-0.16.10-1.21.4"
        );
        let (path, base, version, filename) = maven_artifact(
            "net.fabricmc:fabric-loader:0.16.10",
            Some("https://maven.fabricmc.net"),
        )
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("net/fabricmc/fabric-loader/0.16.10/fabric-loader-0.16.10.jar")
        );
        assert_eq!(
            format!("{base}net/fabricmc/fabric-loader/{version}/{filename}"),
            "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.16.10/fabric-loader-0.16.10.jar"
        );
    }

    #[test]
    fn rejects_a_profile_for_a_different_minecraft_version() {
        let profile = VersionMetadata::parse(PROFILE).unwrap();
        assert!(matches!(
            validate_fabric_profile(&profile, "1.21.3"),
            Err(LoaderError::Conflict(_))
        ));
    }

    #[test]
    fn cancellation_is_reported_as_cancelled() {
        let token = crate::CancellationToken::default();
        token.cancel();
        let context = TaskContext::new(token, std::sync::Arc::new(|_| {}));
        assert!(matches!(
            check_cancelled(&Some(context)),
            Err(LoaderError::Cancelled)
        ));
        assert!(LoaderError::Download(DownloadError::Cancelled).is_cancelled());
        assert!(
            LoaderError::Install(InstallError::Download(DownloadError::Cancelled)).is_cancelled()
        );
    }
}
