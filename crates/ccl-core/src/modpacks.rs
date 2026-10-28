//! Transactional Modrinth pack installation and portable export.

use crate::{
    TaskContext,
    config::DownloadConfig,
    download::{DownloadError, DownloadManager, DownloadRequest},
    install::{InstallError, InstallOptions, InstallService},
    instances::{InstanceError, InstanceMetadata, InstanceService},
    loaders::{LoaderError, LoaderKind, LoaderRequest, LoaderService},
    modpack_format::{self, FormatError, Manifest},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use thiserror::Error;

const RECEIPT: &str = ".ccl/modpack.json";

#[derive(Debug, Error)]
pub enum ModpackError {
    #[error(transparent)]
    Format(#[from] FormatError),
    #[error(transparent)]
    Instance(#[from] InstanceError),
    #[error(transparent)]
    Install(#[from] InstallError),
    #[error(transparent)]
    Loader(#[from] LoaderError),
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error("modpack conflict at {path}: {reason}")]
    Conflict { path: String, reason: String },
    #[error("modpack file operation at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid modpack record: {0}")]
    Record(#[from] serde_json::Error),
    #[error("operation cancelled")]
    Cancelled,
}

impl ModpackError {
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self,
            Self::Cancelled
                | Self::Download(DownloadError::Cancelled)
                | Self::Install(
                    InstallError::Cancelled | InstallError::Download(DownloadError::Cancelled)
                )
        ) || matches!(self, Self::Instance(e) if e.is_cancelled())
            || matches!(self, Self::Loader(e) if e.is_cancelled())
    }
}

#[derive(Clone, Debug, Default)]
pub struct ImportOptions {
    pub include_optional: bool,
    /// Explicitly replace locally modified pack-owned files on update.
    pub replace_modified: bool,
    /// Additional exact HTTPS hosts explicitly trusted by the caller.
    pub allowed_hosts: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackReceipt {
    pub schema: u32,
    pub manifest: Manifest,
    pub owned_files: BTreeMap<String, String>,
    pub include_optional: bool,
}

#[derive(Clone)]
pub struct ModpackService {
    instances: InstanceService,
    config: DownloadConfig,
    runtime_subdirectory: Option<PathBuf>,
    #[cfg(test)]
    prepared: bool,
}

impl ModpackService {
    pub fn new(root: PathBuf, config: DownloadConfig) -> Result<Self, ModpackError> {
        Ok(Self {
            instances: InstanceService::new(root, config.clone())?,
            config,
            runtime_subdirectory: None,
            #[cfg(test)]
            prepared: false,
        })
    }

    pub fn new_with_runtime_subdirectory(
        root: PathBuf,
        config: DownloadConfig,
        relative: PathBuf,
    ) -> Result<Self, ModpackError> {
        let (instances, runtime_subdirectory) = if relative.as_os_str().is_empty() {
            (InstanceService::new(root, config.clone())?, None)
        } else {
            (
                InstanceService::new(root, config.clone())?
                    .with_runtime_subdirectory(relative.clone())?,
                Some(relative),
            )
        };
        Ok(Self {
            instances,
            config,
            runtime_subdirectory,
            #[cfg(test)]
            prepared: false,
        })
    }

    #[cfg(test)]
    pub(crate) fn new_prepared_with_runtime_subdirectory(
        root: PathBuf,
        config: DownloadConfig,
        relative: PathBuf,
    ) -> Result<Self, ModpackError> {
        let mut service = Self::new_with_runtime_subdirectory(root, config, relative)?;
        service.prepared = true;
        Ok(service)
    }

    pub fn inspect(archive: &Path) -> Result<Manifest, ModpackError> {
        Ok(modpack_format::read_pack(archive)?.manifest)
    }

    pub fn show(&self, id: &str) -> Result<PackReceipt, ModpackError> {
        let guard = self.instances.acquire_launch(id)?;
        let game = self.instances.runtime_path_for(guard.game_directory());
        read_receipt(&game)?.ok_or_else(|| conflict(id, "instance has no imported modpack"))
    }

    pub async fn import(
        &self,
        id: &str,
        archive: &Path,
        options: &ImportOptions,
        context: Option<TaskContext>,
    ) -> Result<InstanceMetadata, ModpackError> {
        self.import_inner(
            id,
            archive,
            options,
            context,
            crate::instances::PackTransactionMode::Upsert,
        )
        .await
    }

    /// Update an existing pack only; a missing ID is rejected while holding the instance lock.
    pub async fn import_existing(
        &self,
        id: &str,
        archive: &Path,
        options: &ImportOptions,
        context: Option<TaskContext>,
    ) -> Result<InstanceMetadata, ModpackError> {
        self.import_inner(
            id,
            archive,
            options,
            context,
            crate::instances::PackTransactionMode::UpdateExisting,
        )
        .await
    }

    /// Import a pack into a new managed instance, rejecting existing IDs under the instance lock.
    pub async fn import_new(
        &self,
        id: &str,
        archive: &Path,
        options: &ImportOptions,
        context: Option<TaskContext>,
    ) -> Result<InstanceMetadata, ModpackError> {
        self.import_inner(
            id,
            archive,
            options,
            context,
            crate::instances::PackTransactionMode::CreateOnly,
        )
        .await
    }

    async fn import_inner(
        &self,
        id: &str,
        archive: &Path,
        options: &ImportOptions,
        context: Option<TaskContext>,
        mode: crate::instances::PackTransactionMode,
    ) -> Result<InstanceMetadata, ModpackError> {
        check(&context)?;
        let pack = modpack_format::read_pack(archive)?;
        for value in pack.manifest.dependencies.values() {
            if value.is_empty()
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
            {
                return Err(conflict(
                    value,
                    "dependency version must be a safe version identifier",
                ));
            }
        }
        for file in &pack.manifest.files {
            for url in &file.downloads {
                modpack_format::validate_download_url(url, &options.allowed_hosts)?;
            }
        }
        let selected: Vec<_> = pack
            .manifest
            .files
            .iter()
            .filter(|f| f.client_required(options.include_optional))
            .cloned()
            .collect();
        let mut desired: BTreeMap<String, String> = selected
            .iter()
            .map(|file| {
                (
                    file.path.clone(),
                    file.hashes["sha512"].to_ascii_lowercase(),
                )
            })
            .collect();
        for (path, bytes) in &pack.overrides {
            desired.insert(path.clone(), hash(bytes));
        }
        let fingerprint = hash(&serde_json::to_vec(&(
            &pack.manifest,
            &desired,
            options.include_optional,
            options.replace_modified,
            &options.allowed_hosts,
        ))?);
        let config = self.config.clone();
        let runtime_subdirectory = self.runtime_subdirectory.clone();
        self.instances
            .transact_pack(id, &fingerprint, mode, |stage, current| async move {
                check(&context)?;
                let runtime = runtime_subdirectory
                    .as_ref()
                    .map(|relative| stage.join(relative))
                    .unwrap_or_else(|| stage.clone());
                if let Some(current) = &current {
                    let current_runtime = runtime_subdirectory
                        .as_ref()
                        .map(|relative| current.game_directory.join(relative))
                        .unwrap_or_else(|| current.game_directory.clone());
                    let old = read_receipt(&current_runtime)?;
                    if old.is_none() {
                        return Err(conflict(
                            id,
                            "existing instance is not a managed modpack; import into a new ID",
                        ));
                    }
                    check_update(
                        &current_runtime,
                        &old.unwrap().owned_files,
                        &desired,
                        options.replace_modified,
                    )?;
                }
                // Remove unchanged files removed by the new pack, within the private stage only.
                if let Some(old) = read_receipt(&runtime)? {
                    for path in old
                        .owned_files
                        .keys()
                        .filter(|path| !desired.contains_key(*path))
                    {
                        let target = content_target(&runtime, path)?;
                        if target.is_file() {
                            fs::remove_file(&target).map_err(|e| io(&target, e))?;
                        }
                    }
                }
                let minecraft = pack.manifest.dependencies["minecraft"].clone();
                let loader = loader_request(&pack.manifest);
                let loader_version = self
                    .prepare_game(&stage, &minecraft, loader.as_ref(), context.clone())
                    .await?;
                let downloader = DownloadManager::new(&config)?;
                let mut hosts: Vec<String> = [
                    "cdn.modrinth.com",
                    "github.com",
                    "raw.githubusercontent.com",
                    "gitlab.com",
                    "release-assets.githubusercontent.com",
                    "objects.githubusercontent.com",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect();
                hosts.extend(options.allowed_hosts.clone());
                for file in selected {
                    check(&context)?;
                    let destination = content_target(&runtime, &file.path)?;
                    let mut request = DownloadRequest::new(file.downloads, &destination)
                        .with_sha1(&file.hashes["sha1"])
                        .with_sha512(&file.hashes["sha512"])
                        .with_size(file.file_size)
                        .with_allowed_hosts(hosts.clone());
                    if let Some(context) = &context {
                        request = request.with_task_context(context.clone());
                    }
                    downloader.download(request).await?;
                }
                for (path, bytes) in &pack.overrides {
                    check(&context)?;
                    let target = content_target(&runtime, path)?;
                    fs::create_dir_all(target.parent().unwrap()).map_err(|e| io(&target, e))?;
                    fs::write(&target, bytes).map_err(|e| io(&target, e))?;
                }
                let receipt = PackReceipt {
                    schema: 1,
                    manifest: pack.manifest,
                    owned_files: desired,
                    include_optional: options.include_optional,
                };
                let receipt_path = runtime.join(RECEIPT);
                fs::create_dir_all(receipt_path.parent().unwrap())
                    .map_err(|e| io(&receipt_path, e))?;
                fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt)?)
                    .map_err(|e| io(&receipt_path, e))?;
                check(&context)?;
                let game_directory = current
                    .as_ref()
                    .map(|m| m.game_directory.clone())
                    .unwrap_or_else(|| stage.with_file_name("game"));
                let metadata = InstanceMetadata {
                    id: id.into(),
                    minecraft,
                    loader: loader.map(|l| l.kind),
                    loader_version,
                    game_directory,
                };
                Ok((metadata.clone(), metadata))
            })
            .await
    }

    async fn prepare_game(
        &self,
        stage: &Path,
        minecraft: &str,
        loader: Option<&LoaderRequest>,
        context: Option<TaskContext>,
    ) -> Result<Option<String>, ModpackError> {
        // Payload/transaction fixtures use a prepared vanilla tree; actual runtime
        // provisioning is covered by installer suites and physical pack launches.
        #[cfg(test)]
        if self.prepared {
            return Ok(loader.map(|l| format!("{}-{}", l.minecraft, l.version)));
        }
        let options = InstallOptions::for_current_platform(stage);
        if let Some(request) = loader {
            Ok(Some(
                LoaderService::new(self.config.clone())?
                    .install(request, &options, context)
                    .await?,
            ))
        } else {
            InstallService::new(self.config.clone())?
                .install(minecraft, &options, context)
                .await?;
            Ok(None)
        }
    }

    /// Export pinned remote references; local binaries require explicit redistribution consent.
    pub fn export(
        &self,
        id: &str,
        output: &Path,
        include_local: bool,
    ) -> Result<Manifest, ModpackError> {
        let guard = self.instances.acquire_launch(id)?;
        let game = self.instances.runtime_path_for(guard.game_directory());
        let receipt = match read_receipt(&game)? {
            Some(receipt) => receipt,
            None => PackReceipt {
                schema: 1,
                manifest: manifest_for_instance(&guard.metadata)?,
                owned_files: BTreeMap::new(),
                include_optional: false,
            },
        };
        let expected_loader = loader_request(&receipt.manifest);
        if guard.metadata.minecraft != receipt.manifest.dependencies["minecraft"]
            || guard.metadata.loader != expected_loader.as_ref().map(|l| l.kind)
            || expected_loader
                .as_ref()
                .is_some_and(|l| !guard.version().contains(&l.version))
        {
            return Err(conflict(
                id,
                "instance loader changed outside the pack; export would not reproduce the active setup",
            ));
        }
        let mut manifest = receipt.manifest.clone();
        manifest.files.retain(|f| {
            f.client_required(receipt.include_optional) && !private_export_path(&f.path)
        });
        let mut overrides = BTreeMap::new();
        let mut retained = Vec::new();
        for mut file in manifest.files {
            let target = content_target(&game, &file.path)?;
            let bytes = fs::read(&target).map_err(|e| io(&target, e))?;
            if hash(&bytes).eq_ignore_ascii_case(&file.hashes["sha512"]) {
                // Selected optional files become required to reproduce this installation.
                if let Some(env) = &mut file.env {
                    env.client = modpack_format::EnvironmentValue::Required;
                }
                retained.push(file);
            } else if include_local || !binary_content_path(&file.path) {
                overrides.insert(file.path, bytes);
            } else {
                return Err(conflict(
                    &file.path,
                    "file changed; a remote reference cannot reproduce it (use --include-local-content only with redistribution permission)",
                ));
            }
        }
        manifest.files = retained;
        for directory in [
            "mods",
            "resourcepacks",
            "shaderpacks",
            "config",
            "defaultconfigs",
            "datapacks",
        ] {
            gather_export(
                &game,
                Path::new(directory),
                &manifest,
                include_local,
                &mut overrides,
            )?;
        }
        for name in ["options.txt", "servers.dat", "servers.dat_old"] {
            let path = game.join(name);
            if path.exists() {
                if name.starts_with("servers.") {
                    continue;
                } // Private server lists belong in backups.
                overrides.insert(name.into(), fs::read(&path).map_err(|e| io(&path, e))?);
            }
        }
        modpack_format::write_pack(output, &manifest, &overrides)?;
        Ok(manifest)
    }
}

fn loader_request(manifest: &Manifest) -> Option<LoaderRequest> {
    [
        ("fabric-loader", LoaderKind::Fabric),
        ("quilt-loader", LoaderKind::Quilt),
        ("forge", LoaderKind::Forge),
        ("neoforge", LoaderKind::NeoForge),
    ]
    .into_iter()
    .find_map(|(name, kind)| {
        manifest
            .dependencies
            .get(name)
            .map(|version| LoaderRequest {
                kind,
                minecraft: manifest.dependencies["minecraft"].clone(),
                version: version.clone(),
            })
    })
}

fn manifest_for_instance(instance: &InstanceMetadata) -> Result<Manifest, ModpackError> {
    let mut dependencies = BTreeMap::from([("minecraft".into(), instance.minecraft.clone())]);
    if let Some(kind) = instance.loader {
        let profile = instance
            .loader_version
            .as_deref()
            .ok_or_else(|| conflict(&instance.id, "missing installed loader profile"))?;
        let (key, version) = match kind {
            LoaderKind::Fabric => (
                "fabric-loader",
                profile
                    .strip_prefix("fabric-loader-")
                    .and_then(|v| v.strip_suffix(&format!("-{}", instance.minecraft))),
            ),
            LoaderKind::Forge => (
                "forge",
                profile
                    .strip_prefix(&format!("{}-forge-", instance.minecraft))
                    .or_else(|| profile.strip_prefix(&format!("{}-Forge", instance.minecraft))),
            ),
            LoaderKind::NeoForge => ("neoforge", profile.strip_prefix("neoforge-")),
            LoaderKind::Quilt => (
                "quilt-loader",
                profile
                    .strip_prefix("quilt-loader-")
                    .and_then(|v| v.strip_suffix(&format!("-{}", instance.minecraft))),
            ),
            LoaderKind::LegacyFabric | LoaderKind::OptiFine => {
                return Err(conflict(
                    &instance.id,
                    "standard Modrinth format cannot declare Legacy Fabric or standalone OptiFine; use a userdata backup instead",
                ));
            }
        };
        let version = version
            .filter(|v| !v.is_empty())
            .ok_or_else(|| conflict(profile, "cannot determine loader release for export"))?;
        dependencies.insert(key.into(), version.into());
    }
    let manifest = Manifest {
        format_version: 1,
        game: "minecraft".into(),
        version_id: instance.id.clone(),
        name: instance.id.clone(),
        summary: None,
        files: Vec::new(),
        dependencies,
    };
    manifest.validate()?;
    Ok(manifest)
}

fn read_receipt(game: &Path) -> Result<Option<PackReceipt>, ModpackError> {
    let path = game.join(RECEIPT);
    ensure_no_symlinks(game, &path)?;
    match fs::read(&path) {
        Ok(bytes) => {
            let receipt: PackReceipt = serde_json::from_slice(&bytes)?;
            if receipt.schema != 1 {
                return Err(conflict(RECEIPT, "unsupported receipt schema"));
            }
            receipt.manifest.validate()?;
            for (path, digest) in &receipt.owned_files {
                modpack_format::validate_content_path(path)?;
                if digest.len() != 128 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(conflict(path, "invalid receipt hash"));
                }
            }
            Ok(Some(receipt))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(path, e)),
    }
}

fn check_update(
    game: &Path,
    old: &BTreeMap<String, String>,
    desired: &BTreeMap<String, String>,
    replace: bool,
) -> Result<(), ModpackError> {
    for path in old.keys().chain(desired.keys()) {
        let target = content_target(game, path)?;
        let actual = match fs::read(&target) {
            Ok(bytes) => Some(hash(&bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(io(&target, e)),
        };
        let original = old.get(path);
        let next = desired.get(path);
        if actual.as_ref() == next {
            continue;
        }
        let unchanged = actual.as_ref() == original;
        if !unchanged && !replace {
            return Err(conflict(
                path,
                "local content differs from the last installed pack; back up changes or explicitly use --replace-modified",
            ));
        }
    }
    Ok(())
}

fn gather_export(
    game: &Path,
    relative: &Path,
    manifest: &Manifest,
    include_local: bool,
    output: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), ModpackError> {
    let path = game.join(relative);
    ensure_no_symlinks(game, &path)?;
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        for entry in fs::read_dir(&path).map_err(|e| io(&path, e))? {
            let entry = entry.map_err(|e| io(&path, e))?;
            gather_export(
                game,
                &relative.join(entry.file_name()),
                manifest,
                include_local,
                output,
            )?;
        }
    } else {
        let name = relative.to_string_lossy().replace('\\', "/");
        modpack_format::validate_content_path(&name)?;
        if manifest.files.iter().any(|f| f.path == name) {
            return Ok(());
        }
        let binary = binary_content_path(&name);
        if binary && !include_local {
            return Err(conflict(
                &name,
                "local binary has no verified remote reference; --include-local-content requires redistribution permission",
            ));
        }
        output.insert(name, fs::read(&path).map_err(|e| io(&path, e))?);
    }
    Ok(())
}

fn content_target(game: &Path, path: &str) -> Result<PathBuf, ModpackError> {
    modpack_format::validate_content_path(path)?;
    let target = game.join(path);
    ensure_no_symlinks(game, &target)?;
    Ok(target)
}

fn binary_content_path(path: &str) -> bool {
    matches!(
        path.split('/').next(),
        Some("mods" | "resourcepacks" | "shaderpacks")
    )
}

fn private_export_path(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    path.starts_with("saves/")
        || path.starts_with("servers.dat")
        || matches!(path.as_str(), "usercache.json" | "realms_persistence.json")
}
fn ensure_no_symlinks(root: &Path, target: &Path) -> Result<(), ModpackError> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| conflict(&target.display().to_string(), "path escapes instance"))?;
    let mut path = root.to_owned();
    for component in relative.components() {
        path.push(component);
        if let Ok(meta) = fs::symlink_metadata(&path)
            && meta.file_type().is_symlink()
        {
            return Err(conflict(
                &path.display().to_string(),
                "symlink is not allowed",
            ));
        }
    }
    Ok(())
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha512::digest(bytes))
}
fn conflict(path: &str, reason: &str) -> ModpackError {
    ModpackError::Conflict {
        path: path.into(),
        reason: reason.into(),
    }
}
fn io(path: impl Into<PathBuf>, source: std::io::Error) -> ModpackError {
    ModpackError::Io {
        path: path.into(),
        source,
    }
}
fn check(context: &Option<TaskContext>) -> Result<(), ModpackError> {
    if context
        .as_ref()
        .is_some_and(|c| c.cancellation.is_cancelled())
    {
        Err(ModpackError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[path = "modpacks_tests.rs"]
mod tests;
