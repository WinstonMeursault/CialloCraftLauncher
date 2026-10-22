//! Isolated loader instances under a selected Minecraft game root.

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    config::DownloadConfig,
    install::InstallOptions,
    loaders::{LoaderKind, LoaderRequest, LoaderService},
    task::TaskContext,
};

#[derive(Debug, Error)]
pub enum InstanceError {
    #[error("invalid instance ID: {0}")]
    InvalidId(String),
    #[error("instance not found: {0}")]
    NotFound(String),
    #[error("instance is busy or being used by a game process: {0}")]
    Conflict(String),
    #[error("instance metadata is invalid at {path}: {source}")]
    Metadata {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("instance operation failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Install(#[from] crate::install::InstallError),
    #[error(transparent)]
    Loader(#[from] crate::loaders::LoaderError),
    #[error("operation cancelled")]
    Cancelled,
}

impl InstanceError {
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self,
            Self::Cancelled
                | Self::Install(crate::install::InstallError::Cancelled)
                | Self::Install(crate::install::InstallError::Download(
                    crate::download::DownloadError::Cancelled
                ))
                | Self::Loader(crate::loaders::LoaderError::Cancelled)
                | Self::Loader(crate::loaders::LoaderError::Download(
                    crate::download::DownloadError::Cancelled,
                ))
                | Self::Loader(crate::loaders::LoaderError::Install(
                    crate::install::InstallError::Cancelled,
                ))
        )
    }
}

pub type InstanceResult<T> = std::result::Result<T, InstanceError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstanceMetadata {
    pub id: String,
    pub minecraft: String,
    pub loader: Option<LoaderKind>,
    pub loader_version: Option<String>,
    /// Absolute path used as `--gameDir`; mods, config and saves stay here across updates.
    pub game_directory: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CommitJournal {
    next: InstanceMetadata,
    old: Option<InstanceMetadata>,
    had_previous: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RollbackJournal {
    current: InstanceMetadata,
    next: InstanceMetadata,
    target_active: bool,
}

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// Manages independent game directories in `<game root>/.ccl/instances/<id>`.
#[derive(Clone)]
pub struct InstanceService {
    root: PathBuf,
    config: DownloadConfig,
}

impl InstanceService {
    /// Apply an offline content edit as a complete revision, under the launch lock.
    /// Failed edits leave the active revision untouched; rollback restores this snapshot.
    pub fn edit_content<T, E, F>(&self, id: &str, edit: F) -> Result<T, E>
    where
        E: From<InstanceError>,
        F: FnOnce(&Path) -> Result<T, E>,
    {
        let path = self.instance_path(id)?;
        if !path.is_dir() {
            return Err(InstanceError::NotFound(id.into()).into());
        }
        let _lock = InstanceLock::acquire(&path.join(".operation.lock"), id)?;
        recover_instance(&path)?;
        let metadata = read_metadata(&path.join("instance.json"))?;
        let game = path.join("game");
        let stage = path.join(".stage");
        reject_symlink(&stage)?;
        if stage.exists() {
            fs::remove_dir_all(&stage).map_err(|e| io(&stage, e))?;
        }
        clone_game_generation(&game, &stage)?;
        let result = edit(&stage)?;
        mark_content_revision(&stage)?;
        commit_generation(&path, &stage, &game, &path.join("previous"), &metadata)?;
        Ok(result)
    }

    /// Prepare a modpack revision without exposing partial installation to readers.
    pub(crate) async fn transact_pack<T, E, F, Fut>(
        &self,
        id: &str,
        fingerprint: &str,
        operation: F,
    ) -> Result<T, E>
    where
        E: From<InstanceError>,
        F: FnOnce(PathBuf, Option<InstanceMetadata>) -> Fut,
        Fut: std::future::Future<Output = Result<(InstanceMetadata, T), E>>,
    {
        let path = self.instance_path(id)?;
        fs::create_dir_all(&path).map_err(|e| io(&path, e))?;
        let _lock = InstanceLock::acquire(&path.join(".operation.lock"), id)?;
        recover_instance(&path)?;
        let metadata_path = path.join("instance.json");
        reject_symlink(&metadata_path)?;
        let current = if metadata_path.is_file() {
            Some(read_metadata(&metadata_path)?)
        } else {
            None
        };
        let game = path.join("game");
        let stage = path.join(".stage");
        reject_symlink(&stage)?;
        let marker = stage.join(".ccl/pack-request.json");
        reject_symlink(&marker)?;
        let resume = fs::read_to_string(&marker).ok().as_deref() == Some(fingerprint);
        if stage.exists() && !resume {
            fs::remove_dir_all(&stage).map_err(|e| io(&stage, e))?;
        }
        if !resume {
            fs::create_dir_all(&stage).map_err(|e| io(&stage, e))?;
            if game.exists() {
                clone_game_generation(&game, &stage)?;
            }
        } else if game.exists() {
            // Refresh mutable files after an interrupted attempt; downloaded resources remain.
            replace_user_data(&game, &stage)?;
        }
        let marker_directory = stage.join(".ccl");
        reject_symlink(&marker_directory)?;
        fs::create_dir_all(&marker_directory).map_err(|e| io(&marker_directory, e))?;
        atomic_write(&marker, fingerprint.as_bytes())?;
        let (mut next, result) = operation(stage.clone(), current).await?;
        next.id = id.into();
        next.game_directory = game.clone();
        validate_segment(&next.minecraft)?;
        if let Some(version) = &next.loader_version {
            validate_segment(version)?;
        }
        let _ = fs::remove_file(&marker);
        mark_content_revision(&stage)?;
        commit_generation(&path, &stage, &game, &path.join("previous"), &next)?;
        Ok(result)
    }

    pub fn new(root: PathBuf, config: DownloadConfig) -> InstanceResult<Self> {
        let root = absolute_path(root)?;
        Ok(Self { root, config })
    }

    pub fn list(&self) -> InstanceResult<Vec<InstanceMetadata>> {
        let dir = self.instances_dir();
        let mut result = Vec::new();
        reject_symlink(&self.root.join(".ccl"))?;
        reject_symlink(&dir)?;
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
            Err(error) => return Err(io(&dir, error)),
        };
        for entry in entries {
            let entry = entry.map_err(|e| io(&dir, e))?;
            reject_symlink(&entry.path())?;
            if !entry.file_type().map_err(|e| io(entry.path(), e))?.is_dir() {
                continue;
            }
            let instance_lock = entry.path().join(".operation.lock");
            let _lock =
                InstanceLock::acquire(&instance_lock, &entry.file_name().to_string_lossy())?;
            recover_instance(&entry.path())?;
            let path = entry.path().join("instance.json");
            if path.is_file() {
                result.push(read_metadata(&path)?);
            }
        }
        result.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(result)
    }

    pub fn metadata(&self, id: &str) -> InstanceResult<InstanceMetadata> {
        let instance = self.instance_path(id)?;
        if !instance.is_dir() {
            return Err(InstanceError::NotFound(id.into()));
        }
        let _lock = InstanceLock::acquire(&instance.join(".operation.lock"), id)?;
        recover_instance(&instance)?;
        let path = instance.join("instance.json");
        if !path.is_file() {
            return Err(InstanceError::NotFound(id.into()));
        }
        read_metadata(&path)
    }

    pub fn game_directory(&self, id: &str) -> InstanceResult<PathBuf> {
        self.metadata(id).map(|m| m.game_directory)
    }

    /// Hold the instance lock for the complete lifetime of a launch. Mutations use the same
    /// lock, so a running process cannot have its game directory replaced underneath it.
    pub fn acquire_launch(&self, id: &str) -> InstanceResult<InstanceLaunchGuard> {
        let path = self.instance_path(id)?;
        if !path.is_dir() {
            return Err(InstanceError::NotFound(id.into()));
        }
        let lock = InstanceLock::acquire(&path.join(".operation.lock"), id)?;
        recover_instance(&path)?;
        let metadata_path = path.join("instance.json");
        if !metadata_path.is_file() {
            return Err(InstanceError::NotFound(id.into()));
        }
        let metadata = read_metadata(&metadata_path)?;
        Ok(InstanceLaunchGuard {
            metadata,
            _lock: lock,
        })
    }

    pub async fn install(
        &self,
        id: &str,
        request: &LoaderRequest,
        context: Option<TaskContext>,
    ) -> InstanceResult<InstanceMetadata> {
        validate_id(id)?;
        validate_segment(&request.minecraft)?;
        validate_segment(&request.version)?;
        check_cancelled(&context)?;
        let path = self.instance_path(id)?;
        fs::create_dir_all(&path).map_err(|e| io(&path, e))?;
        let _lock = InstanceLock::acquire(&path.join(".operation.lock"), id)?;
        recover_instance(&path)?;
        let game = path.join("game");
        let stage = path.join(".stage");
        let previous = path.join("previous");
        reject_symlink(&game)?;
        reject_symlink(&stage)?;
        reject_symlink(&previous)?;
        let request_marker = stage.join(".ccl-instance-request.json");
        let resume = stage_matches_request(&stage, request)?;
        if stage.exists() && !resume {
            fs::remove_dir_all(&stage).map_err(|e| io(&stage, e))?;
        }
        if !resume {
            fs::create_dir_all(&stage).map_err(|e| io(&stage, e))?;
            if game.exists() {
                clone_game_generation(&game, &stage)?;
            }
            atomic_write(
                &request_marker,
                &serde_json::to_vec(request).map_err(|source| InstanceError::Metadata {
                    path: request_marker.clone(),
                    source,
                })?,
            )?;
        }
        let operation = async {
            let loader = LoaderService::new(self.config.clone())?;
            let loader_version = loader
                .install(
                    request,
                    &InstallOptions::for_current_platform(&stage),
                    context.clone(),
                )
                .await?;
            validate_segment(&loader_version)?;
            check_cancelled(&context)?;
            Ok::<String, InstanceError>(loader_version)
        }
        .await;
        let loader_version = match operation {
            Ok(version) => version,
            Err(error) => return Err(error),
        };
        if game.exists() {
            replace_user_data(&game, &stage)?;
        }
        let metadata = InstanceMetadata {
            id: id.into(),
            minecraft: request.minecraft.clone(),
            loader: Some(request.kind),
            loader_version: Some(loader_version),
            game_directory: game.clone(),
        };
        clear_content_revision(&stage)?;
        commit_generation(&path, &stage, &game, &previous, &metadata)?;
        let _ = fs::remove_file(game.join(".ccl-instance-request.json"));
        Ok(metadata)
    }

    /// Restore the last committed generation, retaining the current generation for a later swap-back.
    pub async fn rollback(&self, id: &str) -> InstanceResult<InstanceMetadata> {
        let path = self.instance_path(id)?;
        if !path.is_dir() {
            return Err(InstanceError::NotFound(id.into()));
        }
        let _lock = InstanceLock::acquire(&path.join(".operation.lock"), id)?;
        recover_instance(&path)?;
        let previous = path.join("previous");
        let game = path.join("game");
        reject_symlink(&previous)?;
        reject_symlink(&game)?;
        if !previous.is_dir() {
            return Err(InstanceError::NotFound(format!(
                "{id} has no previous generation"
            )));
        }
        let prev_meta_path = path.join("previous.json");
        let meta_path = path.join("instance.json");
        let next_meta = read_metadata(&prev_meta_path)?;
        let current_swap = path.join(".rollback-current");
        let current_meta = read_metadata(&meta_path)?;
        let journal_path = path.join(".rollback.json");
        let mut journal = RollbackJournal {
            current: current_meta.clone(),
            next: next_meta.clone(),
            target_active: false,
        };
        atomic_write(
            &journal_path,
            &serde_json::to_vec(&journal).map_err(|source| InstanceError::Metadata {
                path: journal_path.clone(),
                source,
            })?,
        )?;
        if game.join(".ccl/content-revision").is_file() {
            // Content edits and pack updates roll back the complete user-data snapshot.
            // Mark the reverse edge too, so a second rollback can redo the change.
            mark_content_revision(&previous)?;
        } else {
            copy_latest_user_data(&game, &previous)?;
            clear_content_revision(&previous)?;
        }
        if let Err(error) = fs::rename(&game, &current_swap) {
            let _ = fs::remove_file(&journal_path);
            return Err(io(&game, error));
        }
        if let Err(e) = fs::rename(&previous, &game) {
            let _ = fs::rename(&current_swap, &game);
            let _ = fs::remove_file(&journal_path);
            return Err(io(&game, e));
        }
        journal.target_active = true;
        atomic_write(
            &journal_path,
            &serde_json::to_vec(&journal).map_err(|source| InstanceError::Metadata {
                path: journal_path.clone(),
                source,
            })?,
        )?;
        atomic_write(
            &prev_meta_path,
            &serde_json::to_vec(&current_meta).map_err(|source| InstanceError::Metadata {
                path: prev_meta_path.clone(),
                source,
            })?,
        )?;
        atomic_write(
            &meta_path,
            &serde_json::to_vec_pretty(&next_meta).map_err(|source| InstanceError::Metadata {
                path: meta_path.clone(),
                source,
            })?,
        )?;
        fs::rename(&current_swap, &previous).map_err(|e| io(&current_swap, e))?;
        fs::remove_file(&journal_path).map_err(|e| io(&journal_path, e))?;
        Ok(next_meta)
    }

    /// Return to vanilla in a new generation while preserving the loader generation for rollback.
    pub async fn uninstall_loader(
        &self,
        id: &str,
        context: Option<TaskContext>,
    ) -> InstanceResult<InstanceMetadata> {
        check_cancelled(&context)?;
        let path = self.instance_path(id)?;
        if !path.is_dir() {
            return Err(InstanceError::NotFound(id.into()));
        }
        let _lock = InstanceLock::acquire(&path.join(".operation.lock"), id)?;
        recover_instance(&path)?;
        let meta_path = path.join("instance.json");
        let current = read_metadata(&meta_path)?;
        let Some(loader_version) = current.loader_version.as_deref() else {
            // A failed install may have left a resumable stage; vanilla uninstall discards it.
            let stage = path.join(".stage");
            reject_symlink(&stage)?;
            if stage.exists() {
                fs::remove_dir_all(&stage).map_err(|e| io(&stage, e))?;
            }
            return Ok(current);
        };
        validate_segment(loader_version)?;
        let game = path.join("game");
        let stage = path.join(".stage");
        let previous = path.join("previous");
        reject_symlink(&game)?;
        reject_symlink(&stage)?;
        reject_symlink(&previous)?;
        if stage.exists() {
            fs::remove_dir_all(&stage).map_err(|e| io(&stage, e))?;
        }
        fs::create_dir_all(&stage).map_err(|e| io(&stage, e))?;
        clone_game_generation(&game, &stage)?;
        let loader_profile = stage.join("versions").join(loader_version);
        reject_symlink(&loader_profile)?;
        if loader_profile.exists() {
            fs::remove_dir_all(&loader_profile).map_err(|e| io(&loader_profile, e))?;
        }
        check_cancelled(&context)?;
        let meta = InstanceMetadata {
            id: current.id,
            minecraft: current.minecraft,
            loader: None,
            loader_version: None,
            game_directory: game.clone(),
        };
        clear_content_revision(&stage)?;
        commit_generation(&path, &stage, &game, &previous, &meta)?;
        Ok(meta)
    }

    fn instances_dir(&self) -> PathBuf {
        self.root.join(".ccl/instances")
    }
    fn instance_path(&self, id: &str) -> InstanceResult<PathBuf> {
        validate_id(id)?;
        let ccl = self.root.join(".ccl");
        let instances = ccl.join("instances");
        let instance = instances.join(id);
        reject_symlink(&ccl)?;
        reject_symlink(&instances)?;
        reject_symlink(&instance)?;
        Ok(instance)
    }
}

fn mark_content_revision(game: &Path) -> InstanceResult<()> {
    let directory = game.join(".ccl");
    reject_symlink(&directory)?;
    fs::create_dir_all(&directory).map_err(|e| io(&directory, e))?;
    atomic_write(&game.join(".ccl/content-revision"), b"1")
}

fn clear_content_revision(game: &Path) -> InstanceResult<()> {
    let marker = game.join(".ccl/content-revision");
    reject_symlink(&marker)?;
    match fs::remove_file(&marker) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(marker, e)),
    }
}

