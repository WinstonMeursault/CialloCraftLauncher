//! Vanilla installation, with no command-line or graphical interface dependency.

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use thiserror::Error;
use tokio::task::JoinSet;

use crate::{
    TaskContext,
    config::DownloadConfig,
    download::{DownloadError, DownloadManager, DownloadRequest},
    metadata::{self, DownloadInfo, RuleContext, VersionManifest, VersionMetadata},
    network::HttpClient,
    source::{DownloadKind, url_candidates},
};

pub const VERSION_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("version not found: {0}")]
    VersionNotFound(String),
    #[error("invalid metadata: {0}")]
    Metadata(#[from] metadata::MetadataError),
    #[error("invalid resource: {0}")]
    InvalidResource(String),
    #[error("instance conflict: {0}")]
    Conflict(String),
    #[error("download failed: {0}")]
    Download(#[from] DownloadError),
    #[error("file operation failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("version list unavailable: {0}")]
    VersionList(String),
    #[error("installation cancelled")]
    Cancelled,
    #[error("worker failed: {0}")]
    Worker(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallSummary {
    pub version: String,
    pub game_directory: PathBuf,
    pub downloaded_files: usize,
    pub reused_files: usize,
    pub total_files: usize,
}

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub game_directory: PathBuf,
    pub os: String,
    pub arch: String,
    pub os_version: String,
}

impl InstallOptions {
    pub fn for_current_platform(game_directory: impl Into<PathBuf>) -> Self {
        Self {
            game_directory: game_directory.into(),
            os: match std::env::consts::OS {
                "macos" => "osx",
                other => other,
            }
            .into(),
            arch: std::env::consts::ARCH.into(),
            os_version: String::new(),
        }
    }
}

#[derive(Clone)]
pub struct InstallService {
    config: DownloadConfig,
    downloader: DownloadManager,
    http: HttpClient,
    manifest_url: String,
}

impl InstallService {
    pub fn new(config: DownloadConfig) -> Result<Self, InstallError> {
        let downloader = DownloadManager::new(&config)?;
        let http =
            HttpClient::new(&config).map_err(|e| InstallError::VersionList(e.to_string()))?;
        Ok(Self {
            config,
            downloader,
            http,
            manifest_url: VERSION_MANIFEST_URL.into(),
        })
    }

    /// Replace the manifest endpoint, chiefly for private mirrors and local tests.
    pub fn with_manifest_url(mut self, url: impl Into<String>) -> Self {
        self.manifest_url = url.into();
        self
    }

    pub async fn list_versions(&self) -> Result<VersionManifest, InstallError> {
        self.list_versions_from(&url_candidates(
            &self.manifest_url,
            DownloadKind::VersionList,
            &self.config,
        ))
        .await
    }

    /// List versions using the per-game manifest cache when network offline mode is enabled.
    /// In online mode, refresh the manifest and persist the validated result for later offline use.
    pub async fn list_versions_for(
        &self,
        game_directory: &Path,
    ) -> Result<VersionManifest, InstallError> {
        if self.config.offline {
            return read_cached_version_manifest(game_directory);
        }
        let manifest = self.list_versions().await?;
        save_version_manifest_cache(game_directory, &manifest)?;
        Ok(manifest)
    }

    async fn list_versions_from(
        &self,
        candidates: &[String],
    ) -> Result<VersionManifest, InstallError> {
        let mut failures = Vec::new();
        for url in candidates {
            match self.http.get_json::<serde_json::Value>(url).await {
                Ok(value) => match VersionManifest::parse(&value.to_string()) {
                    Ok(manifest) => {
                        // Some mirrors serve the older manifest without metadata hashes.
                        // It cannot satisfy verified installation; try the next source.
                        if manifest.versions.iter().all(|entry| {
                            entry.sha1.as_deref().is_some_and(|hash| {
                                hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit())
                            })
                        }) {
                            return Ok(manifest);
                        }
                        failures.push(format!(
                            "{url}: version manifest lacks valid metadata SHA-1 values"
                        ));
                    }
                    Err(error) => failures.push(format!("{url}: {error}")),
                },
                Err(e) => failures.push(format!("{url}: {e}")),
            }
        }
        Err(InstallError::VersionList(failures.join("; ")))
    }

    pub async fn install(
        &self,
        version: &str,
        options: &InstallOptions,
        context: Option<TaskContext>,
    ) -> Result<InstallSummary, InstallError> {
        safe_segment(version)?;
        check_cancelled(&context)?;
        let manifest = self.list_versions_for(&options.game_directory).await?;
        if !manifest.versions.iter().any(|entry| entry.id == version) {
            return Err(InstallError::VersionNotFound(version.into()));
        }
        fs::create_dir_all(&options.game_directory)
            .map_err(|source| io(&options.game_directory, source))?;
        let version_dir = options.game_directory.join("versions").join(version);
        fs::create_dir_all(&version_dir).map_err(|source| io(&version_dir, source))?;
        let lock_path = version_dir.join(".ccl-install.lock");
        let lock = File::options()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| io(&lock_path, source))?;
        lock.try_lock().map_err(|_| {
            InstallError::Conflict(format!("another install is using {version_dir:?}"))
        })?;

        let mut documents = HashMap::new();
        let mut current = version.to_owned();
        let mut visited = HashSet::new();
        let mut downloaded = 0usize;
        let mut reused = 0usize;
        loop {
            check_cancelled(&context)?;
            safe_segment(&current)?;
            if !visited.insert(current.clone()) {
                return Err(InstallError::Metadata(
                    metadata::MetadataError::InheritanceCycle(current),
                ));
            }
            if visited.len() > 32 {
                return Err(InstallError::InvalidResource(
                    "inheritance is deeper than 32 versions".into(),
                ));
            }
            let entry = manifest
                .versions
                .iter()
                .find(|entry| entry.id == current)
                .ok_or_else(|| {
                    InstallError::Metadata(metadata::MetadataError::MissingParent(current.clone()))
                })?;
            let hash = required_sha1(entry.sha1.as_deref(), "version metadata")?;
            let path = options
                .game_directory
                .join("versions")
                .join(&current)
                .join(format!("{current}.json"));
            if path.exists() && !file_matches(&path, &hash)? {
                return Err(InstallError::Conflict(format!(
                    "existing version metadata differs from the official manifest: {}",
                    path.display()
                )));
            }
            let outcome = self
                .downloader
                .download(request(
                    &self.config,
                    &entry.url,
                    DownloadKind::File,
                    path.clone(),
                    Some(&hash),
                    None,
                    &context,
                )?)
                .await?;
            count_outcome(outcome.reused_existing, &mut downloaded, &mut reused);
            let contents = fs::read_to_string(&path).map_err(|source| io(&path, source))?;
            let metadata = VersionMetadata::parse(&contents)?;
            if metadata.id.as_deref() != Some(current.as_str()) {
                return Err(InstallError::InvalidResource(format!(
                    "version metadata ID does not match {current}"
                )));
            }
            let parent = metadata.inherits_from.clone();
            documents.insert(current.clone(), metadata);
            match parent {
                Some(id) => current = id,
                None => break,
            }
        }
        let resolved = metadata::resolve_inheritance(version, &documents)?;
        let mut requests = Vec::new();
        let client = resolved.client().ok_or_else(|| {
            InstallError::InvalidResource("version has no client download".into())
        })?;
        requests.push(resource_request(
            &self.config,
            client,
            options
                .game_directory
                .join("versions")
                .join(version)
                .join(format!("{version}.jar")),
            &context,
        )?);

        let features = HashMap::new();
        let rule_context = RuleContext {
            os: &options.os,
            os_version: &options.os_version,
            arch: &options.arch,
            features: &features,
        };
        for library in &resolved.libraries {
            if !metadata::library_allowed(&library.rules, &rule_context) {
                continue;
            }
            if let Some(artifact) = &library.downloads.artifact {
                let path = artifact.path.as_deref().ok_or_else(|| {
                    InstallError::InvalidResource(format!("library {} has no path", library.name))
                })?;
                requests.push(resource_request(
                    &self.config,
                    artifact,
                    options
                        .game_directory
                        .join("libraries")
                        .join(safe_relative(path)?),
                    &context,
                )?);
            }
            if let Some((_, native)) = resolved.native_classifier_for(library, &rule_context) {
                let path = native.path.as_deref().ok_or_else(|| {
                    InstallError::InvalidResource(format!(
                        "native library {} has no path",
                        library.name
                    ))
                })?;
                requests.push(resource_request(
                    &self.config,
                    native,
                    options
                        .game_directory
                        .join("libraries")
                        .join(safe_relative(path)?),
                    &context,
                )?);
            }
        }

        let index = resolved
            .asset_index
            .as_ref()
            .ok_or_else(|| InstallError::InvalidResource("version has no asset index".into()))?;
        let index_id = index
            .id
            .as_deref()
            .ok_or_else(|| InstallError::InvalidResource("asset index has no ID".into()))?;
        safe_segment(index_id)?;
        let index_url = index
            .url
            .as_deref()
            .ok_or_else(|| InstallError::InvalidResource("asset index has no URL".into()))?;
        let index_hash = required_sha1(index.sha1.as_deref(), "asset index")?;
        let index_path = options
            .game_directory
            .join("assets/indexes")
            .join(format!("{index_id}.json"));
        let outcome = self
            .downloader
            .download(request(
                &self.config,
                index_url,
                DownloadKind::File,
                index_path.clone(),
                Some(&index_hash),
                index.size,
                &context,
            )?)
            .await?;
        count_outcome(outcome.reused_existing, &mut downloaded, &mut reused);
        let index_data = fs::read(&index_path).map_err(|source| io(&index_path, source))?;
        let index: AssetObjects =
            serde_json::from_slice(&index_data).map_err(metadata::MetadataError::from)?;
        for asset in index.objects.into_values() {
            let hash = required_sha1(Some(&asset.hash), "asset object")?;
            let prefix = &hash[..2];
            let official_url = format!("https://resources.download.minecraft.net/{prefix}/{hash}");
            requests.push(request(
                &self.config,
                &official_url,
                DownloadKind::File,
                options
                    .game_directory
                    .join("assets/objects")
                    .join(prefix)
                    .join(&hash),
                Some(&hash),
                Some(asset.size),
                &context,
            )?);
        }

        // Bound both transfers and queued tasks. Shared files can appear repeatedly in
        // metadata; downloading each destination once avoids same-path races.
        let mut unique = HashSet::new();
        requests.retain(|r| unique.insert(r.destination.clone()));
        let total_files = downloaded + reused + requests.len();
        let width = self.config.max_parallel;
        let mut pending = requests.into_iter();
        let mut workers = JoinSet::new();
        for _ in 0..width {
            if let Some(req) = pending.next() {
                spawn_download(&mut workers, self.downloader.clone(), req);
            }
        }
        while let Some(result) = workers.join_next().await {
            match result.map_err(|e| InstallError::Worker(e.to_string()))? {
                Ok(outcome) => count_outcome(outcome.reused_existing, &mut downloaded, &mut reused),
                Err(error) => {
                    workers.abort_all();
                    return Err(error.into());
                }
            }
            check_cancelled(&context)?;
            if let Some(req) = pending.next() {
                spawn_download(&mut workers, self.downloader.clone(), req);
            }
        }
        Ok(InstallSummary {
            version: version.into(),
            game_directory: options.game_directory.clone(),
            downloaded_files: downloaded,
            reused_files: reused,
            total_files,
        })
    }
}

