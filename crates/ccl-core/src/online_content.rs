//! Modrinth and CurseForge catalog access for mods and modpacks.
//!
//! Provider credentials are supplied by the caller. This module never reads or stores secrets.

use crate::{
    TaskContext, TaskEvent,
    config::DownloadConfig,
    download::{DownloadError, DownloadManager, DownloadRequest},
    import_formats::{ImportFormatError, ImportFormatService, ImportOptionsExt},
    instances::{InstanceError, InstanceService},
    modpacks::{ImportOptions, ModpackError, ModpackService},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use std::{
    fs,
    future::Future,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use thiserror::Error;

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

const MODRINTH_API: &str = "https://api.modrinth.com/v2";
const CURSEFORGE_API: &str = "https://api.curseforge.com/v1";
const ONLINE_RECEIPT: &str = ".ccl/online-content.json";

async fn wait_for_cancellation(context: &TaskContext) {
    while !context.cancellation.is_cancelled() {
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
}

async fn await_or_cancel<T>(
    operation: impl Future<Output = Result<T, OnlineError>>,
    context: Option<&TaskContext>,
) -> Result<T, OnlineError> {
    if let Some(context) = context {
        if context.cancellation.is_cancelled() {
            return Err(DownloadError::Cancelled.into());
        }
        tokio::select! {
            result = operation => result,
            _ = wait_for_cancellation(context) => Err(DownloadError::Cancelled.into()),
        }
    } else {
        operation.await
    }
}

fn check_context(context: &Option<TaskContext>) -> Result<(), OnlineError> {
    check_context_ref(context.as_ref())
}

fn check_context_ref(context: Option<&TaskContext>) -> Result<(), OnlineError> {
    if context.is_some_and(|context| context.cancellation.is_cancelled()) {
        Err(DownloadError::Cancelled.into())
    } else {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct OnlineConfig {
    /// Optional CurseForge key supplied by the user or calling application.
    pub curseforge_api_key: Option<String>,
    /// Optional proxy URL shared with launcher downloads.
    pub http_proxy: Option<String>,
    /// Disable requests when the caller is operating offline.
    pub offline: bool,
}

impl std::fmt::Debug for OnlineConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnlineConfig")
            .field(
                "curseforge_api_key",
                &self.curseforge_api_key.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "http_proxy",
                &self.http_proxy.as_deref().map(|_| "[configured]"),
            )
            .field("offline", &self.offline)
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum OnlineError {
    #[error("online content is unavailable while offline")]
    Offline,
    #[error("CurseForge requires a user-provided API key")]
    MissingCurseForgeKey,
    #[error("provider request failed: {0}")]
    Request(String),
    #[error("provider returned HTTP {0}")]
    Http(u16),
    #[error("provider response is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid search request: {0}")]
    Invalid(String),
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error(transparent)]
    Instance(#[from] InstanceError),
    #[error("online content operation failed: {0}")]
    Io(String),
    #[error("online content conflicts: {0:?}")]
    Conflicts(Vec<ContentConflict>),
    #[error("online content conflict at {path}: {reason}")]
    Conflict { path: String, reason: String },
    #[error(transparent)]
    Modpack(#[from] ModpackError),
    #[error(transparent)]
    Import(#[from] ImportFormatError),
}
impl OnlineError {
    /// Whether the operation stopped because the caller cancelled it.
    pub fn is_cancelled(&self) -> bool {
        match self {
            Self::Download(DownloadError::Cancelled) => true,
            Self::Instance(error) => error.is_cancelled(),
            Self::Modpack(error) => error.is_cancelled(),
            Self::Import(error) => error.is_cancelled(),
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Modrinth,
    CurseForge,
}

#[derive(Clone, Debug, Default)]
pub struct SearchQuery {
    pub query: String,
    pub game_version: Option<String>,
    /// Loader identifiers such as `fabric` or `neoforge`.
    pub loaders: Vec<String>,
    /// `mod`, `modpack`, `resourcepack`, or `shader`.
    pub project_type: Option<String>,
    pub limit: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectHit {
    pub provider: Provider,
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub downloads: u64,
    pub icon_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VersionHit {
    pub provider: Provider,
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub files: Vec<VersionFile>,
    pub dependencies: Vec<String>,
    pub optional_dependencies: Vec<String>,
    pub incompatible_dependencies: Vec<String>,
    pub pinned_dependencies: std::collections::BTreeMap<String, String>,
    pub dependency_requirements: Vec<DependencyRequirement>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DependencyRequirement {
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub kind: DependencyKind,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DependencyKind {
    Required,
    Optional,
    Incompatible,
    Embedded,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VersionFile {
    pub filename: String,
    pub url: String,
    pub size: u64,
    pub sha1: Option<String>,
    pub sha512: Option<String>,
    pub primary: bool,
}

/// Parameters for resolving and installing an online project into a managed instance.
#[derive(Clone, Debug)]
pub struct OnlineInstallRequest {
    pub root: PathBuf,
    pub instance_id: String,
    pub provider: Provider,
    pub project_id: String,
    pub version_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentConflict {
    pub path: String,
    pub versions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OnlineInstalledVersion {
    pub provider: Provider,
    pub project_id: String,
    pub version_id: String,
    pub version_number: String,
    #[serde(default)]
    pub files: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub metadata: Option<VersionHit>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct OnlineReceipt {
    schema: u32,
    versions: Vec<OnlineInstalledVersion>,
    owned_files: std::collections::BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct OnlineContentService {
    client: reqwest::Client,
    config: OnlineConfig,
    modrinth_api: String,
    curseforge_api: String,
}

impl OnlineContentService {
    /// Create an API client using the provided proxy, offline mode, and optional provider key.
    pub fn new(config: OnlineConfig) -> Result<Self, OnlineError> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(20))
            .timeout(std::time::Duration::from_secs(60))
            .user_agent(concat!("CialloCraftLauncher/", env!("CARGO_PKG_VERSION")));
        if let Some(proxy) = &config.http_proxy {
            builder = builder.proxy(
                reqwest::Proxy::all(proxy).map_err(|e| OnlineError::Request(e.to_string()))?,
            );
        }
        let client = builder
            .build()
            .map_err(|e| OnlineError::Request(e.to_string()))?;
        Ok(Self {
            client,
            config,
            modrinth_api: MODRINTH_API.into(),
            curseforge_api: CURSEFORGE_API.into(),
        })
    }

    /// Override service endpoints for controlled mirrors and deterministic service fixtures.
    pub fn with_endpoints(
        mut self,
        modrinth: impl Into<String>,
        curseforge: impl Into<String>,
    ) -> Self {
        self.modrinth_api = modrinth.into().trim_end_matches('/').into();
        self.curseforge_api = curseforge.into().trim_end_matches('/').into();
        self
    }
    /// Build online settings from launcher download configuration and a caller-owned provider key.
    pub fn from_download_config(
        config: &DownloadConfig,
        curseforge_api_key: Option<String>,
    ) -> Result<Self, OnlineError> {
        Self::new(OnlineConfig {
            curseforge_api_key,
            http_proxy: config.http_proxy.clone(),
            offline: config.offline,
        })
    }

    /// Download one version and add all of its primary JARs in one transactional content revision.
    pub async fn install_version(
        &self,
        root: PathBuf,
        download_config: &DownloadConfig,
        instance_id: &str,
        version: &VersionHit,
    ) -> Result<(), OnlineError> {
        self.install_version_with_context(root, download_config, instance_id, version, None)
            .await
    }

    /// Context-aware variant of [`Self::install_version`]. Downloads report progress and observe cancellation.
    pub async fn install_version_with_context(
        &self,
        root: PathBuf,
        download_config: &DownloadConfig,
        instance_id: &str,
        version: &VersionHit,
        context: Option<TaskContext>,
    ) -> Result<(), OnlineError> {
        self.install_versions_with_context(
            root,
            download_config,
            instance_id,
            std::slice::from_ref(version),
            context,
        )
        .await
    }

    /// Download a resolved dependency set, then atomically update its owned files.
    /// Other installed projects and unowned local files are retained.
    pub async fn install_versions(
        &self,
        root: PathBuf,
        config: &DownloadConfig,
        instance_id: &str,
        versions: &[VersionHit],
    ) -> Result<(), OnlineError> {
        self.install_versions_with_context(root, config, instance_id, versions, None)
            .await
    }

    /// Context-aware variant of [`Self::install_versions`].
    pub async fn install_versions_with_context(
        &self,
        root: PathBuf,
        config: &DownloadConfig,
        instance_id: &str,
        versions: &[VersionHit],
        context: Option<TaskContext>,
    ) -> Result<(), OnlineError> {
        self.online()?;
        check_context(&context)?;
        let conflicts = Self::analyze_conflicts(versions);
        if !conflicts.is_empty() {
            return Err(OnlineError::Conflicts(conflicts));
        }
        let staging = std::env::temp_dir().join(format!(
            "ccl-online-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&staging).map_err(|e| OnlineError::Io(e.to_string()))?;
        let result = async {
            let manager = DownloadManager::new(config)?;
            let mut downloads = Vec::new();
            let mut records = Vec::new();
            let mut total = 0u64;
            for version in versions {
                check_context(&context)?;
                let primary = version
                    .files
                    .iter()
                    .filter(|f| f.primary)
                    .collect::<Vec<_>>();
                let files = if primary.is_empty() {
                    version.files.first().into_iter().collect()
                } else {
                    primary
                };
                if files.is_empty() {
                    return Err(OnlineError::Invalid(format!(
                        "version {} has no downloadable file",
                        version.id
                    )));
                }
                let mut owned = std::collections::BTreeMap::new();
                for file in files {
                    validate_mod_name(&file.filename)?;
                    if file.sha1.is_none() && file.sha512.is_none() {
                        return Err(OnlineError::Invalid(format!(
                            "file {} has no supported integrity hash",
                            file.filename
                        )));
                    }
                    total = total
                        .checked_add(file.size)
                        .ok_or_else(|| OnlineError::Invalid("download size overflow".into()))?;
                    if file.size > 128 * 1024 * 1024 || total > 512 * 1024 * 1024 {
                        return Err(OnlineError::Invalid(
                            "selected content exceeds download limits".into(),
                        ));
                    }
                    let destination = staging.join(format!("{}.download", downloads.len()));
                    let host = reqwest::Url::parse(&file.url)
                        .map_err(|e| OnlineError::Invalid(e.to_string()))?
                        .host_str()
                        .ok_or_else(|| OnlineError::Invalid("download URL has no host".into()))?
                        .to_owned();
                    let mut allowed_hosts = vec![host];
                    if version.provider == Provider::CurseForge {
                        allowed_hosts.extend(curseforge_download_hosts());
                    }
                    let mut request = DownloadRequest::new(vec![file.url.clone()], &destination)
                        .with_size(file.size)
                        .with_allowed_hosts(allowed_hosts);
                    if let Some(hash) = &file.sha1 {
                        request = request.with_sha1(hash);
                    }
                    if let Some(hash) = &file.sha512 {
                        request = request.with_sha512(hash);
                    }
                    if version.provider == Provider::CurseForge {
                        let key = self
                            .config
                            .curseforge_api_key
                            .as_deref()
                            .ok_or(OnlineError::MissingCurseForgeKey)?;
                        request = request.with_api_key(key, curseforge_download_hosts())?;
                    }
                    if let Some(ctx) = context.clone() {
                        request = request.with_task_context(ctx);
                    }
                    manager.download(request).await?;
                    check_context(&context)?;
                    let hash = file_sha512(&destination)?;
                    owned.insert(file.filename.clone(), hash.clone());
                    downloads.push((file.filename.clone(), destination, hash));
                }
                records.push(OnlineInstalledVersion {
                    provider: version.provider,
                    project_id: version.project_id.clone(),
                    version_id: version.id.clone(),
                    version_number: version.version_number.clone(),
                    files: owned,
                    metadata: Some(version.clone()),
                });
            }
            check_context(&context)?;
            apply_downloads(
                root,
                config,
                instance_id,
                records,
                &downloads,
                context.as_ref(),
            )
        }
        .await;
        let _ = fs::remove_dir_all(&staging);
        result
    }

    /// Resolve a provider project for an installed instance and apply its full required dependency graph.
    pub async fn install_project(
        &self,
        root: PathBuf,
        config: &DownloadConfig,
        instance_id: &str,
        provider: Provider,
        project_id: &str,
        version_id: Option<&str>,
    ) -> Result<Vec<VersionHit>, OnlineError> {
        self.install_project_with_context(
            config,
            OnlineInstallRequest {
                root,
                instance_id: instance_id.into(),
                provider,
                project_id: project_id.into(),
                version_id: version_id.map(str::to_owned),
            },
            None,
        )
        .await
    }

    /// Resolve and install online content while forwarding task progress and cancellation to downloads.
    pub async fn install_project_with_context(
        &self,
        config: &DownloadConfig,
        request: OnlineInstallRequest,
        context: Option<TaskContext>,
    ) -> Result<Vec<VersionHit>, OnlineError> {
        let OnlineInstallRequest {
            root,
            instance_id,
            provider,
            project_id,
            version_id,
        } = request;
        self.online()?;
        check_context(&context)?;
        let instances = InstanceService::new(root.clone(), config.clone())?;
        let metadata = instances.metadata(&instance_id)?;
        let loader = metadata.loader.map(|kind| match kind {
            crate::loaders::LoaderKind::Fabric => "fabric",
            crate::loaders::LoaderKind::Quilt => "quilt",
            crate::loaders::LoaderKind::LegacyFabric => "legacy-fabric",
            crate::loaders::LoaderKind::OptiFine => "optifine",
            crate::loaders::LoaderKind::Forge => "forge",
            crate::loaders::LoaderKind::NeoForge => "neoforge",
        });
        let graph = self
            .resolve_project_version_with_context(
                provider,
                &project_id,
                version_id.as_deref(),
                &metadata.minecraft,
                loader,
                context.as_ref(),
            )
            .await?;
        self.install_versions_with_context(root, config, &instance_id, &graph, context)
            .await?;
        Ok(graph)
    }

    /// Download a provider modpack archive and install it through the validated transactional importer.
    pub async fn download_and_import_pack(
        &self,
        root: PathBuf,
        config: &DownloadConfig,
        instance_id: &str,
        version: &VersionHit,
        include_optional: bool,
        context: Option<TaskContext>,
    ) -> Result<crate::instances::InstanceMetadata, OnlineError> {
        self.online()?;
        let curseforge_api_key = self.config.curseforge_api_key.clone();
        let file = version
            .files
            .iter()
            .find(|f| f.primary)
            .or_else(|| version.files.first())
            .ok_or_else(|| {
                OnlineError::Invalid("modpack version has no downloadable file".into())
            })?;
        let host = reqwest::Url::parse(&file.url)
            .map_err(|e| OnlineError::Invalid(e.to_string()))?
            .host_str()
            .ok_or_else(|| OnlineError::Invalid("download URL has no host".into()))?
            .to_owned();
        let path = std::env::temp_dir().join(format!(
            "ccl-online-pack-{}-{}.zip",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let mut allowed_hosts = vec![host];
        if version.provider == Provider::CurseForge {
            allowed_hosts.extend(curseforge_download_hosts());
        }
        let mut request = DownloadRequest::new(vec![file.url.clone()], &path)
            .with_size(file.size)
            .with_allowed_hosts(allowed_hosts);
        if let Some(hash) = &file.sha1 {
            request = request.with_sha1(hash)
        }
        if let Some(hash) = &file.sha512 {
            request = request.with_sha512(hash)
        }
        if let Some(ctx) = context.clone() {
            request = request.with_task_context(ctx)
        }
        if version.provider == Provider::CurseForge {
            request = request.with_api_key(
                curseforge_api_key
                    .as_deref()
                    .ok_or(OnlineError::MissingCurseForgeKey)?,
                curseforge_download_hosts(),
            )?;
        }
        let result = async {
            DownloadManager::new(config)?.download(request).await?;
            match version.provider {
                Provider::Modrinth => ModpackService::new(root, config.clone())?
                    .import(
                        instance_id,
                        &path,
                        &ImportOptions {
                            include_optional,
                            ..Default::default()
                        },
                        context,
                    )
                    .await
                    .map_err(OnlineError::from),
                Provider::CurseForge => ImportFormatService::new(root, config.clone())
                    .import(
                        instance_id,
                        &path,
                        &ImportOptionsExt {
                            include_optional,
                            replace_modified: false,
                            curseforge_api_key,
                            allowed_hosts: Vec::new(),
                        },
                        context,
                    )
                    .await
                    .map_err(OnlineError::from),
            }
        }
        .await;
        let _ = fs::remove_file(path);
        result
    }

    async fn resolve_graph_from_context(
        &self,
        root: VersionHit,
        game_version: &str,
        loader: &str,
        context: Option<&TaskContext>,
    ) -> Result<Vec<VersionHit>, OnlineError> {
        let mut todo = vec![root];
        let mut seen = std::collections::BTreeSet::new();
        let mut resolved = Vec::new();
        while let Some(version) = todo.pop() {
            check_context_ref(context)?;
            if !seen.insert((version.project_id.clone(), version.id.clone())) {
                continue;
            }
            for dep in version
                .dependency_requirements
                .iter()
                .filter(|d| d.kind == DependencyKind::Required)
            {
                let (project, pin) = if let Some(project) = &dep.project_id {
                    (project.clone(), dep.version_id.clone())
                } else if let Some(id) = &dep.version_id {
                    let exact = await_or_cancel(self.modrinth_version_by_id(id), context).await?;
                    (exact.project_id, Some(id.clone()))
                } else {
                    continue;
                };
                if let Some((_, selected_id)) = seen.iter().find(|(p, _)| p == &project) {
                    if pin.as_ref().is_some_and(|expected| expected != selected_id) {
                        return Err(OnlineError::Invalid(format!(
                            "dependency {project} requires conflicting pinned versions"
                        )));
                    }
                    continue;
                }
                let selected = if let Some(id) = pin {
                    let exact = await_or_cancel(self.modrinth_version_by_id(&id), context).await?;
                    if exact.project_id != project
                        || !exact.game_versions.iter().any(|x| x == game_version)
                        || !exact.loaders.iter().any(|x| x == loader)
                    {
                        return Err(OnlineError::Invalid(format!(
                            "pinned dependency version {id} is incompatible"
                        )));
                    }
                    exact
                } else {
                    await_or_cancel(
                        self.versions(
                            Provider::Modrinth,
                            &project,
                            Some(game_version),
                            Some(loader),
                        ),
                        context,
                    )
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        OnlineError::Invalid(format!(
                            "no compatible version found for dependency {project}"
                        ))
                    })?
                };
                todo.push(selected)
            }
            resolved.push(version)
        }
        Ok(resolved)
    }

    async fn resolve_curseforge_graph_from_context(
        &self,
        root: VersionHit,
        game_version: &str,
        loader: Option<&str>,
        context: Option<&TaskContext>,
    ) -> Result<Vec<VersionHit>, OnlineError> {
        let mut todo = vec![root];
        let mut seen = std::collections::BTreeSet::new();
        let mut resolved = Vec::new();
        while let Some(version) = todo.pop() {
            check_context_ref(context)?;
            if !seen.insert((version.project_id.clone(), version.id.clone())) {
                continue;
            }
            for dep in &version.dependencies {
                if seen.iter().any(|(p, _)| p == dep) {
                    continue;
                }
                let selected = await_or_cancel(
                    self.versions(Provider::CurseForge, dep, Some(game_version), loader),
                    context,
                )
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    OnlineError::Invalid(format!(
                        "no compatible CurseForge dependency version found for project {dep}"
                    ))
                })?;
                todo.push(selected)
            }
            resolved.push(version)
        }
        Ok(resolved)
    }

    /// Search projects on the selected provider.
    pub async fn search(
        &self,
        provider: Provider,
        query: &SearchQuery,
    ) -> Result<Vec<ProjectHit>, OnlineError> {
        self.online()?;
        if query.query.trim().is_empty() {
            return Err(OnlineError::Invalid("query must not be blank".into()));
        }
        match provider {
            Provider::Modrinth => {
                let mut request = self
                    .client
                    .get(format!("{}/search", self.modrinth_api))
                    .query(&[
                        ("query", query.query.as_str()),
                        ("limit", &query.limit.clamp(1, 100).to_string()),
                    ]);
                let facets = modrinth_facets(query);
                if !facets.is_empty() {
                    request = request.query(&[("facets", serde_json::to_string(&facets)?)]);
                }
                let response = self.send(request).await?;
                let value: MrSearch = response
                    .json()
                    .await
                    .map_err(|e| OnlineError::Request(e.to_string()))?;
                Ok(project_hits_from_modrinth(value, provider))
            }
            Provider::CurseForge => {
                let mut request = self
                    .client
                    .get(format!("{}/mods/search", self.curseforge_api))
                    .query(&[
                        ("gameId", "432"),
                        ("searchFilter", query.query.as_str()),
                        ("pageSize", &query.limit.clamp(1, 50).to_string()),
                    ]);
                request = self.authorize_curseforge(request)?;
                if let Some(v) = &query.game_version {
                    request = request.query(&[("gameVersion", v)]);
                }
                if let Some(t) = &query.project_type {
                    let id = match t.as_str() {
                        "mod" => "6",
                        "modpack" => "4471",
                        "resourcepack" => "12",
                        "shader" => "6552",
                        _ => {
                            return Err(OnlineError::Invalid(format!(
                                "unsupported CurseForge project type `{t}`"
                            )));
                        }
                    };
                    request = request.query(&[("classId", id)]);
                }
                if let Some(loader) = query.loaders.first() {
                    let id = match loader.as_str() {
                        "forge" => "1",
                        "fabric" => "4",
                        "quilt" => "5",
                        "neoforge" => "6",
                        _ => {
                            return Err(OnlineError::Invalid(format!(
                                "unsupported CurseForge loader `{loader}`"
                            )));
                        }
                    };
                    request = request.query(&[("modLoaderType", id)]);
                }
                let response = self.send(request).await?;
                let value: CfEnvelope<Vec<CfMod>> = response
                    .json()
                    .await
                    .map_err(|e| OnlineError::Request(e.to_string()))?;
                Ok(project_hits_from_curseforge(value, provider))
            }
        }
    }

    /// List provider versions for a project, optionally filtered by game version and loader.
    pub async fn versions(
        &self,
        provider: Provider,
        project_id: &str,
        game_version: Option<&str>,
        loader: Option<&str>,
    ) -> Result<Vec<VersionHit>, OnlineError> {
        self.online()?;
        match provider {
            Provider::Modrinth => {
                let mut request = self.client.get(format!(
                    "{}/project/{}/version",
                    self.modrinth_api,
                    encode(project_id)
                ));
                if let Some(v) = game_version {
                    request = request.query(&[("game_versions", serde_json::to_string(&[v])?)]);
                }
                if let Some(l) = loader {
                    request = request.query(&[("loaders", serde_json::to_string(&[l])?)]);
                }
                let response = self.send(request).await?;
                let values: Vec<MrVersion> = response
                    .json()
                    .await
                    .map_err(|e| OnlineError::Request(e.to_string()))?;
                Ok(values.into_iter().map(from_mr_version).collect())
            }
            Provider::CurseForge => {
                let id = project_id.parse::<u64>().map_err(|_| {
                    OnlineError::Invalid("CurseForge project ID must be numeric".into())
                })?;
                let mut request = self
                    .client
                    .get(format!("{}/mods/{id}/files", self.curseforge_api))
                    .query(&[("pageSize", "50")]);
                request = self.authorize_curseforge(request)?;
                if let Some(game) = game_version {
                    request = request.query(&[("gameVersion", game)]);
                }
                if let Some(loader) = loader {
                    let kind = match loader {
                        "forge" => 1,
                        "fabric" => 4,
                        "quilt" => 5,
                        "neoforge" => 6,
                        _ => {
                            return Err(OnlineError::Invalid(format!(
                                "CurseForge has no loader filter for {loader}"
                            )));
                        }
                    };
                    request = request.query(&[("modLoaderType", kind)]);
                }

                let response = self.send(request).await?;
                let files: CfEnvelope<Vec<CfVersion>> = response
                    .json()
                    .await
                    .map_err(|e| OnlineError::Request(e.to_string()))?;
                Ok(files
                    .data
                    .into_iter()
                    .map(|f| from_cf_version(project_id, f))
                    .filter(|v| {
                        game_version.is_none_or(|g| v.game_versions.iter().any(|x| x == g))
                            && loader.is_none_or(|l| v.loaders.iter().any(|x| x == l))
                    })
                    .collect())
            }
        }
    }

    /// Resolve required Modrinth project dependencies to the latest compatible versions.
    pub async fn resolve_dependencies(
        &self,
        project_id: &str,
        game_version: &str,
        loader: &str,
    ) -> Result<Vec<VersionHit>, OnlineError> {
        let mut todo = vec![(project_id.to_owned(), None::<String>)];
        let mut seen = std::collections::BTreeSet::new();
        let mut resolved = Vec::new();
        while let Some((id, pinned)) = todo.pop() {
            if !seen.insert((id.clone(), pinned.clone())) {
                continue;
            }
            if let Some(existing) = resolved.iter().find(|v: &&VersionHit| v.project_id == id) {
                if pinned.as_ref().is_some_and(|pin| pin != &existing.id) {
                    return Err(OnlineError::Invalid(format!(
                        "dependency {id} requires conflicting pinned versions"
                    )));
                }
                continue;
            }
            let version = if let Some(version_id) = pinned {
                let version = self.modrinth_version_by_id(&version_id).await?;
                if version.project_id != id
                    || !version.game_versions.iter().any(|v| v == game_version)
                    || !version.loaders.iter().any(|l| l == loader)
                {
                    return Err(OnlineError::Invalid(format!(
                        "pinned version {version_id} is incompatible with {game_version}/{loader}"
                    )));
                }
                version
            } else {
                self.versions(Provider::Modrinth, &id, Some(game_version), Some(loader))
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        OnlineError::Invalid(format!(
                            "no compatible version found for project {id}"
                        ))
                    })?
            };
            for requirement in version
                .dependency_requirements
                .iter()
                .filter(|d| d.kind == DependencyKind::Required)
            {
                let (dep_id, dep_version) = if let Some(project) = &requirement.project_id {
                    (project.clone(), requirement.version_id.clone())
                } else if let Some(version_id) = &requirement.version_id {
                    let pinned = self.modrinth_version_by_id(version_id).await?;
                    (pinned.project_id, Some(version_id.clone()))
                } else {
                    continue;
                };
                todo.push((dep_id, dep_version));
            }
            resolved.push(version);
        }
        Ok(resolved)
    }

    /// Resolve the required dependency graph for a selected compatible provider version.
    /// When `version_id` is absent, the newest compatible result is selected just as in install.
    pub async fn resolve_project_version(
        &self,
        provider: Provider,
        project_id: &str,
        version_id: Option<&str>,
        game_version: &str,
        loader: Option<&str>,
    ) -> Result<Vec<VersionHit>, OnlineError> {
        self.resolve_project_version_with_context(
            provider,
            project_id,
            version_id,
            game_version,
            loader,
            None,
        )
        .await
    }

    async fn resolve_project_version_with_context(
        &self,
        provider: Provider,
        project_id: &str,
        version_id: Option<&str>,
        game_version: &str,
        loader: Option<&str>,
        context: Option<&TaskContext>,
    ) -> Result<Vec<VersionHit>, OnlineError> {
        check_context_ref(context)?;
        let root = await_or_cancel(
            self.versions(provider, project_id, Some(game_version), loader),
            context,
        )
        .await?
        .into_iter()
        .find(|version| version_id.is_none_or(|id| version.id == id))
        .ok_or_else(|| {
            OnlineError::Invalid("no compatible requested provider version was found".into())
        })?;
        if loader.is_none() && !root.loaders.is_empty() {
            return Err(OnlineError::Invalid(
                "the target instance has no mod loader, but the selected project requires one"
                    .into(),
            ));
        }
        match provider {
            Provider::Modrinth => {
                self.resolve_graph_from_context(root, game_version, loader.unwrap_or(""), context)
                    .await
            }
            Provider::CurseForge => {
                self.resolve_curseforge_graph_from_context(root, game_version, loader, context)
                    .await
            }
        }
    }

    async fn modrinth_version_by_id(&self, version_id: &str) -> Result<VersionHit, OnlineError> {
        let response = self
            .send(self.client.get(format!(
                "{}/version/{}",
                self.modrinth_api,
                encode(version_id)
            )))
            .await?;
        let value: MrVersion = response
            .json()
            .await
            .map_err(|e| OnlineError::Request(e.to_string()))?;
        Ok(from_mr_version(value))
    }

    /// Resolve CurseForge required dependencies using the same version filters as the root project.
    pub async fn resolve_curseforge_dependencies(
        &self,
        project_id: &str,
        game_version: &str,
        loader: Option<&str>,
    ) -> Result<Vec<VersionHit>, OnlineError> {
        let mut todo = vec![project_id.to_owned()];
        let mut seen = std::collections::BTreeSet::new();
        let mut resolved = Vec::new();
        while let Some(id) = todo.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let versions = self
                .versions(Provider::CurseForge, &id, Some(game_version), loader)
                .await?;
            let version = versions.into_iter().next().ok_or_else(|| {
                OnlineError::Invalid(format!(
                    "no compatible CurseForge version found for project {id}"
                ))
            })?;
            todo.extend(
                version
                    .dependencies
                    .iter()
                    .filter(|dependency| !seen.contains(*dependency))
                    .cloned(),
            );
            resolved.push(version)
        }
        Ok(resolved)
    }

    /// Check Modrinth project IDs against the latest compatible release, returning only updates.
    pub async fn check_updates(
        &self,
        installed: &[(String, String)],
        game_version: &str,
        loader: &str,
    ) -> Result<Vec<(String, VersionHit)>, OnlineError> {
        let mut updates = Vec::new();
        for (project_id, installed_version) in installed {
            let available = self
                .versions(
                    Provider::Modrinth,
                    project_id,
                    Some(game_version),
                    Some(loader),
                )
                .await?
                .into_iter()
                .filter(|v| {
                    v.game_versions.iter().any(|g| g == game_version)
                        && v.loaders.iter().any(|l| l == loader)
                })
                .collect::<Vec<_>>();
            if let Some(latest) = latest_update(available, installed_version) {
                updates.push((project_id.clone(), latest));
            }
        }
        Ok(updates)
    }

    /// Inspect an instance's online receipt and return compatible newer provider versions.
    pub async fn check_instance_updates(
        &self,
        root: PathBuf,
        config: &DownloadConfig,
        instance_id: &str,
    ) -> Result<Vec<(OnlineInstalledVersion, VersionHit)>, OnlineError> {
        let instances = InstanceService::new(root, config.clone())?;
        let game = instances.game_directory(instance_id)?;
        let receipt_path = game.join(ONLINE_RECEIPT);
        let bytes = fs::read(&receipt_path).map_err(|e| OnlineError::Io(e.to_string()))?;
        let receipt: OnlineReceipt = serde_json::from_slice(&bytes)?;
        let metadata = instances.metadata(instance_id)?;
        let loader = metadata.loader.map(|kind| match kind {
            crate::loaders::LoaderKind::Fabric => "fabric",
            crate::loaders::LoaderKind::Quilt => "quilt",
            crate::loaders::LoaderKind::LegacyFabric => "legacy-fabric",
            crate::loaders::LoaderKind::OptiFine => "optifine",
            crate::loaders::LoaderKind::Forge => "forge",
            crate::loaders::LoaderKind::NeoForge => "neoforge",
        });
        let mut updates = Vec::new();
        for installed in receipt.versions {
            let installed_loader = loader.or_else(|| {
                installed
                    .metadata
                    .as_ref()
                    .and_then(|m| m.loaders.first().map(String::as_str))
            });
            let versions = self
                .versions(
                    installed.provider,
                    &installed.project_id,
                    Some(&metadata.minecraft),
                    installed_loader,
                )
                .await?;
            if let Some(latest) = latest_update(versions, &installed.version_id) {
                updates.push((installed, latest));
            }
        }
        Ok(updates)
    }

    /// Return compatible updates from the instance's online receipt.
    pub async fn updates_for_instance(
        &self,
        root: PathBuf,
        config: &DownloadConfig,
        instance_id: &str,
    ) -> Result<Vec<(OnlineInstalledVersion, VersionHit)>, OnlineError> {
        self.check_instance_updates(root, config, instance_id).await
    }

    /// Analyze project, dependency, and file conflicts for versions recorded in the instance receipt.
    pub async fn analyze_instance(
        &self,
        root: PathBuf,
        config: &DownloadConfig,
        instance_id: &str,
    ) -> Result<Vec<ContentConflict>, OnlineError> {
        let instances = InstanceService::new(root, config.clone())?;
        let game = instances.game_directory(instance_id)?;
        let receipt: OnlineReceipt = serde_json::from_slice(
            &fs::read(game.join(ONLINE_RECEIPT)).map_err(|e| OnlineError::Io(e.to_string()))?,
        )?;
        let metadata = instances.metadata(instance_id)?;
        let mut selected = Vec::new();
        for installed in receipt.versions {
            let versions = if let Some(cached) = installed.metadata.clone() {
                vec![cached]
            } else {
                self.versions(
                    installed.provider,
                    &installed.project_id,
                    Some(&metadata.minecraft),
                    None,
                )
                .await?
            };
            if let Some(version) = versions.into_iter().find(|v| v.id == installed.version_id) {
                selected.push(version)
            }
        }
        Ok(Self::analyze_conflicts(&selected))
    }

    /// Report paths claimed by multiple selected versions with different content hashes.
    pub fn analyze_conflicts(versions: &[VersionHit]) -> Vec<ContentConflict> {
        let mut paths: std::collections::BTreeMap<String, Vec<(String, String)>> =
            std::collections::BTreeMap::new();
        for version in versions {
            for (project, pin) in &version.pinned_dependencies {
                if versions.iter().any(|selected| {
                    selected.provider == version.provider
                        && selected.project_id == *project
                        && selected.id != *pin
                }) {
                    paths
                        .entry(format!("pinned-version:{project}"))
                        .or_default()
                        .push((version.id.clone(), pin.clone()));
                }
            }
            let required = version.dependencies.iter().cloned().chain(
                version
                    .dependency_requirements
                    .iter()
                    .filter(|d| d.kind == DependencyKind::Required)
                    .filter_map(|d| {
                        d.project_id.clone().or_else(|| {
                            d.version_id
                                .as_ref()
                                .and_then(|id| {
                                    versions
                                        .iter()
                                        .find(|v| v.provider == version.provider && v.id == *id)
                                        .map(|v| v.project_id.clone())
                                })
                                .or_else(|| {
                                    d.version_id
                                        .as_ref()
                                        .map(|id| format!("missing-version:{id}"))
                                })
                        })
                    }),
            );
            for dependency in required {
                if !versions.iter().any(|selected| {
                    selected.provider == version.provider && selected.project_id == dependency
                }) {
                    paths
                        .entry(format!("missing-required:{dependency}"))
                        .or_default()
                        .push((version.version_number.clone(), version.id.clone()));
                }
            }
            for requirement in version
                .dependency_requirements
                .iter()
                .filter(|d| d.kind == DependencyKind::Required && d.version_id.is_some())
            {
                let Some(project) = requirement.project_id.as_ref() else {
                    continue;
                };
                let pin = requirement.version_id.as_ref().unwrap();
                if versions.iter().any(|selected| {
                    selected.provider == version.provider
                        && selected.project_id == *project
                        && selected.id != *pin
                }) {
                    paths
                        .entry(format!("pinned-version:{project}"))
                        .or_default()
                        .push((version.version_number.clone(), pin.clone()));
                }
            }
            let incompatible = version.incompatible_dependencies.iter().cloned().chain(
                version
                    .dependency_requirements
                    .iter()
                    .filter(|d| d.kind == DependencyKind::Incompatible)
                    .filter_map(|d| {
                        d.project_id.clone().or_else(|| {
                            d.version_id
                                .as_ref()
                                .and_then(|id| {
                                    versions
                                        .iter()
                                        .find(|v| v.provider == version.provider && v.id == *id)
                                        .map(|v| v.project_id.clone())
                                })
                                .or_else(|| {
                                    d.version_id
                                        .as_ref()
                                        .map(|id| format!("missing-version:{id}"))
                                })
                        })
                    }),
            );
            for dependency in incompatible {
                if versions.iter().any(|selected| {
                    selected.provider == version.provider && selected.project_id == dependency
                }) {
                    paths
                        .entry(format!("incompatible:{dependency}"))
                        .or_default()
                        .push((version.version_number.clone(), version.id.clone()));
                }
            }
            for file in &version.files {
                let digest = file
                    .sha512
                    .as_ref()
                    .or(file.sha1.as_ref())
                    .cloned()
                    .unwrap_or_else(|| format!("unverified:{}:{}", version.project_id, version.id));
                paths
                    .entry(file.filename.to_ascii_lowercase())
                    .or_default()
                    .push((version.version_number.clone(), digest));
            }
        }
        let mut project_versions: std::collections::BTreeMap<
            (Provider, &str),
            std::collections::BTreeSet<&str>,
        > = std::collections::BTreeMap::new();
        for version in versions {
            project_versions
                .entry((version.provider, &version.project_id))
                .or_default()
                .insert(&version.id);
        }
        for ((provider, project), ids) in project_versions {
            if ids.len() > 1 {
                paths
                    .entry(format!("multiple-versions:{project}"))
                    .or_default()
                    .extend(
                        versions
                            .iter()
                            .filter(|v| v.provider == provider && v.project_id == project)
                            .map(|v| (v.version_number.clone(), v.id.clone())),
                    );
            }
        }
        paths
            .into_iter()
            .filter_map(|(path, claims)| {
                let hashes: std::collections::BTreeSet<_> =
                    claims.iter().map(|(_, hash)| hash).collect();
                ((path.starts_with("missing-required:")
                    || path.starts_with("incompatible:")
                    || path.starts_with("multiple-versions:")
                    || path.starts_with("pinned-version:")
                    || path.starts_with("missing-version:"))
                    || (claims.len() > 1 && hashes.len() > 1))
                    .then(|| ContentConflict {
                        path,
                        versions: claims.into_iter().map(|(version, _)| version).collect(),
                    })
            })
            .collect()
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, OnlineError> {
        self.online()?;
        let response = request
            .send()
            .await
            .map_err(|e| OnlineError::Request(e.to_string()))?;
        if !response.status().is_success() {
            return Err(OnlineError::Http(response.status().as_u16()));
        }
        Ok(response)
    }
    fn authorize_curseforge(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, OnlineError> {
        let key = self
            .config
            .curseforge_api_key
            .as_deref()
            .filter(|k| !k.trim().is_empty())
            .ok_or(OnlineError::MissingCurseForgeKey)?;
        Ok(request.header("x-api-key", key))
    }
    fn online(&self) -> Result<(), OnlineError> {
        if self.config.offline {
            Err(OnlineError::Offline)
        } else {
            Ok(())
        }
    }
}

pub(crate) fn curseforge_download_hosts() -> Vec<String> {
    [
        "edge.forgecdn.net",
        "mediafilez.forgecdn.net",
        "mediafiles.forgecdn.net",
        "media.forgecdn.net",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
fn validate_mod_name(name: &str) -> Result<(), OnlineError> {
    if !name.to_ascii_lowercase().ends_with(".jar") || name.contains(['/', '\\']) {
        return Err(OnlineError::Invalid(format!("unsafe mod filename: {name}")));
    }
    crate::modpack_format::validate_content_path(&format!("mods/{name}"))
        .map_err(|e| OnlineError::Invalid(e.to_string()))
}
fn file_sha512(path: &std::path::Path) -> Result<String, OnlineError> {
    use std::io::Read;
    let mut file = fs::File::open(path).map_err(|e| OnlineError::Io(e.to_string()))?;
    let mut digest = Sha512::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| OnlineError::Io(e.to_string()))?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn latest_update(mut versions: Vec<VersionHit>, installed: &str) -> Option<VersionHit> {
    versions
        .drain(..)
        .next()
        .filter(|latest| latest.id != installed && latest.version_number != installed)
}
fn apply_downloads(
    root: PathBuf,
    config: &DownloadConfig,
    id: &str,
    records: Vec<OnlineInstalledVersion>,
    downloads: &[(String, PathBuf, String)],
    context: Option<&TaskContext>,
) -> Result<(), OnlineError> {
    let instances = InstanceService::new(root, config.clone())?;
    instances.edit_content(id, |game| -> Result<(), OnlineError> {
        check_context_ref(context)?;
        let receipt_path = game.join(ONLINE_RECEIPT);
        let old = match fs::read(&receipt_path) {
            Ok(bytes) => serde_json::from_slice::<OnlineReceipt>(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => OnlineReceipt::default(),
            Err(e) => return Err(OnlineError::Io(e.to_string())),
        };
        if old.schema > 1 {
            return Err(OnlineError::Invalid(
                "unsupported online receipt schema".into(),
            ));
        }
        for name in old.owned_files.keys() {
            validate_mod_name(name)?;
        }
        let mut retained = old
            .versions
            .into_iter()
            .filter(|v| {
                !records
                    .iter()
                    .any(|n| n.provider == v.provider && n.project_id == v.project_id)
            })
            .collect::<Vec<_>>();
        retained.extend(records);
        let metadata = retained
            .iter()
            .filter_map(|v| v.metadata.clone())
            .collect::<Vec<_>>();
        let conflicts = OnlineContentService::analyze_conflicts(&metadata);
        if !conflicts.is_empty() {
            return Err(OnlineError::Conflicts(conflicts));
        }
        let mods = game.join("mods");
        fs::create_dir_all(&mods).map_err(|e| OnlineError::Io(e.to_string()))?;
        let mut desired = std::collections::BTreeMap::new();
        let mut folded = std::collections::BTreeMap::new();
        for record in &retained {
            for (name, hash) in &record.files {
                validate_mod_name(name)?;
                if let Some((prior_name, prior_hash)) =
                    folded.insert(name.to_ascii_lowercase(), (name.clone(), hash.clone()))
                    && (prior_hash != *hash || prior_name != *name)
                {
                    return Err(OnlineError::Invalid(format!(
                        "colliding installed filename: {name}"
                    )));
                }
                desired.insert(name.clone(), hash.clone());
            }
        }
        for (name, _, _) in downloads {
            validate_mod_name(name)?;
            for entry in fs::read_dir(&mods).map_err(|e| OnlineError::Io(e.to_string()))? {
                let entry = entry.map_err(|e| OnlineError::Io(e.to_string()))?;
                let existing = entry.file_name().to_string_lossy().into_owned();
                if existing.eq_ignore_ascii_case(name)
                    && (!old.owned_files.contains_key(&existing) || existing != *name)
                {
                    return Err(OnlineError::Invalid(format!(
                        "unmanaged or case-colliding local file: {existing}"
                    )));
                }
            }
        }
        for (name, hash) in &old.owned_files {
            check_context_ref(context)?;
            if desired.get(name) == Some(hash) {
                continue;
            }
            let target = mods.join(name);
            if !target.is_file() || file_sha512(&target)? != *hash {
                return Err(OnlineError::Conflict {
                    path: name.clone(),
                    reason: "owned file was modified or removed since the previous install".into(),
                });
            }
            fs::remove_file(&target).map_err(|e| OnlineError::Io(e.to_string()))?;
        }
        if let Some(context) = context {
            context
                .emit(TaskEvent::Progress {
                    task: "online_content_apply".into(),
                    completed: 1,
                    total: Some(1),
                })
                .map_err(|_| OnlineError::Download(DownloadError::Cancelled))?;
        }
        check_context_ref(context)?;
        for (name, source, hash) in downloads {
            check_context_ref(context)?;
            let target = mods.join(name);
            if target.exists() {
                if file_sha512(&target)? != *hash {
                    return Err(OnlineError::Conflict {
                        path: name.clone(),
                        reason: "local file differs from the selected provider version".into(),
                    });
                }
            } else {
                fs::copy(source, &target).map_err(|e| OnlineError::Io(e.to_string()))?;
                if file_sha512(&target)? != *hash {
                    return Err(OnlineError::Invalid(
                        "download changed before transactional copy".into(),
                    ));
                }
            }
        }
        let receipt = OnlineReceipt {
            schema: 1,
            versions: retained,
            owned_files: desired,
        };
        check_context_ref(context)?;
        fs::create_dir_all(receipt_path.parent().unwrap())
            .map_err(|e| OnlineError::Io(e.to_string()))?;
        fs::write(receipt_path, serde_json::to_vec_pretty(&receipt)?)
            .map_err(|e| OnlineError::Io(e.to_string()))?;
        check_context_ref(context)?;
        Ok(())
    })
}

fn encode(s: &str) -> String {
    urlencoding(s)
}
fn urlencoding(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn modrinth_facets(q: &SearchQuery) -> Vec<Vec<String>> {
    let mut facets = Vec::new();
    if let Some(t) = &q.project_type {
        facets.push(vec![format!("project_type:{t}")]);
    }
    if let Some(v) = &q.game_version {
        facets.push(vec![format!("versions:{v}")]);
    }
    if !q.loaders.is_empty() {
        facets.push(
            q.loaders
                .iter()
                .map(|l| format!("categories:{l}"))
                .collect(),
        );
    }
    facets
}

#[derive(Deserialize)]
struct MrSearch {
    hits: Vec<MrHit>,
}
#[derive(Deserialize)]
struct MrHit {
    project_id: String,
    slug: String,
    title: String,
    description: String,
    downloads: u64,
    icon_url: Option<String>,
}
#[derive(Deserialize)]
struct MrVersion {
    id: String,
    project_id: String,
    name: String,
    version_number: String,
    game_versions: Vec<String>,
    loaders: Vec<String>,
    files: Vec<MrFile>,
    #[serde(default)]
    dependencies: Vec<MrDep>,
}
#[derive(Deserialize)]
struct MrFile {
    filename: String,
    url: String,
    size: u64,
    hashes: std::collections::HashMap<String, String>,
    #[serde(default)]
    primary: bool,
}
#[derive(Deserialize)]
struct MrDep {
    project_id: Option<String>,
    version_id: Option<String>,
    dependency_type: Option<String>,
}

fn from_mr_version(v: MrVersion) -> VersionHit {
    let mut dependencies = Vec::new();
    let mut optional_dependencies = Vec::new();
    let mut incompatible_dependencies = Vec::new();
    let mut pinned_dependencies = std::collections::BTreeMap::new();
    let mut dependency_requirements = Vec::new();
    for dep in v.dependencies {
        let kind = match dep.dependency_type.as_deref() {
            Some("required") => DependencyKind::Required,
            Some("optional") => DependencyKind::Optional,
            Some("incompatible") => DependencyKind::Incompatible,
            Some("embedded") => DependencyKind::Embedded,
            None if dep.project_id.is_some() => DependencyKind::Required,
            None => continue,
            Some(other) => {
                tracing::warn!(
                    dependency_type = other,
                    "unknown Modrinth dependency type ignored"
                );
                continue;
            }
        };
        if let Some(project) = &dep.project_id {
            match kind {
                DependencyKind::Required => dependencies.push(project.clone()),
                DependencyKind::Optional => optional_dependencies.push(project.clone()),
                DependencyKind::Incompatible => incompatible_dependencies.push(project.clone()),
                DependencyKind::Embedded => {}
            }
            if kind == DependencyKind::Required
                && let Some(version) = &dep.version_id
            {
                pinned_dependencies.insert(project.clone(), version.clone());
            }
        }
        dependency_requirements.push(DependencyRequirement {
            project_id: dep.project_id,
            version_id: dep.version_id,
            kind,
        });
    }
    VersionHit {
        provider: Provider::Modrinth,
        id: v.id,
        project_id: v.project_id,
        name: v.name,
        version_number: v.version_number,
        game_versions: v.game_versions,
        loaders: v.loaders,
        files: v
            .files
            .into_iter()
            .map(|f| VersionFile {
                filename: f.filename,
                url: f.url,
                size: f.size,
                sha1: f.hashes.get("sha1").cloned(),
                sha512: f.hashes.get("sha512").cloned(),
                primary: f.primary,
            })
            .collect(),
        dependencies,
        optional_dependencies,
        incompatible_dependencies,
        pinned_dependencies,
        dependency_requirements,
    }
}
#[derive(Deserialize)]
struct CfEnvelope<T> {
    data: T,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfVersion {
    id: u64,
    display_name: String,
    file_name: String,
    file_length: u64,
    download_url: Option<String>,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    hashes: Vec<CfHash>,
    #[serde(default)]
    dependencies: Vec<CfVersionDependency>,
}
#[derive(Deserialize)]
struct CfHash {
    value: String,
    algo: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfVersionDependency {
    mod_id: u64,
    relation_type: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfMod {
    id: u64,
    name: String,
    slug: String,
    summary: String,
    download_count: u64,
    logo: Option<CfLogo>,
}
#[derive(Deserialize)]
struct CfLogo {
    url: String,
}

fn project_hits_from_modrinth(value: MrSearch, provider: Provider) -> Vec<ProjectHit> {
    value
        .hits
        .into_iter()
        .map(|h| ProjectHit {
            provider,
            id: h.project_id,
            slug: h.slug,
            title: h.title,
            description: h.description,
            downloads: h.downloads,
            icon_url: h.icon_url,
        })
        .collect()
}
fn project_hits_from_curseforge(
    value: CfEnvelope<Vec<CfMod>>,
    provider: Provider,
) -> Vec<ProjectHit> {
    value
        .data
        .into_iter()
        .map(|h| ProjectHit {
            provider,
            id: h.id.to_string(),
            slug: h.slug,
            title: h.name,
            description: h.summary,
            downloads: h.download_count,
            icon_url: h.logo.map(|x| x.url),
        })
        .collect()
}
fn from_cf_version(project_id: &str, f: CfVersion) -> VersionHit {
    let mut game_versions = Vec::new();
    let mut loaders = Vec::new();
    for value in f.game_versions {
        if value.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            game_versions.push(value)
        } else if ["fabric", "forge", "neoforge", "quilt"]
            .contains(&value.to_ascii_lowercase().as_str())
        {
            loaders.push(value.to_ascii_lowercase())
        }
    }
    let (mut dependencies, mut optional_dependencies, mut incompatible_dependencies) =
        (Vec::new(), Vec::new(), Vec::new());
    for dependency in f.dependencies {
        match dependency.relation_type {
            3 => dependencies.push(dependency.mod_id.to_string()),
            2 => optional_dependencies.push(dependency.mod_id.to_string()),
            5 => incompatible_dependencies.push(dependency.mod_id.to_string()),
            _ => {}
        }
    }
    VersionHit {
        provider: Provider::CurseForge,
        id: f.id.to_string(),
        project_id: project_id.into(),
        name: f.display_name.clone(),
        version_number: f.display_name,
        game_versions,
        loaders,
        files: f
            .download_url
            .into_iter()
            .map(|url| VersionFile {
                filename: f.file_name.clone(),
                url,
                size: f.file_length,
                sha1: f
                    .hashes
                    .iter()
                    .find(|h| h.algo == 1)
                    .map(|h| h.value.clone()),
                sha512: None,
                primary: true,
            })
            .collect(),
        dependencies,
        optional_dependencies,
        incompatible_dependencies,
        pinned_dependencies: Default::default(),
        dependency_requirements: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn facets_keep_loader_choices_as_alternatives() {
        let q = SearchQuery {
            query: "map".into(),
            game_version: Some("1.20.1".into()),
            loaders: vec!["fabric".into(), "forge".into()],
            project_type: Some("mod".into()),
            limit: 20,
        };
        let f = modrinth_facets(&q);
        assert!(f.contains(&vec!["categories:fabric".into(), "categories:forge".into()]));
        assert!(f.contains(&vec!["versions:1.20.1".into()]));
    }
    #[test]
    fn encoded_path_cannot_change_api_path() {
        assert_eq!(encode("abc/def"), "abc%2Fdef");
    }

    #[tokio::test]
    #[ignore = "requires access to the public Modrinth API"]
    async fn public_modrinth_fabric_api_project_has_compatible_versions() {
        let client = OnlineContentService::new(OnlineConfig::default()).unwrap();
        let hits = client
            .search(
                Provider::Modrinth,
                &SearchQuery {
                    query: "Fabric API".into(),
                    game_version: Some("1.20.1".into()),
                    loaders: vec!["fabric".into()],
                    project_type: Some("mod".into()),
                    limit: 10,
                },
            )
            .await
            .unwrap();
        let fabric_api = hits.iter().find(|hit| hit.slug == "fabric-api").unwrap();
        let versions = client
            .versions(
                Provider::Modrinth,
                &fabric_api.id,
                Some("1.20.1"),
                Some("fabric"),
            )
            .await
            .unwrap();
        assert!(versions.iter().any(|version| {
            version.game_versions.iter().any(|game| game == "1.20.1")
                && version.loaders.iter().any(|loader| loader == "fabric")
        }));
    }
}

#[cfg(test)]
#[path = "online_content_tests.rs"]
mod regression_tests;