pub struct InstanceLaunchGuard {
    pub metadata: InstanceMetadata,
    _lock: InstanceLock,
}
impl InstanceLaunchGuard {
    pub fn game_directory(&self) -> &Path {
        &self.metadata.game_directory
    }
    pub fn version(&self) -> &str {
        self.metadata
            .loader_version
            .as_deref()
            .unwrap_or(&self.metadata.minecraft)
    }
}

fn validate_id(id: &str) -> InstanceResult<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || id == "."
        || id == ".."
    {
        return Err(InstanceError::InvalidId(id.into()));
    }
    Ok(())
}

fn absolute_path(path: PathBuf) -> InstanceResult<PathBuf> {
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().map_err(|e| io(".", e))?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::Normal(part) => normalized.push(part),
        }
    }
    let mut ancestor = normalized.clone();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            return Err(InstanceError::InvalidId(format!(
                "cannot resolve game root: {}",
                normalized.display()
            )));
        };
        suffix.push(name.to_os_string());
        if !ancestor.pop() {
            return Err(InstanceError::InvalidId(format!(
                "cannot resolve game root: {}",
                normalized.display()
            )));
        }
    }
    let mut result = fs::canonicalize(&ancestor).map_err(|e| io(&ancestor, e))?;
    for part in suffix.iter().rev() {
        result.push(part);
    }
    Ok(result)
}