fn spawn_download(
    workers: &mut JoinSet<Result<crate::download::DownloadOutcome, DownloadError>>,
    manager: DownloadManager,
    request: DownloadRequest,
) {
    workers.spawn(async move { manager.download(request).await });
}

fn count_outcome(reused_existing: bool, downloaded: &mut usize, reused: &mut usize) {
    if reused_existing {
        *reused += 1;
    } else {
        *downloaded += 1;
    }
}

fn request(
    config: &DownloadConfig,
    url: &str,
    kind: DownloadKind,
    path: PathBuf,
    hash: Option<&str>,
    size: Option<u64>,
    context: &Option<TaskContext>,
) -> Result<DownloadRequest, InstallError> {
    let mut req = DownloadRequest::new(url_candidates(url, kind, config), path);
    if let Some(hash) = hash {
        req = req.with_sha1(hash);
    }
    if let Some(size) = size {
        req = req.with_size(size);
    }
    if let Some(context) = context {
        req = req.with_task_context(context.clone());
    }
    Ok(req)
}

fn resource_request(
    config: &DownloadConfig,
    info: &DownloadInfo,
    path: PathBuf,
    context: &Option<TaskContext>,
) -> Result<DownloadRequest, InstallError> {
    let url = info.url.as_deref().ok_or_else(|| {
        InstallError::InvalidResource(format!("resource has no URL: {}", path.display()))
    })?;
    let hash = required_sha1(info.sha1.as_deref(), "resource")?;
    request(
        config,
        url,
        DownloadKind::File,
        path,
        Some(&hash),
        info.size,
        context,
    )
}