fn validate_segment(value: &str) -> InstanceResult<String> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains(['/', '\\', ':'])
        || value.chars().any(char::is_control)
    {
        return Err(InstanceError::InvalidId(format!(
            "unsafe version path component: {value}"
        )));
    }
    Ok(value.to_owned())
}

fn check_cancelled(ctx: &Option<TaskContext>) -> InstanceResult<()> {
    if ctx.as_ref().is_some_and(|c| c.cancellation.is_cancelled()) {
        Err(InstanceError::Cancelled)
    } else {
        Ok(())
    }
}

fn read_metadata(path: &Path) -> InstanceResult<InstanceMetadata> {
    reject_symlink(path)?;
    let data = fs::read(path).map_err(|e| io(path, e))?;
    let mut meta: InstanceMetadata =
        serde_json::from_slice(&data).map_err(|source| InstanceError::Metadata {
            path: path.into(),
            source,
        })?;
    validate_id(&meta.id)?;
    if path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name != meta.id.as_str())
    {
        return Err(InstanceError::InvalidId(format!(
            "metadata ID does not match its instance directory: {}",
            meta.id
        )));
    }
    validate_segment(&meta.minecraft)?;
    if let Some(version) = &meta.loader_version {
        validate_segment(version)?;
    }
    if meta.loader.is_some() != meta.loader_version.is_some() {
        return Err(InstanceError::InvalidId(
            "loader kind and loader version must both be present or absent".into(),
        ));
    }
    if meta.loader.is_some() && meta.loader_version.as_deref() == Some(meta.minecraft.as_str()) {
        return Err(InstanceError::InvalidId(
            "loader profile cannot use the vanilla Minecraft version ID".into(),
        ));
    }
    let expected_game = path.parent().unwrap().join("game");
    if !meta.game_directory.as_os_str().is_empty() && meta.game_directory != expected_game {
        return Err(InstanceError::InvalidId(format!(
            "metadata game directory escapes its instance: {}",
            meta.game_directory.display()
        )));
    }
    reject_symlink(&expected_game)?;
    meta.game_directory = expected_game;
    Ok(meta)
}

fn commit_generation(
    instance: &Path,
    stage: &Path,
    game: &Path,
    previous: &Path,
    meta: &InstanceMetadata,
) -> InstanceResult<()> {
    commit_generation_using(instance, stage, game, previous, meta, atomic_write)
}