fn required_sha1(value: Option<&str>, label: &str) -> Result<String, InstallError> {
    let hash =
        value.ok_or_else(|| InstallError::InvalidResource(format!("{label} has no SHA-1")))?;
    if hash.len() != 40 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(InstallError::InvalidResource(format!(
            "{label} has invalid SHA-1"
        )));
    }
    Ok(hash.to_ascii_lowercase())
}

fn safe_segment(segment: &str) -> Result<(), InstallError> {
    if segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.contains(['/', '\\', ':'])
        || segment.chars().any(char::is_control)
    {
        return Err(InstallError::InvalidResource(format!(
            "unsafe path component: {segment}"
        )));
    }
    Ok(())
}

fn safe_relative(path: &str) -> Result<PathBuf, InstallError> {
    if path.contains('\\') || path.contains(':') || path.is_empty() {
        return Err(InstallError::InvalidResource(format!(
            "unsafe relative path: {path}"
        )));
    }
    let path = Path::new(path);
    if path
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(InstallError::InvalidResource(format!(
            "unsafe relative path: {}",
            path.display()
        )));
    }
    Ok(path.to_path_buf())
}

fn file_matches(path: &Path, expected: &str) -> Result<bool, InstallError> {
    let bytes = fs::read(path).map_err(|source| io(path, source))?;
    Ok(format!("{:x}", Sha1::digest(bytes)) == expected)
}

fn check_cancelled(context: &Option<TaskContext>) -> Result<(), InstallError> {
    if context
        .as_ref()
        .is_some_and(|context| context.cancellation.is_cancelled())
    {
        Err(InstallError::Cancelled)
    } else {
        Ok(())
    }
}

fn io(path: &Path, source: std::io::Error) -> InstallError {
    InstallError::Io {
        path: path.into(),
        source,
    }
}

fn write_cache_atomic(path: &Path, bytes: &[u8]) -> Result<(), InstallError> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".tmp-{}", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| io(&temporary, e))?;
        file.write_all(bytes).map_err(|e| io(&temporary, e))?;
        file.sync_all().map_err(|e| io(&temporary, e))?;
        #[cfg(windows)]
        if path.exists() {
            fs::remove_file(path).map_err(|e| io(path, e))?;
        }
        fs::rename(&temporary, path).map_err(|e| io(path, e))
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

fn save_version_manifest_cache(
    game_directory: &Path,
    manifest: &VersionManifest,
) -> Result<PathBuf, InstallError> {
    let cache = game_directory.join(".ccl/cache/version_manifest.json");
    if let Some(parent) = cache.parent() {
        fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    let bytes =
        serde_json::to_vec(manifest).map_err(|e| InstallError::VersionList(e.to_string()))?;
    write_cache_atomic(&cache, &bytes)?;
    Ok(cache)
}

fn read_cached_version_manifest(game_directory: &Path) -> Result<VersionManifest, InstallError> {
    let cache = game_directory.join(".ccl/cache/version_manifest.json");
    let bytes = fs::read(&cache).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            InstallError::VersionList(format!(
                "offline mode: version manifest is not cached at {}",
                cache.display()
            ))
        } else {
            io(&cache, error)
        }
    })?;
    let manifest = VersionManifest::parse(&String::from_utf8_lossy(&bytes))?;
    if manifest.versions.iter().any(|entry| {
        entry
            .sha1
            .as_deref()
            .is_none_or(|hash| hash.len() != 40 || !hash.bytes().all(|b| b.is_ascii_hexdigit()))
    }) {
        return Err(InstallError::VersionList(
            "offline cache contains invalid metadata hashes".into(),
        ));
    }
    Ok(manifest)
}