fn commit_generation_using(
    instance: &Path,
    stage: &Path,
    game: &Path,
    previous: &Path,
    meta: &InstanceMetadata,
    write_metadata: impl FnOnce(&Path, &[u8]) -> InstanceResult<()>,
) -> InstanceResult<()> {
    let meta_path = instance.join("instance.json");
    let previous_meta = instance.join("previous.json");
    let old = if meta_path.is_file() {
        Some(read_metadata(&meta_path)?)
    } else {
        None
    };
    let had_previous = previous.exists();
    let journal_path = instance.join(".commit.json");
    let journal = CommitJournal {
        next: meta.clone(),
        old: old.clone(),
        had_previous,
    };
    atomic_write(
        &journal_path,
        &serde_json::to_vec(&journal).map_err(|source| InstanceError::Metadata {
            path: journal_path.clone(),
            source,
        })?,
    )?;

    // Preserve the older rollback target until the new game directory and metadata commit.
    let saved_previous = instance.join(".saved-previous");
    let saved_previous_meta = instance.join(".saved-previous.json");
    if had_previous {
        reject_symlink(previous)?;
        fs::rename(previous, &saved_previous).map_err(|e| io(&saved_previous, e))?;
    }
    if previous_meta.exists() {
        reject_symlink(&previous_meta)?;
        fs::rename(&previous_meta, &saved_previous_meta)
            .map_err(|e| io(&saved_previous_meta, e))?;
    }
    if game.exists() {
        if let Err(error) = fs::rename(game, previous) {
            restore_saved_previous(instance, had_previous)?;
            let _ = fs::remove_file(&journal_path);
            return Err(io(game, error));
        }
        if let Some(old) = &old {
            let bytes = serde_json::to_vec(old).map_err(|source| InstanceError::Metadata {
                path: previous_meta.clone(),
                source,
            })?;
            if let Err(error) = atomic_write(&previous_meta, &bytes) {
                let _ = fs::rename(previous, game);
                restore_saved_previous(instance, had_previous)?;
                let _ = fs::remove_file(&journal_path);
                return Err(error);
            }
        }
    }
    if let Err(error) = fs::rename(stage, game) {
        if previous.exists() && !game.exists() {
            let _ = fs::rename(previous, game);
        }
        restore_saved_previous(instance, had_previous)?;
        let _ = fs::remove_file(&journal_path);
        return Err(io(game, error));
    }
    let data = serde_json::to_vec_pretty(meta).map_err(|source| InstanceError::Metadata {
        path: meta_path.clone(),
        source,
    })?;
    if let Err(error) = write_metadata(&meta_path, &data) {
        let failed = instance.join(".failed-generation");
        let _ = fs::rename(game, &failed);
        if previous.exists() {
            let _ = fs::rename(previous, game);
        }
        let _ = fs::remove_dir_all(failed);
        if let Some(old) = &old {
            let old_bytes = serde_json::to_vec(old).map_err(|source| InstanceError::Metadata {
                path: meta_path.clone(),
                source,
            })?;
            let _ = atomic_write(&meta_path, &old_bytes);
        }
        restore_saved_previous(instance, had_previous)?;
        let _ = fs::remove_file(&journal_path);
        return Err(error);
    }
    if saved_previous.exists() {
        fs::remove_dir_all(&saved_previous).map_err(|e| io(&saved_previous, e))?;
    }
    if saved_previous_meta.exists() {
        fs::remove_file(&saved_previous_meta).map_err(|e| io(&saved_previous_meta, e))?;
    }
    fs::remove_file(&journal_path).map_err(|e| io(&journal_path, e))?;
    Ok(())
}

fn restore_saved_previous(instance: &Path, had_previous: bool) -> InstanceResult<()> {
    let saved_previous = instance.join(".saved-previous");
    let saved_previous_meta = instance.join(".saved-previous.json");
    let previous = instance.join("previous");
    let previous_meta = instance.join("previous.json");
    if had_previous && saved_previous.exists() {
        if previous.exists() {
            fs::remove_dir_all(&previous).map_err(|e| io(&previous, e))?;
        }
        fs::rename(&saved_previous, &previous).map_err(|e| io(&previous, e))?;
    }
    if saved_previous_meta.exists() {
        if previous_meta.exists() {
            fs::remove_file(&previous_meta).map_err(|e| io(&previous_meta, e))?;
        }
        fs::rename(&saved_previous_meta, &previous_meta).map_err(|e| io(&previous_meta, e))?;
    }
    Ok(())
}

fn recover_instance(instance: &Path) -> InstanceResult<()> {
    recover_rollback(instance)?;
    let journal_path = instance.join(".commit.json");
    recover_atomic_file(&journal_path)?;
    recover_atomic_file(&instance.join("instance.json"))?;
    recover_atomic_file(&instance.join("previous.json"))?;
    if !journal_path.exists() {
        return Ok(());
    }
    reject_symlink(&journal_path)?;
    let bytes = fs::read(&journal_path).map_err(|e| io(&journal_path, e))?;
    let journal: CommitJournal =
        serde_json::from_slice(&bytes).map_err(|source| InstanceError::Metadata {
            path: journal_path.clone(),
            source,
        })?;
    let game = instance.join("game");
    let stage = instance.join(".stage");
    let previous = instance.join("previous");
    let previous_meta = instance.join("previous.json");
    let saved_previous = instance.join(".saved-previous");
    let saved_previous_meta = instance.join(".saved-previous.json");
    for path in [&game, &stage, &previous, &saved_previous] {
        reject_symlink(path)?;
    }
    recover_atomic_file(&instance.join("instance.json"))?;
    recover_atomic_file(&previous_meta)?;
    if game.exists() && !stage.exists() {
        // The staged directory was moved into place: finish the metadata commit.
        if let Some(old) = &journal.old {
            atomic_write(
                &previous_meta,
                &serde_json::to_vec(old).map_err(|source| InstanceError::Metadata {
                    path: previous_meta.clone(),
                    source,
                })?,
            )?;
        }
        atomic_write(
            &instance.join("instance.json"),
            &serde_json::to_vec_pretty(&journal.next).map_err(|source| {
                InstanceError::Metadata {
                    path: instance.join("instance.json"),
                    source,
                }
            })?,
        )?;
        let _ = fs::remove_file(game.join(".ccl-instance-request.json"));
        if saved_previous.exists() {
            fs::remove_dir_all(&saved_previous).map_err(|e| io(&saved_previous, e))?;
        }
        if saved_previous_meta.exists() {
            fs::remove_file(&saved_previous_meta).map_err(|e| io(&saved_previous_meta, e))?;
        }
    } else {
        // The new stage was not committed. Restore the active generation and its rollback target.
        if !game.exists() && previous.exists() {
            fs::rename(&previous, &game).map_err(|e| io(&game, e))?;
        }
        if stage.exists() {
            fs::remove_dir_all(&stage).map_err(|e| io(&stage, e))?;
        }
        restore_saved_previous(instance, journal.had_previous)?;
    }
    fs::remove_file(&journal_path).map_err(|e| io(&journal_path, e))?;
    Ok(())
}

fn recover_rollback(instance: &Path) -> InstanceResult<()> {
    let journal_path = instance.join(".rollback.json");
    recover_atomic_file(&journal_path)?;
    if !journal_path.exists() {
        return Ok(());
    }
    reject_symlink(&journal_path)?;
    let journal: RollbackJournal =
        serde_json::from_slice(&fs::read(&journal_path).map_err(|e| io(&journal_path, e))?)
            .map_err(|source| InstanceError::Metadata {
                path: journal_path.clone(),
                source,
            })?;
    let game = instance.join("game");
    let previous = instance.join("previous");
    let current_swap = instance.join(".rollback-current");
    for path in [&game, &previous, &current_swap] {
        reject_symlink(path)?;
    }
    recover_rollback_user_data(&previous)?;
    if current_swap.exists() && !game.exists() {
        // Only the current generation was moved. Abort the rollback and restore it.
        fs::rename(&current_swap, &game).map_err(|e| io(&game, e))?;
    } else if (current_swap.exists() && game.exists()) || journal.target_active {
        // The prior generation is active, or the current generation was already moved back.
        let previous_meta = instance.join("previous.json");
        let meta_path = instance.join("instance.json");
        atomic_write(
            &previous_meta,
            &serde_json::to_vec(&journal.current).map_err(|source| InstanceError::Metadata {
                path: previous_meta.clone(),
                source,
            })?,
        )?;
        atomic_write(
            &meta_path,
            &serde_json::to_vec_pretty(&journal.next).map_err(|source| {
                InstanceError::Metadata {
                    path: meta_path.clone(),
                    source,
                }
            })?,
        )?;
        if current_swap.exists() {
            if previous.exists() {
                fs::remove_dir_all(&previous).map_err(|e| io(&previous, e))?;
            }
            fs::rename(&current_swap, &previous).map_err(|e| io(&previous, e))?;
        }
    }
    fs::remove_file(&journal_path).map_err(|e| io(&journal_path, e))?;
    Ok(())
}

fn recover_rollback_user_data(target: &Path) -> InstanceResult<()> {
    for name in [
        "mods",
        "config",
        "saves",
        "resourcepacks",
        "shaderpacks",
        "screenshots",
    ] {
        let staged = target.join(format!(".rollback-data-{name}"));
        let backup = target.join(format!(".rollback-old-{name}"));
        let destination = target.join(name);
        reject_symlink(&staged)?;
        reject_symlink(&backup)?;
        reject_symlink(&destination)?;
        if destination.exists() {
            if backup.exists() {
                fs::remove_dir_all(&backup).map_err(|e| io(&backup, e))?;
            }
        } else if backup.exists() {
            fs::rename(&backup, &destination).map_err(|e| io(&destination, e))?;
        }
        if staged.exists() {
            fs::remove_dir_all(&staged).map_err(|e| io(&staged, e))?;
        }
    }
    Ok(())
}

fn clone_game_generation(from: &Path, to: &Path) -> InstanceResult<()> {
    reject_symlink(from)?;
    fs::create_dir_all(to).map_err(|e| io(to, e))?;
    for entry in fs::read_dir(from).map_err(|e| io(from, e))? {
        let entry = entry.map_err(|e| io(from, e))?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        if entry.file_name() == ".ccl-instance-request.json" {
            continue;
        }
        let kind = entry.file_type().map_err(|e| io(&source, e))?;
        if kind.is_symlink() {
            return Err(InstanceError::Conflict(format!(
                "symlink is not allowed in instance game data: {}",
                source.display()
            )));
        } else if kind.is_dir() && entry.file_name() == ".ccl" {
            clone_ccl_directory(from, to)?;
        } else if kind.is_dir() && entry.file_name() == "assets" {
            link_tree(&source, &destination)?;
        } else if kind.is_dir() {
            copy_tree(&source, &destination)?;
        } else if kind.is_file() {
            fs::copy(&source, &destination).map_err(|e| io(&destination, e))?;
        }
    }
    Ok(())
}

fn clone_ccl_directory(from_game: &Path, to_game: &Path) -> InstanceResult<()> {
    let source_root = from_game.join(".ccl");
    let destination_root = to_game.join(".ccl");
    reject_symlink(&source_root)?;
    fs::create_dir_all(&destination_root).map_err(|e| io(&destination_root, e))?;
    for entry in fs::read_dir(&source_root).map_err(|e| io(&source_root, e))? {
        let entry = entry.map_err(|e| io(&source_root, e))?;
        if entry.file_name() == "runtimes" {
            if !entry.file_type().map_err(|e| io(entry.path(), e))?.is_dir() {
                return Err(InstanceError::Conflict(format!(
                    "runtime storage is not a directory: {}",
                    entry.path().display()
                )));
            }
            continue;
        }
        let source = entry.path();
        let destination = destination_root.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| io(&source, e))?;
        if kind.is_symlink() {
            return Err(InstanceError::Conflict(format!(
                "symlink is not allowed in instance .ccl data: {}",
                source.display()
            )));
        } else if kind.is_dir() {
            copy_tree(&source, &destination)?;
        } else if kind.is_file() {
            fs::copy(&source, &destination).map_err(|e| io(&destination, e))?;
        }
    }
    reuse_runtimes(from_game, to_game)
}

fn replace_user_data(from: &Path, to: &Path) -> InstanceResult<()> {
    sync_latest_user_data(from, to)
}

fn stage_matches_request(stage: &Path, request: &LoaderRequest) -> InstanceResult<bool> {
    if !stage.is_dir() {
        return Ok(false);
    }
    let marker = stage.join(".ccl-instance-request.json");
    reject_symlink(&marker)?;
    let Ok(bytes) = fs::read(&marker) else {
        return Ok(false);
    };
    let Ok(saved) = serde_json::from_slice::<LoaderRequest>(&bytes) else {
        return Ok(false);
    };
    Ok(saved.kind == request.kind
        && saved.minecraft == request.minecraft
        && saved.version == request.version)
}

fn copy_latest_user_data(current: &Path, target: &Path) -> InstanceResult<()> {
    sync_latest_user_data(current, target)
}