#[derive(Deserialize)]
struct AssetObjects {
    objects: HashMap<String, AssetObject>,
}
#[derive(Deserialize)]
struct AssetObject {
    hash: String,
    size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn legacy_mirror_manifest_without_hashes_falls_back_to_verified_manifest() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            for hash in [None, Some("0123456789012345678901234567890123456789")] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 2048];
                assert!(stream.read(&mut request).unwrap() > 0);
                let mut entry =
                    serde_json::json!({"id":"fixture", "url":"https://example.com/version.json"});
                if let Some(hash) = hash {
                    entry["sha1"] = serde_json::json!(hash);
                }
                let body = serde_json::json!({"versions":[entry]}).to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            }
        });
        let service = InstallService::new(DownloadConfig::default()).unwrap();
        let result = service
            .list_versions_from(&[format!("{base}/legacy"), format!("{base}/verified")])
            .await
            .unwrap();
        assert_eq!(
            result.versions[0].sha1.as_deref(),
            Some("0123456789012345678901234567890123456789")
        );
        worker.join().unwrap();
    }

    #[test]
    fn rejects_path_traversal_and_bad_hashes() {
        for bad in ["../x", "/absolute", "C:/x", "a\\b"] {
            assert!(safe_relative(bad).is_err());
        }
        for bad in ["..", "a/b", "a\\b", "C:x"] {
            assert!(safe_segment(bad).is_err());
        }
        assert!(required_sha1(Some("abc"), "test").is_err());
    }

    #[tokio::test]
    async fn offline_install_without_manifest_fails_before_network_or_directory_creation() {
        let root = std::env::temp_dir().join(format!("ccl-offline-install-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let config = DownloadConfig {
            offline: true,
            ..Default::default()
        };
        let service = InstallService::new(config).unwrap();
        let options = InstallOptions::for_current_platform(&root);
        let error = service
            .install("known-version", &options, None)
            .await
            .unwrap_err();
        assert!(
            matches!(error, InstallError::VersionList(message) if message.contains("offline mode") && message.contains("version manifest"))
        );
        assert!(!root.exists());
    }

    #[tokio::test]
    async fn offline_version_listing_uses_cached_manifest_without_network() {
        let root = std::env::temp_dir().join(format!(
            "ccl-offline-versions-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&root);
        let manifest = VersionManifest::parse(&serde_json::json!({
            "latest":{"release":"known-version","snapshot":"known-version"},
            "versions":[{"id":"known-version","type":"release","url":"https://example.invalid/version.json","sha1":"0123456789012345678901234567890123456789"}]
        }).to_string()).unwrap();
        save_version_manifest_cache(&root, &manifest).unwrap();
        // This endpoint would fail if touched. Offline listing must be a pure cache read.
        let service = InstallService::new(DownloadConfig {
            offline: true,
            ..Default::default()
        })
        .unwrap()
        .with_manifest_url("http://127.0.0.1:1/should-not-connect");
        let listed = service.list_versions_for(&root).await.unwrap();
        assert_eq!(listed.versions[0].id, "known-version");
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn offline_version_listing_rejects_invalid_cached_hash_without_network() {
        let root = std::env::temp_dir().join(format!(
            "ccl-offline-invalid-versions-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&root);
        let cache = root.join(".ccl/cache/version_manifest.json");
        fs::create_dir_all(cache.parent().unwrap()).unwrap();
        fs::write(
            &cache,
            serde_json::json!({
                "latest":{"release":"known-version","snapshot":"known-version"},
                "versions":[{"id":"known-version","type":"release","url":"https://example.invalid/version.json","sha1":"not-a-valid-sha1"}]
            }).to_string(),
        )
        .unwrap();
        let service = InstallService::new(DownloadConfig {
            offline: true,
            ..Default::default()
        })
        .unwrap()
        .with_manifest_url("http://127.0.0.1:1/should-not-connect");
        let error = service.list_versions_for(&root).await.unwrap_err();
        assert!(
            matches!(error, InstallError::VersionList(message) if message.contains("invalid metadata hashes"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn offline_install_uses_persisted_manifest_then_reports_missing_metadata() {
        let root =
            std::env::temp_dir().join(format!("ccl-offline-cache-install-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let manifest = VersionManifest::parse(&serde_json::json!({
            "latest":{"release":"known-version","snapshot":"known-version"},
            "versions":[{"id":"known-version","type":"release","url":"https://example.invalid/version.json","sha1":"0123456789012345678901234567890123456789"}]
        }).to_string()).unwrap();
        let cache = save_version_manifest_cache(&root, &manifest).unwrap();
        let service = InstallService::new(DownloadConfig {
            offline: true,
            ..Default::default()
        })
        .unwrap();
        let options = InstallOptions::for_current_platform(&root);
        let error = service
            .install("known-version", &options, None)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            InstallError::Download(DownloadError::OfflineCacheMiss(_))
        ));
        assert!(cache.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