fn sync_latest_user_data(source: &Path, destination: &Path) -> InstanceResult<()> {
    reject_symlink(source)?;
    reject_symlink(destination)?;
    fs::create_dir_all(destination).map_err(|e| io(destination, e))?;
    let mut retained = std::collections::HashSet::new();
    for entry in fs::read_dir(source).map_err(|e| io(source, e))? {
        let entry = entry.map_err(|e| io(source, e))?;
        let name = entry.file_name();
        let src = entry.path();
        let dst = destination.join(&name);
        reject_symlink(&src)?;
        reject_symlink(&dst)?;
        if is_managed_game_entry(&name) || name == ".ccl" || name == ".ccl-instance-request.json" {
            continue;
        }
        mirror_user_entry(&src, &dst)?;
        retained.insert(name);
    }
    for entry in fs::read_dir(destination).map_err(|e| io(destination, e))? {
        let entry = entry.map_err(|e| io(destination, e))?;
        let name = entry.file_name();
        reject_symlink(&entry.path())?;
        if is_managed_game_entry(&name) || name == ".ccl" || name == ".ccl-instance-request.json" {
            continue;
        }
        if !retained.contains(&name) {
            let path = entry.path();
            reject_symlink(&path)?;
            remove_path(&path)?;
        }
    }

    let source_ccl = source.join(".ccl");
    let destination_ccl = destination.join(".ccl");
    reject_symlink(&source_ccl)?;
    reject_symlink(&destination_ccl)?;
    let source_logs = source_ccl.join("logs");
    let destination_logs = destination_ccl.join("logs");
    reject_symlink(&source_logs)?;
    reject_symlink(&destination_logs)?;
    if source_logs.exists() {
        fs::create_dir_all(&destination_ccl).map_err(|e| io(&destination_ccl, e))?;
        mirror_user_entry(&source_logs, &destination_logs)?;
    } else if destination_logs.exists() {
        remove_path(&destination_logs)?;
    }
    Ok(())
}

fn is_managed_game_entry(name: &std::ffi::OsStr) -> bool {
    name == "assets" || name == "libraries" || name == "versions"
}

fn mirror_user_entry(source: &Path, destination: &Path) -> InstanceResult<()> {
    reject_symlink(source)?;
    reject_symlink(destination)?;
    if source.is_dir() {
        if destination.exists() && !destination.is_dir() {
            remove_path(destination)?;
        }
        fs::create_dir_all(destination).map_err(|e| io(destination, e))?;
        let mut retained = std::collections::HashSet::new();
        for entry in fs::read_dir(source).map_err(|e| io(source, e))? {
            let entry = entry.map_err(|e| io(source, e))?;
            let name = entry.file_name();
            let src = entry.path();
            let dst = destination.join(&name);
            mirror_user_entry(&src, &dst)?;
            retained.insert(name);
        }
        for entry in fs::read_dir(destination).map_err(|e| io(destination, e))? {
            let entry = entry.map_err(|e| io(destination, e))?;
            if !retained.contains(&entry.file_name()) {
                let path = entry.path();
                reject_symlink(&path)?;
                remove_path(&path)?;
            }
        }
    } else if source.is_file() {
        if destination.is_dir() {
            remove_path(destination)?;
        }
        fs::copy(source, destination).map_err(|e| io(destination, e))?;
    } else {
        return Err(InstanceError::Conflict(format!(
            "unsupported user data entry: {}",
            source.display()
        )));
    }
    Ok(())
}

fn remove_path(path: &Path) -> InstanceResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|e| io(path, e))?;
    if metadata.file_type().is_symlink() {
        return Err(InstanceError::Conflict(format!(
            "symlink is not allowed in instance user data: {}",
            path.display()
        )));
    }
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|e| io(path, e))
    } else {
        fs::remove_file(path).map_err(|e| io(path, e))
    }
}
fn copy_tree(from: &Path, to: &Path) -> InstanceResult<()> {
    fs::create_dir_all(to).map_err(|e| io(to, e))?;
    for entry in fs::read_dir(from).map_err(|e| io(from, e))? {
        let entry = entry.map_err(|e| io(from, e))?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| io(&src, e))?;
        if kind.is_dir() {
            copy_tree(&src, &dst)?;
        } else if kind.is_file() {
            fs::copy(&src, &dst).map_err(|e| io(&dst, e))?;
        } else if kind.is_symlink() {
            return Err(InstanceError::Conflict(format!(
                "symlink is not allowed in instance user data: {}",
                src.display()
            )));
        }
    }
    Ok(())
}

fn reuse_runtimes(from: &Path, to: &Path) -> InstanceResult<()> {
    let source_root = from.join(".ccl/runtimes");
    let destination_root = to.join(".ccl/runtimes");
    reject_symlink(&from.join(".ccl"))?;
    reject_symlink(&source_root)?;
    if source_root.is_dir() {
        link_runtime_tree(
            &source_root,
            &destination_root,
            &source_root
                .canonicalize()
                .map_err(|e| io(&source_root, e))?,
            &destination_root,
        )?;
    }
    Ok(())
}

fn link_runtime_tree(
    from: &Path,
    to: &Path,
    canonical_root: &Path,
    destination_root: &Path,
) -> InstanceResult<()> {
    fs::create_dir_all(to).map_err(|e| io(to, e))?;
    for entry in fs::read_dir(from).map_err(|e| io(from, e))? {
        let entry = entry.map_err(|e| io(from, e))?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| io(&source, e))?;
        if kind.is_dir() {
            link_runtime_tree(&source, &destination, canonical_root, destination_root)?;
        } else if kind.is_file() {
            fs::copy(&source, &destination).map_err(|e| io(&destination, e))?;
        } else if kind.is_symlink() {
            let target = fs::read_link(&source).map_err(|e| io(&source, e))?;
            let resolved = source.canonicalize().map_err(|e| io(&source, e))?;
            let relative = resolved.strip_prefix(canonical_root).map_err(|_| {
                InstanceError::Conflict(format!(
                    "runtime symlink escapes its instance: {}",
                    source.display()
                ))
            })?;
            let mapped_absolute = destination_root.join(relative);
            let mapped = if target.is_absolute() {
                relative_link_target(
                    destination.parent().ok_or_else(|| {
                        InstanceError::Conflict(format!(
                            "runtime symlink has no parent: {}",
                            destination.display()
                        ))
                    })?,
                    &mapped_absolute,
                )?
            } else {
                target
            };
            create_symlink(&mapped, &destination, resolved.is_dir())
                .map_err(|e| io(&destination, e))?;
        }
    }
    Ok(())
}

fn relative_link_target(from: &Path, target: &Path) -> InstanceResult<PathBuf> {
    let from_components: Vec<_> = from.components().collect();
    let target_components: Vec<_> = target.components().collect();
    let common = from_components
        .iter()
        .zip(&target_components)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return Err(InstanceError::Conflict(format!(
            "runtime symlink cannot be relocated from {} to {}",
            from.display(),
            target.display()
        )));
    }
    let mut relative = PathBuf::new();
    for _ in common..from_components.len() {
        relative.push("..");
    }
    for component in &target_components[common..] {
        relative.push(component.as_os_str());
    }
    Ok(relative)
}

#[cfg(unix)]
fn create_symlink(target: &Path, destination: &Path, _is_dir: bool) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, destination)
}

#[cfg(windows)]
fn create_symlink(target: &Path, destination: &Path, is_dir: bool) -> std::io::Result<()> {
    if is_dir {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

#[cfg(not(any(unix, windows)))]
fn create_symlink(_target: &Path, _destination: &Path, _is_dir: bool) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlinks are unsupported on this platform",
    ))
}

fn link_tree(from: &Path, to: &Path) -> InstanceResult<()> {
    fs::create_dir_all(to).map_err(|e| io(to, e))?;
    for entry in fs::read_dir(from).map_err(|e| io(from, e))? {
        let entry = entry.map_err(|e| io(from, e))?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        let file_type = entry.file_type().map_err(|e| io(&source, e))?;
        if file_type.is_symlink() {
            return Err(InstanceError::Conflict(format!(
                "symlink is not allowed in reusable game files: {}",
                source.display()
            )));
        } else if file_type.is_dir() {
            link_tree(&source, &destination)?;
        } else if file_type.is_file() && fs::hard_link(&source, &destination).is_err() {
            fs::copy(&source, &destination).map_err(|e| io(&destination, e))?;
        }
    }
    Ok(())
}
fn atomic_write(path: &Path, bytes: &[u8]) -> InstanceResult<()> {
    recover_atomic_file(path)?;
    reject_symlink(path)?;
    let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp-{}-{id}", std::process::id()));
    let backup = path.with_extension("replace-backup");
    reject_symlink(&backup)?;
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| io(&tmp, e))?;
    use std::io::Write;
    file.write_all(bytes).map_err(|e| io(&tmp, e))?;
    file.sync_all().map_err(|e| io(&tmp, e))?;
    if path.exists() {
        if backup.exists() {
            fs::remove_file(&backup).map_err(|e| io(&backup, e))?;
        }
        fs::rename(path, &backup).map_err(|e| io(path, e))?;
    }
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        if backup.exists() {
            let _ = fs::rename(&backup, path);
        }
        return Err(io(path, error));
    }
    if backup.exists() {
        fs::remove_file(&backup).map_err(|e| io(&backup, e))?;
    }
    Ok(())
}

fn recover_atomic_file(path: &Path) -> InstanceResult<()> {
    let backup = path.with_extension("replace-backup");
    reject_symlink(path)?;
    reject_symlink(&backup)?;
    match (path.exists(), backup.exists()) {
        (false, true) => fs::rename(&backup, path).map_err(|e| io(path, e)),
        (true, true) => fs::remove_file(&backup).map_err(|e| io(&backup, e)),
        _ => Ok(()),
    }
}
fn io(path: impl Into<PathBuf>, source: std::io::Error) -> InstanceError {
    InstanceError::Io {
        path: path.into(),
        source,
    }
}

fn reject_symlink(path: &Path) -> InstanceResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(InstanceError::Conflict(format!(
            "symlink is not allowed in instance storage: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io(path, error)),
    }
}

struct InstanceLock(File);
impl InstanceLock {
    fn acquire(path: &Path, id: &str) -> InstanceResult<Self> {
        let file = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| io(path, e))?;
        file.try_lock_exclusive()
            .map_err(|_| InstanceError::Conflict(id.into()))?;
        Ok(Self(file))
    }
}
impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ccl-instance-commit-test-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn metadata(version: &str, directory: &Path) -> InstanceMetadata {
        InstanceMetadata {
            id: "test".into(),
            minecraft: version.into(),
            loader: None,
            loader_version: None,
            game_directory: directory.into(),
        }
    }

    #[test]
    fn failed_commit_restores_active_and_previous_generation() {
        let temp = Temp::new();
        let instance = temp.0.join("test");
        fs::create_dir_all(&instance).unwrap();
        let instance = instance.as_path();
        let game = instance.join("game");
        let stage = instance.join(".stage");
        let previous = instance.join("previous");
        fs::create_dir_all(&game).unwrap();
        fs::create_dir_all(&stage).unwrap();
        fs::create_dir_all(&previous).unwrap();
        fs::write(game.join("marker"), "active").unwrap();
        fs::write(stage.join("marker"), "candidate").unwrap();
        fs::write(previous.join("marker"), "old rollback").unwrap();
        fs::write(
            instance.join("instance.json"),
            serde_json::to_vec(&metadata("1.21", &game)).unwrap(),
        )
        .unwrap();
        fs::write(
            instance.join("previous.json"),
            serde_json::to_vec(&metadata("1.20", &game)).unwrap(),
        )
        .unwrap();

        let error = commit_generation_using(
            instance,
            &stage,
            &game,
            &previous,
            &metadata("1.22", &game),
            |path, _| {
                Err(io(
                    path,
                    std::io::Error::new(std::io::ErrorKind::PermissionDenied, "injected failure"),
                ))
            },
        )
        .unwrap_err();
        assert!(matches!(error, InstanceError::Io { .. }));
        assert_eq!(fs::read_to_string(game.join("marker")).unwrap(), "active");
        assert_eq!(
            fs::read_to_string(previous.join("marker")).unwrap(),
            "old rollback"
        );
        assert_eq!(
            read_metadata(&instance.join("instance.json"))
                .unwrap()
                .minecraft,
            "1.21"
        );
        assert_eq!(
            read_metadata(&instance.join("previous.json"))
                .unwrap()
                .minecraft,
            "1.20"
        );
    }

    #[test]
    fn recovery_finishes_commit_if_stage_was_already_activated() {
        let temp = Temp::new();
        let instance = temp.0.join("test");
        fs::create_dir_all(&instance).unwrap();
        let instance = instance.as_path();
        let game = instance.join("game");
        let previous = instance.join("previous");
        fs::create_dir_all(&game).unwrap();
        fs::create_dir_all(&previous).unwrap();
        fs::write(game.join("marker"), "new").unwrap();
        fs::write(previous.join("marker"), "old").unwrap();
        let old = metadata("1.21", &game);
        let next = metadata("1.22", &game);
        fs::write(
            instance.join("instance.json"),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        fs::write(
            instance.join("previous.json"),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        fs::write(
            instance.join(".commit.json"),
            serde_json::to_vec(&CommitJournal {
                old: Some(old),
                next,
                had_previous: false,
            })
            .unwrap(),
        )
        .unwrap();
        recover_instance(instance).unwrap();
        assert_eq!(
            read_metadata(&instance.join("instance.json"))
                .unwrap()
                .minecraft,
            "1.22"
        );
        assert_eq!(
            read_metadata(&instance.join("previous.json"))
                .unwrap()
                .minecraft,
            "1.21"
        );
        assert!(!instance.join(".commit.json").exists());
    }

    #[test]
    fn interrupted_uninstall_commit_keeps_loader_generation_for_rollback() {
        let temp = Temp::new();
        let instance = temp.0.join("test");
        fs::create_dir_all(&instance).unwrap();
        let game = instance.join("game");
        let previous = instance.join("previous");
        let loader_id = "1.21.1-forge-52.1.0";
        fs::create_dir_all(game.join("versions/1.21.1")).unwrap();
        fs::create_dir_all(previous.join("versions").join(loader_id)).unwrap();
        fs::write(
            previous
                .join("versions")
                .join(loader_id)
                .join(format!("{loader_id}.json")),
            "profile",
        )
        .unwrap();
        let old = InstanceMetadata {
            id: "test".into(),
            minecraft: "1.21.1".into(),
            loader: Some(LoaderKind::Forge),
            loader_version: Some(loader_id.into()),
            game_directory: game.clone(),
        };
        let next = InstanceMetadata {
            loader: None,
            loader_version: None,
            ..old.clone()
        };
        fs::write(
            instance.join("instance.json"),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        fs::write(
            instance.join(".commit.json"),
            serde_json::to_vec(&CommitJournal {
                old: Some(old),
                next,
                had_previous: false,
            })
            .unwrap(),
        )
        .unwrap();

        recover_instance(&instance).unwrap();
        assert!(
            read_metadata(&instance.join("instance.json"))
                .unwrap()
                .loader
                .is_none()
        );
        let previous_meta = read_metadata(&instance.join("previous.json")).unwrap();
        assert_eq!(previous_meta.loader, Some(LoaderKind::Forge));
        assert!(
            previous
                .join("versions")
                .join(loader_id)
                .join(format!("{loader_id}.json"))
                .is_file()
        );
    }

    #[test]
    fn failed_stage_can_resume_only_for_the_same_request() {
        let temp = Temp::new();
        let stage = temp.0.join(".stage");
        fs::create_dir_all(&stage).unwrap();
        let request = LoaderRequest {
            kind: LoaderKind::Fabric,
            minecraft: "1.21".into(),
            version: "0.16.10".into(),
        };
        fs::write(
            stage.join(".ccl-instance-request.json"),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        fs::write(stage.join("downloaded-resource"), "ready").unwrap();
        assert!(stage_matches_request(&stage, &request).unwrap());
        let changed = LoaderRequest {
            version: "0.16.11".into(),
            ..request.clone()
        };
        assert!(!stage_matches_request(&stage, &changed).unwrap());
        assert_eq!(
            fs::read_to_string(stage.join("downloaded-resource")).unwrap(),
            "ready"
        );
    }

    #[test]
    fn user_data_sync_preserves_root_files_logs_and_deletions() {
        let temp = Temp::new();
        let current = temp.0.join("current");
        let stage = temp.0.join("stage");
        fs::create_dir_all(current.join(".ccl/logs")).unwrap();
        fs::create_dir_all(stage.join(".ccl/logs")).unwrap();
        fs::create_dir_all(current.join("assets")).unwrap();
        fs::create_dir_all(stage.join("assets")).unwrap();
        fs::create_dir_all(stage.join("versions")).unwrap();
        fs::write(current.join("options.txt"), "latest options").unwrap();
        fs::write(current.join("servers.dat"), "latest servers").unwrap();
        fs::write(current.join(".ccl/logs/run.log"), "latest log").unwrap();
        fs::write(stage.join("options.txt"), "stale options").unwrap();
        fs::write(stage.join("servers.dat_old"), "deleted by user").unwrap();
        fs::write(stage.join(".ccl/logs/run.log"), "stale log").unwrap();
        fs::write(current.join("assets/resource"), "current resource").unwrap();
        fs::write(stage.join("assets/resource"), "staged resource").unwrap();
        fs::write(stage.join("versions/profile"), "staged profile").unwrap();

        sync_latest_user_data(&current, &stage).unwrap();
        assert_eq!(
            fs::read_to_string(stage.join("options.txt")).unwrap(),
            "latest options"
        );
        assert_eq!(
            fs::read_to_string(stage.join("servers.dat")).unwrap(),
            "latest servers"
        );
        assert!(!stage.join("servers.dat_old").exists());
        assert_eq!(
            fs::read_to_string(stage.join(".ccl/logs/run.log")).unwrap(),
            "latest log"
        );
        assert_eq!(
            fs::read_to_string(stage.join("assets/resource")).unwrap(),
            "staged resource"
        );
        assert_eq!(
            fs::read_to_string(stage.join("versions/profile")).unwrap(),
            "staged profile"
        );
    }

    #[test]
    fn stage_clone_preserves_verified_installer_cache_files() {
        let temp = Temp::new();
        let current = temp.0.join("game");
        let stage = temp.0.join(".stage");
        let installers = current.join(".ccl/installers");
        fs::create_dir_all(&installers).unwrap();
        fs::write(
            installers.join("forge-1.21.1-52.1.0-installer.jar"),
            b"cached jar",
        )
        .unwrap();
        fs::write(
            installers.join("forge-1.21.1-52.1.0-installer.json"),
            br#"{"sha1":"expected","url":"https://example.invalid/installer.jar"}"#,
        )
        .unwrap();

        clone_game_generation(&current, &stage).unwrap();
        let staged_installers = stage.join(".ccl/installers");
        assert_eq!(
            fs::read(staged_installers.join("forge-1.21.1-52.1.0-installer.jar")).unwrap(),
            b"cached jar"
        );
        assert_eq!(
            fs::read_to_string(staged_installers.join("forge-1.21.1-52.1.0-installer.json"))
                .unwrap(),
            r#"{"sha1":"expected","url":"https://example.invalid/installer.jar"}"#
        );
    }

    #[cfg(unix)]
    #[test]
    fn stage_clone_rejects_symlinks_in_installer_cache() {
        use std::os::unix::fs::symlink;

        let temp = Temp::new();
        let current = temp.0.join("game");
        let stage = temp.0.join(".stage");
        let installers = current.join(".ccl/installers");
        fs::create_dir_all(&installers).unwrap();
        let external = temp.0.join("external-installer.jar");
        fs::write(&external, "outside").unwrap();
        symlink(&external, installers.join("forge-installer.jar")).unwrap();

        assert!(matches!(
            clone_game_generation(&current, &stage),
            Err(InstanceError::Conflict(_))
        ));
        assert!(!stage.join(".ccl/installers/forge-installer.jar").exists());
    }

    #[cfg(unix)]
    #[test]
    fn staged_runtime_symlinks_remain_inside_the_moved_generation() {
        use std::os::unix::fs::symlink;

        let temp = Temp::new();
        let current = temp.0.join("game");
        let stage = temp.0.join(".stage");
        let runtime = current.join(".ccl/runtimes/jre/bin");
        fs::create_dir_all(&runtime).unwrap();
        let java = runtime.join("java");
        fs::write(&java, "runtime").unwrap();
        let alias = runtime.join("current-java");
        symlink(&java, &alias).unwrap();

        clone_game_generation(&current, &stage).unwrap();
        let staged_alias = stage.join(".ccl/runtimes/jre/bin/current-java");
        assert!(!fs::read_link(&staged_alias).unwrap().is_absolute());
        fs::rename(&stage, current.with_file_name("active")).unwrap();
        let active = current.with_file_name("active");
        assert_eq!(
            fs::canonicalize(active.join(".ccl/runtimes/jre/bin/current-java")).unwrap(),
            fs::canonicalize(active.join(".ccl/runtimes/jre/bin/java")).unwrap()
        );
    }
}
