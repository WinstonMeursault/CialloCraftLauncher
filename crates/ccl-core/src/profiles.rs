//! Safe management of ordinary launcher profiles stored in `versions/<id>`.
//!
//! Profile bookkeeping lives under `.ccl/profiles`; opening a profile never moves or
//! converts its files. Resource files stay at the selected game root and the runtime
//! directory is resolved independently using the same HMCL-compatible rules as launch.

use std::{
    fs::{self, File},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    config::DownloadConfig,
    content::ContentService,
    instances::{self, CleanupPlan, DiskUsage, InstanceMetadata, InstanceService, Revision},
    launch,
    loaders::LoaderKind,
    metadata::VersionMetadata,
    modpacks::ModpackService,
};

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("invalid profile ID: {0}")]
    InvalidId(String),
    #[error("profile not found: {0}")]
    NotFound(String),
    #[error("profile is busy or being edited: {0}")]
    Conflict(String),
    #[error("profile depends on this profile: {0}")]
    Referenced(String),
    #[error("unsafe profile path: {0}")]
    UnsafePath(PathBuf),
    #[error("profile metadata is invalid at {path}: {source}")]
    Metadata {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("profile operation failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Instance(#[from] crate::instances::InstanceError),
}

pub type ProfileResult<T> = Result<T, ProfileError>;
static PROFILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A discovered vanilla or loader profile. `game_directory` is the actual runtime
/// directory; `resource_root` owns shared versions, libraries and assets.
#[derive(Debug, Clone)]
pub struct RootProfile {
    resource_root: PathBuf,
    profile_directory: PathBuf,
    metadata: InstanceMetadata,
}

impl RootProfile {
    pub fn open(root: impl AsRef<Path>, id: &str) -> ProfileResult<Self> {
        validate_id(id)?;
        let resource_root = absolute(root.as_ref())?;
        check_existing_chain(&resource_root)?;
        let profile_directory = resource_root.join("versions").join(id);
        check_existing_chain(&profile_directory)?;
        if !profile_directory.is_dir() {
            return Err(ProfileError::NotFound(id.into()));
        }
        let json_path = profile_directory.join(format!("{id}.json"));
        check_existing_chain(&json_path)?;
        let bytes = fs::read(&json_path).map_err(|e| io(&json_path, e))?;
        let parsed: VersionMetadata =
            serde_json::from_slice(&bytes).map_err(|source| ProfileError::Metadata {
                path: json_path,
                source,
            })?;
        let effective = launch::load_installed_metadata(&resource_root, id)
            .map_err(|e| ProfileError::Conflict(e.to_string()))?;
        let (loader, _detected_loader_version) = detect_loader(
            id,
            &effective,
            parsed.inherits_from.as_deref().unwrap_or(id),
        );
        let loader_version = loader.map(|_| id.to_owned());
        let raw: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        let minecraft = resolve_minecraft(&resource_root, id, &raw, &effective.libraries, loader)
            .unwrap_or_else(|| "Unknown".into());
        let game_directory = launch::installed_working_directory(&resource_root, id)
            .map_err(|e| ProfileError::Conflict(e.to_string()))?;
        let game_directory = absolute(&game_directory)?;
        if recover_if_pending(&resource_root, id, &game_directory)? {
            return Self::open(&resource_root, id);
        }
        Ok(Self {
            resource_root,
            profile_directory,
            metadata: InstanceMetadata {
                id: id.into(),
                minecraft,
                loader,
                loader_version,
                game_directory,
            },
        })
    }

    pub fn list(root: impl AsRef<Path>) -> ProfileResult<Vec<Self>> {
        let root = absolute(root.as_ref())?;
        check_existing_chain(&root)?;
        let versions = root.join("versions");
        check_existing_chain(&versions)?;
        let entries = match fs::read_dir(&versions) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io(&versions, e)),
        };
        let mut result = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| io(&versions, e))?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if !instances::is_valid_id(&id) {
                continue;
            }
            let path = entry.path();
            if !entry.file_type().map_err(|e| io(&path, e))?.is_dir() {
                continue;
            }
            if path.join(format!("{id}.json")).is_file() {
                result.push(Self::open(&root, &id)?);
            }
        }
        result.sort_by(|a, b| a.metadata.id.cmp(&b.metadata.id));
        Ok(result)
    }

    pub fn id(&self) -> &str {
        &self.metadata.id
    }
    pub fn resource_root(&self) -> &Path {
        &self.resource_root
    }
    pub fn profile_directory(&self) -> &Path {
        &self.profile_directory
    }
    pub fn game_directory(&self) -> &Path {
        &self.metadata.game_directory
    }
    pub fn metadata(&self) -> InstanceMetadata {
        self.metadata.clone()
    }

    /// Acquire this profile's stable cross-process lock. Keep the guard for the whole
    /// launch or mutation lifetime. Lock files are persistent and must not be removed.
    pub fn guard(&self) -> ProfileResult<ProfileGuard> {
        let profile = acquire_lock_file(&self.resource_root, &format!("profile-{}", self.id()))?;
        let resolved = launch::installed_working_directory(&self.resource_root, self.id())
            .map_err(|e| ProfileError::Conflict(e.to_string()))?;
        let resolved = absolute(&resolved)?;
        if resolved != self.metadata.game_directory {
            return Err(ProfileError::Conflict(format!(
                "profile runtime directory changed; reopen profile {}",
                self.id()
            )));
        }
        let runtime = acquire_lock_file(
            &self.resource_root,
            &format!("runtime-{}", short_hash(&resolved)),
        )?;
        let resolved_again = absolute(
            &launch::installed_working_directory(&self.resource_root, self.id())
                .map_err(|e| ProfileError::Conflict(e.to_string()))?,
        )?;
        if resolved_again != resolved {
            return Err(ProfileError::Conflict(format!(
                "profile runtime directory changed; reopen profile {}",
                self.id()
            )));
        }
        recover_if_pending(&self.resource_root, self.id(), self.game_directory())?;
        Ok(ProfileGuard {
            _files: vec![profile, runtime],
        })
    }

    /// Remove only `versions/<id>`. Shared resources and external/custom runtime
    /// directories remain intact. A runtime inside the profile directory is removed
    /// along with it. Profiles inheriting from this profile block deletion.
    pub fn delete(&self) -> ProfileResult<()> {
        let _guard = self.mutation_guard()?;
        cleanup_stale_workspaces(&self.resource_root, self.id())?;
        let dependents = self.dependents()?;
        if !dependents.is_empty() {
            return Err(ProfileError::Referenced(dependents.join(", ")));
        }
        check_existing_chain(&self.profile_directory)?;
        let trash = self.resource_root.join(".ccl/profiles/trash");
        ensure_directory(&trash)?;
        let retired = trash.join(format!("{}-{}", self.id(), unique()));
        fs::rename(&self.profile_directory, &retired)
            .map_err(|e| io(&self.profile_directory, e))?;
        if let Err(error) = fs::remove_dir_all(&retired) {
            let restore = fs::rename(&retired, &self.profile_directory);
            if restore.is_err() {
                return Err(io(&retired, error));
            }
            return Err(io(&self.profile_directory, error));
        }
        let history = self
            .resource_root
            .join(".ccl/profiles/history")
            .join(self.id());
        if history.exists() {
            check_existing_chain(&history)?;
            fs::remove_dir_all(&history).map_err(|e| io(&history, e))?;
        }
        Ok(())
    }

    /// Files this profile owns and may remove.
    pub fn delete_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![
            self.profile_directory.clone(),
            self.resource_root
                .join(".ccl/profiles/history")
                .join(self.id()),
        ];
        paths.extend(owned_workspace_dirs(&self.resource_root, self.id()).unwrap_or_default());
        paths
    }

    /// Shared or external paths preserved when the profile is deleted.
    pub fn retained_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![
            self.resource_root.join("libraries"),
            self.resource_root.join("assets"),
            self.resource_root.join("versions"),
        ];
        if !self
            .metadata
            .game_directory
            .starts_with(&self.profile_directory)
        {
            paths.push(self.metadata.game_directory.clone());
        }
        paths
    }

    /// Other profiles whose metadata inherits from this profile.
    pub fn dependents(&self) -> ProfileResult<Vec<String>> {
        let versions = self.resource_root.join("versions");
        let mut result = Vec::new();
        if !versions.is_dir() {
            return Ok(result);
        }
        for entry in fs::read_dir(&versions).map_err(|e| io(&versions, e))? {
            let entry = entry.map_err(|e| io(&versions, e))?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if id.eq_ignore_ascii_case(self.id()) || !instances::is_valid_id(&id) {
                continue;
            }
            let path = entry.path().join(format!("{id}.json"));
            if !path.is_file() {
                continue;
            }
            check_existing_chain(&path)?;
            let bytes = fs::read(&path).map_err(|e| io(&path, e))?;
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|source| ProfileError::Metadata {
                    path: path.clone(),
                    source,
                })?;
            if value
                .get("inheritsFrom")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|v| v.eq_ignore_ascii_case(self.id()))
                || value
                    .get("jar")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|v| v.eq_ignore_ascii_case(self.id()))
            {
                result.push(id);
            }
        }
        result.sort();
        Ok(result)
    }

    fn mutation_guard(&self) -> ProfileResult<ProfileGuard> {
        let mut guard = self.guard()?;
        let resource = acquire_lock_file(&self.resource_root, "resource-root")?;
        recover_profile_journal(&self.resource_root, self.id(), self.game_directory())?;
        guard._files.push(resource);
        Ok(guard)
    }

    /// Rename only the version profile. Referenced profiles prevent the rename so their
    /// inheritance remains valid. Shared resources and runtime data are left in place.
    pub fn rename(&self, destination_id: &str) -> ProfileResult<InstanceMetadata> {
        validate_id(destination_id)?;
        if destination_id.eq_ignore_ascii_case(self.id()) {
            return Err(ProfileError::Conflict(
                "source and destination IDs must differ".into(),
            ));
        }
        let versions = self.resource_root.join("versions");
        let target = versions.join(destination_id);
        let (first, second) =
            if self.id().to_ascii_lowercase() < destination_id.to_ascii_lowercase() {
                (
                    acquire_guard(&self.resource_root, self.id())?,
                    acquire_guard(&self.resource_root, destination_id)?,
                )
            } else {
                (
                    acquire_guard(&self.resource_root, destination_id)?,
                    acquire_guard(&self.resource_root, self.id())?,
                )
            };
        let _guards = (first, second);
        let _resource_guard = acquire_lock_file(&self.resource_root, "resource-root")?;
        recover_profile_journal(&self.resource_root, self.id(), self.game_directory())?;
        if target.exists() {
            return Err(ProfileError::Conflict(format!(
                "destination already exists: {destination_id}"
            )));
        }
        let dependents = self.dependents()?;
        if !dependents.is_empty() {
            return Err(ProfileError::Referenced(dependents.join(", ")));
        }
        let temporary = versions.join(format!(".ccl-rename-{}", unique()));
        let target_history = self
            .resource_root
            .join(".ccl/profiles/history")
            .join(destination_id);
        let source_history = self
            .resource_root
            .join(".ccl/profiles/history")
            .join(self.id());
        if target_history.exists() {
            return Err(ProfileError::Conflict(format!(
                "destination history exists: {destination_id}"
            )));
        }
        fs::rename(&self.profile_directory, &temporary)
            .map_err(|e| io(&self.profile_directory, e))?;
        let old_json = temporary.join(format!("{}.json", self.id()));
        let new_json = temporary.join(format!("{destination_id}.json"));
        let json_backup = temporary.join(".ccl-rename-metadata-backup");
        let old_jar = temporary.join(format!("{}.jar", self.id()));
        let new_jar = temporary.join(format!("{destination_id}.jar"));
        let result: ProfileResult<()> = (|| {
            if old_json.is_file() {
                fs::copy(&old_json, &json_backup).map_err(|e| io(&old_json, e))?;
                let mut value: serde_json::Value =
                    serde_json::from_slice(&fs::read(&old_json).map_err(|e| io(&old_json, e))?)
                        .map_err(|source| ProfileError::Metadata {
                            path: old_json.clone(),
                            source,
                        })?;
                if let Some(object) = value.as_object_mut() {
                    object.insert("id".into(), destination_id.into());
                }
                fs::write(&new_json, serde_json::to_vec_pretty(&value).unwrap())
                    .map_err(|e| io(&new_json, e))?;
                fs::remove_file(&old_json).map_err(|e| io(&old_json, e))?;
            }
            if old_jar.is_file() {
                fs::rename(&old_jar, &new_jar).map_err(|e| io(&old_jar, e))?;
            }
            if source_history.exists() {
                ensure_directory(target_history.parent().unwrap())?;
                fs::rename(&source_history, &target_history).map_err(|e| io(&source_history, e))?;
            }
            fs::rename(&temporary, &target).map_err(|e| io(&target, e))?;
            Ok(())
        })();
        if result.is_err() && temporary.exists() {
            if new_json.exists() {
                let _ = fs::remove_file(&new_json);
            }
            if json_backup.exists() {
                let _ = fs::rename(&json_backup, &old_json);
            }
            if new_jar.exists() {
                let _ = fs::rename(&new_jar, &old_jar);
            }
            if target_history.exists() && !source_history.exists() {
                let _ = fs::rename(&target_history, &source_history);
            }
            let _ = fs::rename(&temporary, &self.profile_directory);
        }
        result?;
        let _ = fs::remove_file(target.join(".ccl-rename-metadata-backup"));
        let renamed = Self::open(&self.resource_root, destination_id)?;
        Ok(renamed.metadata())
    }

    /// Copy a regular profile into the launcher's independent instance area. Shared
    /// resources remain in the selected root; userdata and version artifacts are copies.
    pub fn copy_to(
        &self,
        root: impl AsRef<Path>,
        destination_id: &str,
        _config: DownloadConfig,
    ) -> ProfileResult<InstanceMetadata> {
        validate_id(destination_id)?;
        let target_root = absolute(root.as_ref())?;
        check_existing_chain(&target_root)?;
        let _source = self.mutation_guard()?;
        let _target = acquire_guard(&target_root, destination_id)?;
        let target_instance = target_root.join(".ccl/instances").join(destination_id);
        if target_instance.exists() {
            return Err(ProfileError::Conflict(format!(
                "destination already exists: {destination_id}"
            )));
        }
        ensure_directory(target_instance.parent().unwrap())?;
        let temp = target_root.join(".ccl/profiles/copies").join(format!(
            "{}-{}",
            destination_id,
            unique()
        ));
        ensure_directory(temp.parent().unwrap())?;
        let effective = launch::load_installed_metadata(&self.resource_root, self.id())
            .map_err(|e| ProfileError::Conflict(e.to_string()))?;
        let result: ProfileResult<InstanceMetadata> = (|| {
            let game = temp.join("game");
            copy_tree(&self.resource_root.join("assets"), &game.join("assets"))?;
            copy_tree(
                &self.resource_root.join("libraries"),
                &game.join("libraries"),
            )?;
            let versions = game.join("versions");
            ensure_directory(&versions)?;
            copy_version_closure(
                &self.resource_root,
                &versions,
                self.id(),
                destination_id,
                &effective,
                self.metadata.loader.is_some(),
            )?;
            if self.metadata.loader.is_none() {
                write_flattened_metadata(
                    &versions,
                    &effective,
                    self.id(),
                    &self.metadata.minecraft,
                )?;
            }
            let game = temp.join("game");
            copy_runtime_data(&self.metadata.game_directory, &game, &self.resource_root)?;
            let game_directory = target_instance.join("game");
            let meta = InstanceMetadata {
                id: destination_id.into(),
                minecraft: self.metadata.minecraft.clone(),
                loader: self.metadata.loader,
                loader_version: self.metadata.loader.map(|_| destination_id.to_owned()),
                game_directory: game_directory.clone(),
            };
            fs::write(
                temp.join("instance.json"),
                serde_json::to_vec_pretty(&meta).unwrap(),
            )
            .map_err(|e| io(&temp, e))?;
            fs::rename(&temp, &target_instance).map_err(|e| io(&target_instance, e))?;
            Ok(meta)
        })();
        if temp.exists() {
            let _ = fs::remove_dir_all(&temp);
        }
        result
    }

    pub fn disk_usage(&self) -> ProfileResult<DiskUsage> {
        let _guard = self.guard()?;
        let active_bytes = profile_metadata_bytes(self)?
            + runtime_bytes(self.game_directory(), self.resource_root())?;
        let history_bytes = tree_bytes(
            &self
                .resource_root
                .join(".ccl/profiles/history")
                .join(self.id()),
        )?;
        let stage_bytes = owned_workspace_dirs(&self.resource_root, self.id())?
            .iter()
            .try_fold(0u64, |sum, path| {
                Ok::<u64, ProfileError>(sum + tree_bytes(path)?)
            })?;
        Ok(DiskUsage {
            id: self.id().into(),
            active_bytes,
            previous_bytes: 0,
            history_bytes,
            stage_bytes,
            logical_bytes: active_bytes + history_bytes + stage_bytes,
        })
    }

    pub fn history(&self) -> ProfileResult<Vec<Revision>> {
        let _guard = self.guard()?;
        let path = self
            .resource_root
            .join(".ccl/profiles/history")
            .join(self.id());
        if !path.is_dir() {
            return Ok(Vec::new());
        }
        check_existing_chain(&path)?;
        let mut result = Vec::new();
        for entry in fs::read_dir(&path).map_err(|e| io(&path, e))? {
            let entry = entry.map_err(|e| io(&path, e))?;
            let item = entry.path();
            if !entry.file_type().map_err(|e| io(&item, e))?.is_dir() {
                continue;
            }
            check_existing_chain(&item)?;
            let game = item.join("game");
            let mut bytes = tree_bytes(&game)?;
            bytes += tree_bytes(&item.join("version"))?;
            let revision = entry.file_name().to_string_lossy().into_owned();
            let metadata_path = item.join("metadata.json");
            let metadata = if metadata_path.is_file() {
                let mut m: InstanceMetadata = serde_json::from_slice(
                    &fs::read(&metadata_path).map_err(|e| io(&metadata_path, e))?,
                )
                .map_err(|source| ProfileError::Metadata {
                    path: metadata_path,
                    source,
                })?;
                m.game_directory = self.metadata.game_directory.clone();
                m
            } else {
                self.metadata()
            };
            result.push(Revision {
                revision,
                metadata,
                bytes,
            });
        }
        result.sort_by(|a, b| a.revision.cmp(&b.revision));
        Ok(result)
    }

    pub fn rollback(&self, revision: &str) -> ProfileResult<InstanceMetadata> {
        self.restore_revision(revision)
    }

    pub fn restore_revision(&self, revision: &str) -> ProfileResult<InstanceMetadata> {
        if revision.is_empty()
            || revision.contains('/')
            || revision.contains('\\')
            || revision == "."
            || revision == ".."
        {
            return Err(ProfileError::UnsafePath(PathBuf::from(revision)));
        }
        let _guard = self.mutation_guard()?;
        let history_base = self
            .resource_root
            .join(".ccl/profiles/history")
            .join(self.id());
        let revision = if revision == "previous" {
            let mut ids: Vec<_> = fs::read_dir(&history_base)
                .map_err(|e| io(&history_base, e))?
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_type().is_ok_and(|ty| ty.is_dir()))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            ids.sort();
            ids.pop()
                .ok_or_else(|| ProfileError::NotFound("previous".into()))?
        } else {
            revision.to_owned()
        };
        let history_dir = history_base.join(&revision);
        let archive = history_dir.join("game");
        check_existing_chain(&archive)?;
        if !archive.is_dir() {
            return Err(ProfileError::NotFound(revision));
        }
        let source = self.game_directory().to_path_buf();
        let backup = self
            .resource_root
            .join(".ccl/profiles/history")
            .join(self.id())
            .join(unique());
        ensure_directory(&backup)?;
        write_history_snapshot(self, &backup)?;
        let mut baseline = Vec::new();
        collect_resource_files(&self.resource_root, &self.resource_root, &mut baseline)?;
        let workspace = self
            .resource_root
            .join(".ccl/profile-workspaces")
            .join(format!("restore-{}", unique()));
        ensure_directory(&workspace)?;
        let result: ProfileResult<()> = (|| {
            copy_resource_root(&self.resource_root, &workspace)?;
            let version = history_dir.join("version");
            if version.is_dir() {
                let current = workspace.join("versions").join(self.id());
                if current.exists() {
                    fs::remove_dir_all(&current).map_err(|e| io(&current, e))?;
                }
                copy_tree(&version, &current)?;
            }
            let mut changes = Vec::new();
            if source.starts_with(&self.resource_root) {
                let relative = source.strip_prefix(&self.resource_root).unwrap();
                restore_runtime_data(&archive, &workspace.join(relative), &workspace)?;
            }
            changes.extend(plan_workspace_root(
                &workspace,
                &self.resource_root,
                &baseline,
                self.id(),
            )?);
            if !source.starts_with(&self.resource_root) {
                let mut external_baseline = Vec::new();
                collect_resource_files(&source, &source, &mut external_baseline)?;
                changes.extend(plan_external_runtime(
                    &archive,
                    &source,
                    &external_baseline,
                )?);
            }
            if !changes.is_empty() {
                apply_journaled_changes(
                    &self.resource_root,
                    self.id(),
                    &backup,
                    self.game_directory(),
                    &changes,
                )?;
            }
            Ok(())
        })();
        let _ = fs::remove_dir_all(&workspace);
        result?;
        Ok(Self::open(&self.resource_root, self.id())?.metadata())
    }

    /// Preview or apply retention cleanup for sidecar history only. The profile and
    /// runtime are never cleanup targets.
    pub fn cleanup(
        &self,
        keep: usize,
        include_previous: bool,
        apply: bool,
    ) -> ProfileResult<CleanupPlan> {
        let _guard = self.guard()?;
        let path = self
            .resource_root
            .join(".ccl/profiles/history")
            .join(self.id());
        let mut dirs = Vec::new();
        if path.is_dir() {
            check_existing_chain(&path)?;
            for item in fs::read_dir(&path).map_err(|e| io(&path, e))? {
                let item = item.map_err(|e| io(&path, e))?;
                if item.file_type().map_err(|e| io(item.path(), e))?.is_dir() {
                    dirs.push(item.path());
                }
            }
        }
        dirs.sort();
        dirs.reverse();
        let mut targets = Vec::new();
        let protected_previous = usize::from(!include_previous);
        for p in dirs
            .into_iter()
            .skip(keep.saturating_add(protected_previous))
        {
            targets.push(crate::instances::CleanupTarget {
                revision: p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                bytes: tree_bytes(&p)?,
            });
        }
        for workspace in owned_workspace_dirs(&self.resource_root, self.id())? {
            targets.push(crate::instances::CleanupTarget {
                revision: format!(
                    "workspace:{}",
                    workspace.file_name().unwrap_or_default().to_string_lossy()
                ),
                bytes: tree_bytes(&workspace)?,
            });
        }
        let logical_bytes = targets.iter().map(|t| t.bytes).sum();
        if apply {
            for target in &targets {
                if let Some(name) = target.revision.strip_prefix("workspace:") {
                    let workspace = self
                        .resource_root
                        .join(".ccl/profile-workspaces")
                        .join(name);
                    if owned_workspace_dirs(&self.resource_root, self.id())?.contains(&workspace) {
                        fs::remove_dir_all(&workspace).map_err(|e| io(&workspace, e))?;
                    }
                    continue;
                }
                let p = path.join(&target.revision);
                check_existing_chain(&p)?;
                fs::remove_dir_all(&p).map_err(|e| io(&p, e))?;
            }
        }
        Ok(CleanupPlan {
            id: self.id().into(),
            targets,
            logical_bytes,
            applied: apply,
        })
    }
}

pub struct ProfileGuard {
    _files: Vec<File>,
}

pub struct ProfileWorkspace {
    root: PathBuf,
    id: String,
    service: InstanceService,
    profile: RootProfile,
    _guard: ProfileGuard,
    runtime_subdirectory: PathBuf,
}

impl Drop for ProfileWorkspace {
    fn drop(&mut self) {
        if self
            .root
            .parent()
            .is_some_and(|p| p == self.profile.resource_root.join(".ccl/profile-workspaces"))
            && self
                .root
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(self.profile.id()))
            && self.root.exists()
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalEntry {
    target_root: PathBuf,
    workspace_root: PathBuf,
    relative: PathBuf,
    had_source: bool,
    desired: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalData {
    profile_id: String,
    resource_root: PathBuf,
    runtime_root: PathBuf,
    changes: Vec<JournalEntry>,
    committed: bool,
    history_path: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceManifest {
    resources: Vec<(PathBuf, String)>,
    external_runtime: Vec<(PathBuf, String)>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceOwner {
    resource_root: PathBuf,
    profile_id: String,
}
impl ProfileWorkspace {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn service(&self) -> &InstanceService {
        &self.service
    }
    pub fn runtime_directory(&self) -> PathBuf {
        self.service
            .runtime_path_for(&self.root.join(".ccl/instances").join(&self.id).join("game"))
    }
    pub fn runtime_subdirectory(&self) -> &Path {
        &self.runtime_subdirectory
    }
    pub fn content_service(&self, config: DownloadConfig) -> ProfileResult<ContentService> {
        if self.runtime_subdirectory.as_os_str().is_empty() {
            ContentService::new(self.root.clone(), config)
                .map_err(|e| ProfileError::Conflict(e.to_string()))
        } else {
            ContentService::new_with_runtime_subdirectory(
                self.root.clone(),
                config,
                self.runtime_subdirectory.clone(),
            )
            .map_err(|e| ProfileError::Conflict(e.to_string()))
        }
    }
    pub fn modpack_service(&self, config: DownloadConfig) -> ProfileResult<ModpackService> {
        ModpackService::new_with_runtime_subdirectory(
            self.root.clone(),
            config,
            self.runtime_subdirectory.clone(),
        )
        .map_err(|e| ProfileError::Conflict(e.to_string()))
    }
    pub fn abort(self) -> ProfileResult<()> {
        if self.root.exists() {
            fs::remove_dir_all(&self.root).map_err(|e| io(&self.root, e))?;
        }
        Ok(())
    }
    /// Publish only workspace runtime changes. Conflicting source changes are rejected.
    pub fn commit(self) -> ProfileResult<InstanceMetadata> {
        let workspace_root = self.root.clone();
        let result = self.commit_inner();
        if workspace_root.exists() {
            let _ = fs::remove_dir_all(&workspace_root);
        }
        result
    }

    fn commit_inner(self) -> ProfileResult<InstanceMetadata> {
        let source_runtime = self.profile.game_directory().to_path_buf();
        let workspace_runtime = self.root.join(".ccl/instances").join(&self.id).join("game");
        let old_manifest = self.root.join(".ccl/profile-workspace.json");
        let snapshots: WorkspaceManifest =
            serde_json::from_slice(&fs::read(&old_manifest).map_err(|e| io(&old_manifest, e))?)
                .map_err(|source| ProfileError::Metadata {
                    path: old_manifest.clone(),
                    source,
                })?;
        let service_metadata = self.service.metadata(&self.id)?;
        let dependents = self.profile.dependents()?;
        if (service_metadata.loader != self.profile.metadata.loader
            || service_metadata.loader_version != self.profile.metadata.loader_version
            || service_metadata.minecraft != self.profile.metadata.minecraft)
            && !dependents.is_empty()
        {
            return Err(ProfileError::Referenced(dependents.join(", ")));
        }
        let _resource_lock = acquire_lock_file(&self.profile.resource_root, "resource-root")?;
        recover_profile_journal(
            &self.profile.resource_root,
            &self.id,
            &self.profile.metadata.game_directory,
        )?;
        let history = self
            .profile
            .resource_root
            .join(".ccl/profiles/history")
            .join(&self.id)
            .join(unique());
        ensure_directory(&history)?;
        if let Err(error) = write_history_snapshot(&self.profile, &history) {
            let _ = fs::remove_dir_all(&history);
            return Err(error);
        }
        let publish_result: ProfileResult<()> = (|| {
            prepare_profile_metadata(&workspace_runtime, &self.profile, &service_metadata)?;
            let external_workspace = workspace_runtime.join(".ccl-profile-runtime");
            let mut changes = Vec::new();
            if !source_runtime.starts_with(&self.profile.resource_root) {
                changes.extend(plan_external_runtime(
                    &external_workspace,
                    &source_runtime,
                    &snapshots.external_runtime,
                )?);
            }
            changes.extend(plan_workspace_root(
                &workspace_runtime,
                &self.profile.resource_root,
                &snapshots.resources,
                self.profile.id(),
            )?);
            if !changes.is_empty() {
                apply_journaled_changes(
                    &self.profile.resource_root,
                    &self.id,
                    &history,
                    &self.profile.metadata.game_directory,
                    &changes,
                )?;
            }
            Ok(())
        })();
        if let Err(error) = publish_result {
            let _ = fs::remove_dir_all(&history);
            return Err(error);
        }
        Ok(InstanceMetadata {
            id: self.profile.id().into(),
            game_directory: self.profile.game_directory().into(),
            minecraft: service_metadata.minecraft,
            loader: service_metadata.loader,
            loader_version: service_metadata
                .loader
                .map(|_| self.profile.id().to_owned()),
        })
    }
}

impl RootProfile {
    /// Build an isolated private service root. No data is copied by discovery/open;
    /// workspace creation is explicit and the source runtime is snapshotted for conflict detection.
    pub fn begin_workspace(&self, config: DownloadConfig) -> ProfileResult<ProfileWorkspace> {
        let guard = self.guard()?;
        let resource_lock = acquire_lock_file(&self.resource_root, "resource-root")?;
        let base = self.resource_root.join(".ccl/profile-workspaces");
        ensure_directory(&base)?;
        cleanup_stale_workspaces(&self.resource_root, self.id())?;
        let root = base.join(format!("{}-{}", self.id(), unique()));
        ensure_directory(&root)?;
        let owner_path = root.join(".ccl/profile-workspace-owner.json");
        ensure_directory(owner_path.parent().unwrap())?;
        let owner = WorkspaceOwner {
            resource_root: self.resource_root.clone(),
            profile_id: self.id().into(),
        };
        fs::write(&owner_path, serde_json::to_vec(&owner).unwrap())
            .map_err(|e| io(&owner_path, e))?;
        let result = (|| {
            recover_profile_journal(&self.resource_root, self.id(), self.game_directory())?;
            let instance_dir = root.join(".ccl/instances").join(self.id());
            ensure_directory(&instance_dir)?;
            let game = instance_dir.join("game");
            let mut resource_snapshot = Vec::new();
            collect_resource_files(
                &self.resource_root,
                &self.resource_root,
                &mut resource_snapshot,
            )?;
            let mut external_runtime = Vec::new();
            if !self
                .metadata
                .game_directory
                .starts_with(&self.resource_root)
            {
                collect_resource_files(
                    &self.metadata.game_directory,
                    &self.metadata.game_directory,
                    &mut external_runtime,
                )?;
            }
            copy_resource_root(&self.resource_root, &game)?;
            let mut after_copy = Vec::new();
            collect_resource_files(&self.resource_root, &self.resource_root, &mut after_copy)?;
            if resource_snapshot != after_copy {
                return Err(ProfileError::Conflict(
                    "game root changed while preparing the private workspace".into(),
                ));
            }
            let runtime_subdirectory = if !self
                .metadata
                .game_directory
                .starts_with(&self.resource_root)
            {
                let relative = PathBuf::from(".ccl-profile-runtime");
                let runtime = game.join(&relative);
                copy_runtime_data(&self.metadata.game_directory, &runtime, &self.resource_root)?;
                let mut after_runtime = Vec::new();
                collect_resource_files(
                    &self.metadata.game_directory,
                    &self.metadata.game_directory,
                    &mut after_runtime,
                )?;
                if external_runtime != after_runtime {
                    return Err(ProfileError::Conflict(
                        "custom runtime changed while preparing the private workspace".into(),
                    ));
                }
                relative
            } else {
                self.metadata
                    .game_directory
                    .strip_prefix(&self.resource_root)
                    .unwrap_or(Path::new(""))
                    .to_path_buf()
            };
            let meta = InstanceMetadata {
                game_directory: game.clone(),
                ..self.metadata.clone()
            };
            fs::write(
                instance_dir.join("instance.json"),
                serde_json::to_vec_pretty(&meta).unwrap(),
            )
            .map_err(|e| io(&instance_dir, e))?;
            let manifest = WorkspaceManifest {
                resources: resource_snapshot,
                external_runtime,
            };
            let marker = root.join(".ccl/profile-workspace.json");
            fs::write(&marker, serde_json::to_vec(&manifest).unwrap())
                .map_err(|e| io(&marker, e))?;
            let service = if runtime_subdirectory.as_os_str().is_empty() {
                InstanceService::new(root.clone(), config)?
            } else {
                InstanceService::new(root.clone(), config)?
                    .with_runtime_subdirectory(runtime_subdirectory.clone())?
            };
            Ok(ProfileWorkspace {
                root: root.clone(),
                id: self.id().into(),
                service,
                profile: self.clone(),
                _guard: guard,
                runtime_subdirectory,
            })
        })();
        drop(resource_lock);
        if result.is_err() {
            let _ = fs::remove_dir_all(&root);
        }
        result
    }
}

fn prepare_profile_metadata(
    game_root: &Path,
    profile: &RootProfile,
    metadata: &InstanceMetadata,
) -> ProfileResult<()> {
    let installing = metadata.loader.is_some()
        && (metadata.loader != profile.metadata.loader
            || metadata.loader_version != profile.metadata.loader_version);
    let uninstalling = metadata.loader.is_none() && profile.metadata.loader.is_some();
    if !installing && !uninstalling {
        return Ok(());
    }
    let target_dir = game_root.join("versions").join(profile.id());
    ensure_directory(&target_dir)?;
    let target_json = target_dir.join(format!("{}.json", profile.id()));
    let base_dir = game_root.join("versions").join(&metadata.minecraft);
    let base_json = base_dir.join(format!("{}.json", metadata.minecraft));
    let mut base: serde_json::Value = serde_json::from_slice(
        &fs::read(&base_json).map_err(|e| io(&base_json, e))?,
    )
    .map_err(|source| ProfileError::Metadata {
        path: base_json.clone(),
        source,
    })?;
    if installing {
        let loader_id = metadata.loader_version.as_deref().unwrap_or(profile.id());
        let loader_json = game_root
            .join("versions")
            .join(loader_id)
            .join(format!("{loader_id}.json"));
        let loader: serde_json::Value =
            serde_json::from_slice(&fs::read(&loader_json).map_err(|e| io(&loader_json, e))?)
                .map_err(|source| ProfileError::Metadata {
                    path: loader_json.clone(),
                    source,
                })?;
        let base_libs = base
            .get("libraries")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let loader_libs = loader
            .get("libraries")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut libraries = base_libs;
        for item in loader_libs {
            let name = item.get("name");
            if !libraries.iter().any(|old| old.get("name") == name) {
                libraries.push(item);
            }
        }
        let object = base.as_object_mut().ok_or_else(|| {
            ProfileError::Conflict("base version metadata is not an object".into())
        })?;
        if let Some(loader_object) = loader.as_object() {
            for (key, value) in loader_object {
                if !matches!(key.as_str(), "id" | "inheritsFrom" | "libraries") {
                    object.insert(key.clone(), value.clone());
                }
            }
        }
        object.insert("libraries".into(), serde_json::Value::Array(libraries));
    }
    if let Some(object) = base.as_object_mut() {
        object.insert("id".into(), profile.id().into());
        object.remove("inheritsFrom");
        object.remove("jar");
    }
    fs::write(&target_json, serde_json::to_vec_pretty(&base).unwrap())
        .map_err(|e| io(&target_json, e))?;
    let target_jar = target_dir.join(format!("{}.jar", profile.id()));
    if !target_jar.exists() {
        let base_jar = base_dir.join(format!("{}.jar", metadata.minecraft));
        if base_jar.is_file() {
            fs::copy(&base_jar, &target_jar).map_err(|e| io(&base_jar, e))?;
        }
    }
    Ok(())
}

fn write_history_snapshot(profile: &RootProfile, destination: &Path) -> ProfileResult<()> {
    ensure_directory(destination)?;
    copy_runtime_data(
        profile.game_directory(),
        &destination.join("game"),
        profile.resource_root(),
    )?;
    copy_tree(profile.profile_directory(), &destination.join("version"))?;
    let metadata = serde_json::to_vec_pretty(&profile.metadata()).unwrap();
    fs::write(destination.join("metadata.json"), metadata).map_err(|e| io(destination, e))?;
    Ok(())
}

fn validate_id(id: &str) -> ProfileResult<()> {
    if instances::is_valid_id(id) {
        Ok(())
    } else {
        Err(ProfileError::InvalidId(id.into()))
    }
}
fn absolute(path: &Path) -> ProfileResult<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(|e| io(".", e))?.join(path)
    };
    let mut suffix = Vec::new();
    let mut ancestor = path.as_path();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            return Err(ProfileError::UnsafePath(path));
        };
        suffix.push(name.to_os_string());
        ancestor = ancestor
            .parent()
            .ok_or_else(|| ProfileError::UnsafePath(path.clone()))?;
    }
    let mut result = fs::canonicalize(ancestor).map_err(|e| io(ancestor, e))?;
    for name in suffix.into_iter().rev() {
        result.push(name);
    }
    Ok(result)
}
fn io(path: impl AsRef<Path>, source: std::io::Error) -> ProfileError {
    ProfileError::Io {
        path: path.as_ref().to_path_buf(),
        source,
    }
}
fn unique() -> String {
    format!(
        "{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        PROFILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}
fn detect_loader(
    id: &str,
    metadata: &VersionMetadata,
    minecraft: &str,
) -> (Option<LoaderKind>, Option<String>) {
    for library in &metadata.libraries {
        let name = library.name.to_ascii_lowercase();
        let candidate = if name.starts_with("net.fabricmc:fabric-loader:") {
            Some((LoaderKind::Fabric, library.name.rsplit(':').next()))
        } else if name.starts_with("org.quiltmc:quilt-loader:") {
            Some((LoaderKind::Quilt, library.name.rsplit(':').next()))
        } else if name.starts_with("net.legacyfabric:legacy-fabric-loader:") {
            Some((LoaderKind::LegacyFabric, library.name.rsplit(':').next()))
        } else if name.starts_with("net.neoforged:neoforge:") {
            Some((LoaderKind::NeoForge, library.name.rsplit(':').next()))
        } else if name.starts_with("net.minecraftforge:forge:")
            || name.starts_with("net.minecraftforge:fmlloader:")
        {
            Some((LoaderKind::Forge, library.name.rsplit(':').next()))
        } else if name.contains("optifine") {
            Some((LoaderKind::OptiFine, Some(id)))
        } else {
            None
        };
        if let Some((kind, version)) = candidate {
            let version = version
                .unwrap_or(id)
                .strip_prefix(&format!("{minecraft}-"))
                .unwrap_or(version.unwrap_or(id))
                .to_owned();
            return (Some(kind), Some(version));
        }
    }
    (None, None)
}
fn version_like(value: &str) -> Option<String> {
    let candidate = value.strip_prefix("minecraft:").unwrap_or(value);
    let bytes = candidate.as_bytes();
    if bytes.len() == 6
        && bytes[2] == b'w'
        && bytes[5].is_ascii_lowercase()
        && [0, 1, 3, 4].iter().all(|i| bytes[*i].is_ascii_digit())
    {
        return Some(candidate.to_owned());
    }
    let numeric = if let Some((base, suffix)) = candidate.split_once('-') {
        let n = suffix
            .strip_prefix("pre")
            .or_else(|| suffix.strip_prefix("rc"))?;
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        base
    } else {
        candidate
    };
    let mut parts = numeric.split('.');
    if !parts.next()?.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let rest: Vec<_> = parts.collect();
    if rest.is_empty()
        || rest
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some(candidate.to_owned())
}
fn loader_minecraft_version(
    kind: LoaderKind,
    libraries: &[crate::metadata::Library],
) -> Option<String> {
    let prefixes: &[&str] = match kind {
        LoaderKind::Forge => &["net.minecraftforge:fmlloader:", "net.minecraftforge:forge:"],
        LoaderKind::NeoForge => &["net.neoforged:neoforge:"],
        _ => return None,
    };
    libraries.iter().find_map(|library| {
        prefixes.iter().find_map(|prefix| {
            library
                .name
                .strip_prefix(prefix)
                .and_then(|v| version_like(v.split_once('-')?.0))
        })
    })
}
fn resolve_minecraft(
    root: &Path,
    id: &str,
    original: &serde_json::Value,
    libraries: &[crate::metadata::Library],
    loader: Option<LoaderKind>,
) -> Option<String> {
    if let Some(value) = original
        .get("inheritsFrom")
        .or_else(|| original.get("jar"))
        .and_then(serde_json::Value::as_str)
        .and_then(version_like)
    {
        return Some(value);
    }
    if let Some(value) = original
        .get("minecraftVersion")
        .or_else(|| original.get("clientVersion"))
        .and_then(serde_json::Value::as_str)
        .and_then(version_like)
    {
        return Some(value);
    }
    if let Some(version) = loader.and_then(|kind| loader_minecraft_version(kind, libraries)) {
        return Some(version);
    }
    let mut current = original.clone();
    let mut visited = std::collections::HashSet::new();
    for _ in 0..64 {
        let Some(parent) = current
            .get("inheritsFrom")
            .or_else(|| current.get("jar"))
            .and_then(serde_json::Value::as_str)
        else {
            return version_like(id);
        };
        if let Some(version) = version_like(parent) {
            return Some(version);
        }
        if !visited.insert(parent.to_ascii_lowercase()) {
            return None;
        }
        let path = root
            .join("versions")
            .join(parent)
            .join(format!("{parent}.json"));
        if check_existing_chain(&path).is_err() {
            return None;
        }
        current = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        if let Some(value) = current
            .get("minecraftVersion")
            .or_else(|| current.get("clientVersion"))
            .and_then(serde_json::Value::as_str)
            .and_then(version_like)
        {
            return Some(value);
        }
    }
    version_like(id)
}
fn check_existing_chain(path: &Path) -> ProfileResult<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Prefix(_) => current.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => return Err(ProfileError::UnsafePath(path.into())),
            Component::Normal(v) => {
                current.push(v);
                if fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink()) {
                    return Err(ProfileError::UnsafePath(current));
                }
            }
        }
    }
    Ok(())
}
fn ensure_directory(path: &Path) -> ProfileResult<()> {
    check_existing_chain(path)?;
    fs::create_dir_all(path).map_err(|e| io(path, e))
}
fn acquire_guard(root: &Path, id: &str) -> ProfileResult<ProfileGuard> {
    validate_id(id)?;
    let file = acquire_lock_file(root, &format!("profile-{id}"))?;
    Ok(ProfileGuard { _files: vec![file] })
}
fn acquire_lock_file(root: &Path, key: &str) -> ProfileResult<File> {
    let locks = root.join(".ccl/profile-locks");
    ensure_directory(&locks)?;
    let path = locks.join(format!("{}.lock", key.to_ascii_lowercase()));
    check_existing_chain(&path)?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| io(&path, e))?;
    file.try_lock_exclusive().map_err(|e| {
        if e.kind() == std::io::ErrorKind::WouldBlock {
            ProfileError::Conflict(key.into())
        } else {
            io(&path, e)
        }
    })?;
    Ok(file)
}
fn short_hash(path: &Path) -> String {
    format!("{:x}", Sha256::digest(path.to_string_lossy().as_bytes()))
}
fn copy_tree(source: &Path, destination: &Path) -> ProfileResult<()> {
    check_existing_chain(source)?;
    if !source.exists() {
        return Ok(());
    }
    ensure_directory(destination)?;
    for entry in fs::read_dir(source).map_err(|e| io(source, e))? {
        let entry = entry.map_err(|e| io(source, e))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| io(&from, e))?;
        if kind.is_symlink() {
            return Err(ProfileError::UnsafePath(from));
        }
        if kind.is_dir() {
            copy_tree(&from, &to)?;
        } else if kind.is_file() {
            fs::copy(&from, &to).map_err(|e| io(&from, e))?;
        }
    }
    Ok(())
}
fn copy_version_closure(
    source_root: &Path,
    destination_versions: &Path,
    selected_id: &str,
    destination_id: &str,
    effective: &VersionMetadata,
    loader: bool,
) -> ProfileResult<()> {
    let mut ids = Vec::new();
    let mut visited = std::collections::HashSet::new();
    collect_version_closure(source_root, selected_id, &mut visited, &mut ids, 0)?;
    if let Some(owner) = effective
        .extra
        .get("jar")
        .and_then(serde_json::Value::as_str)
    {
        collect_version_closure(source_root, owner, &mut visited, &mut ids, 0)?;
    }
    if loader
        && ids
            .iter()
            .any(|id| id != selected_id && id == destination_id)
    {
        return Err(ProfileError::Conflict(format!(
            "destination ID collides with inherited version {destination_id}"
        )));
    }
    let mut target_ids = std::collections::HashSet::new();
    for source_id in ids {
        let target_id = if loader && source_id == selected_id {
            destination_id
        } else {
            &source_id
        };
        if !target_ids.insert(target_id.to_owned()) {
            continue;
        }
        copy_version_artifacts(source_root, &source_id, destination_versions, target_id)?;
    }
    Ok(())
}

fn collect_version_closure(
    root: &Path,
    id: &str,
    visited: &mut std::collections::HashSet<String>,
    ordered: &mut Vec<String>,
    depth: usize,
) -> ProfileResult<()> {
    if depth > 32 {
        return Err(ProfileError::Conflict(
            "version inheritance exceeds 32 profiles".into(),
        ));
    }
    validate_id(id)?;
    if !visited.insert(id.to_owned()) {
        return Ok(());
    }
    let path = root.join("versions").join(id).join(format!("{id}.json"));
    check_existing_chain(&path)?;
    let bytes = fs::read(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ProfileError::NotFound(id.into())
        } else {
            io(&path, error)
        }
    })?;
    let metadata: VersionMetadata =
        serde_json::from_slice(&bytes).map_err(|source| ProfileError::Metadata {
            path: path.clone(),
            source,
        })?;
    if metadata.id.as_deref() != Some(id) {
        return Err(ProfileError::Conflict(format!(
            "version metadata ID does not match {id}"
        )));
    }
    if let Some(parent) = metadata.inherits_from.as_deref() {
        collect_version_closure(root, parent, visited, ordered, depth + 1)?;
    }
    if let Some(owner) = metadata
        .extra
        .get("jar")
        .and_then(serde_json::Value::as_str)
    {
        collect_version_closure(root, owner, visited, ordered, depth + 1)?;
    }
    ordered.push(id.into());
    Ok(())
}

fn copy_version_artifacts(
    source_root: &Path,
    source_id: &str,
    destination_versions: &Path,
    target_id: &str,
) -> ProfileResult<()> {
    let source_dir = source_root.join("versions").join(source_id);
    let source_json = source_dir.join(format!("{source_id}.json"));
    check_existing_chain(&source_json)?;
    let target_dir = destination_versions.join(target_id);
    ensure_directory(&target_dir)?;
    let target_json = target_dir.join(format!("{target_id}.json"));
    let mut value: serde_json::Value = serde_json::from_slice(
        &fs::read(&source_json).map_err(|e| io(&source_json, e))?,
    )
    .map_err(|source| ProfileError::Metadata {
        path: source_json.clone(),
        source,
    })?;
    if let Some(object) = value.as_object_mut() {
        object.insert("id".into(), target_id.into());
    }
    fs::write(&target_json, serde_json::to_vec_pretty(&value).unwrap())
        .map_err(|e| io(&target_json, e))?;
    let source_jar = source_dir.join(format!("{source_id}.jar"));
    check_existing_chain(&source_jar)?;
    if source_jar.is_file() {
        fs::copy(&source_jar, target_dir.join(format!("{target_id}.jar")))
            .map_err(|e| io(&source_jar, e))?;
    }
    Ok(())
}

fn write_flattened_metadata(
    versions: &Path,
    effective: &VersionMetadata,
    selected_id: &str,
    minecraft: &str,
) -> ProfileResult<()> {
    let mut flattened = effective.clone();
    flattened.id = Some(minecraft.into());
    flattened.inherits_from = None;
    if let Some(owner) = effective
        .extra
        .get("jar")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| effective.client().is_some().then(|| selected_id.to_owned()))
    {
        if owner == minecraft {
            flattened.extra.remove("jar");
        } else {
            flattened.extra.insert("jar".into(), owner.into());
        }
    }
    let dir = versions.join(minecraft);
    ensure_directory(&dir)?;
    let path = dir.join(format!("{minecraft}.json"));
    fs::write(&path, serde_json::to_vec_pretty(&flattened).unwrap()).map_err(|e| io(&path, e))?;
    if flattened.client().is_some() {
        let owner = flattened
            .extra
            .get("jar")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(minecraft);
        let jar = versions.join(owner).join(format!("{owner}.jar"));
        if !jar.is_file() {
            return Err(ProfileError::NotFound(format!("client jar owner {owner}")));
        }
    }
    Ok(())
}
fn tree_bytes(path: &Path) -> ProfileResult<u64> {
    check_existing_chain(path)?;
    if !path.exists() {
        return Ok(0);
    }
    if path.is_file() {
        return Ok(fs::metadata(path).map_err(|e| io(path, e))?.len());
    }
    let mut total = 0;
    for entry in fs::read_dir(path).map_err(|e| io(path, e))? {
        let entry = entry.map_err(|e| io(path, e))?;
        let p = entry.path();
        let ty = entry.file_type().map_err(|e| io(&p, e))?;
        if ty.is_symlink() {
            total += fs::symlink_metadata(&p).map_err(|e| io(&p, e))?.len();
        } else if ty.is_dir() {
            total += tree_bytes(&p)?;
        } else if ty.is_file() {
            total += entry.metadata().map_err(|e| io(&p, e))?.len();
        }
    }
    Ok(total)
}

fn runtime_bytes(path: &Path, resource_root: &Path) -> ProfileResult<u64> {
    let mut total = 0;
    for name in runtime_paths(path, resource_root)? {
        total += tree_bytes(&path.join(name))?;
    }
    Ok(total)
}

fn profile_metadata_bytes(profile: &RootProfile) -> ProfileResult<u64> {
    let mut total = 0;
    for name in [
        format!("{}.json", profile.id()),
        format!("{}.jar", profile.id()),
        "hmclversion.cfg".into(),
    ] {
        total += tree_bytes(&profile.profile_directory.join(name))?;
    }
    Ok(total)
}

fn private_data_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "launcher_profiles.json"
            | "launcher_profiles_microsoft_store.json"
            | "launcher_accounts.json"
            | "accounts.json"
            | "hmcl_accounts.json"
    )
}
fn safe_ccl_relative(path: &Path) -> bool {
    let parts: Vec<_> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    if parts.iter().any(|part| private_data_name(part)) {
        return false;
    }
    if parts.first() != Some(&".ccl") {
        return false;
    }
    match parts.as_slice() {
        [
            ".ccl",
            "modpack.json" | "online-content.json" | "content-revision",
        ] => true,
        [".ccl", "cache", "version_manifest.json"] => true,
        [".ccl", "installers", name] => {
            name.ends_with("-installer.jar") || name.ends_with("-installer.jar.sha1.json")
        }
        [".ccl", "runtimes", component, ..] => !component.is_empty() && parts.len() >= 4,
        [".ccl", "forge-data", version, ..] => !version.is_empty() && parts.len() >= 4,
        _ => false,
    }
}
fn safe_ccl_prefix(path: &Path) -> bool {
    path == Path::new(".ccl")
        || ["cache", "installers", "runtimes", "forge-data"]
            .iter()
            .any(|name| {
                path == Path::new(".ccl").join(name)
                    || path.starts_with(Path::new(".ccl").join(name))
            })
}

fn copy_resource_root(source: &Path, destination: &Path) -> ProfileResult<()> {
    check_existing_chain(source)?;
    ensure_directory(destination)?;
    for entry in fs::read_dir(source).map_err(|e| io(source, e))? {
        let entry = entry.map_err(|e| io(source, e))?;
        let from = entry.path();
        let name = entry.file_name();
        let to = destination.join(&name);
        if name == ".ccl" {
            copy_safe_game_sidecars(&from, &to)?;
            continue;
        }
        if name == ".ccl-profile-runtime" || private_data_name(&name.to_string_lossy()) {
            continue;
        }
        let kind = entry.file_type().map_err(|e| io(&from, e))?;
        if kind.is_symlink() {
            return Err(ProfileError::UnsafePath(from));
        }
        if kind.is_dir() {
            copy_resource_root(&from, &to)?;
        } else if kind.is_file() {
            fs::copy(&from, &to).map_err(|e| io(&from, e))?;
        }
    }
    Ok(())
}

fn copy_safe_game_sidecars(source: &Path, destination: &Path) -> ProfileResult<()> {
    if fs::symlink_metadata(source).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(ProfileError::UnsafePath(source.into()));
    }
    if source.is_dir() {
        copy_safe_ccl_directory(source, destination, Path::new(".ccl"), source)?;
    }
    Ok(())
}

fn copy_safe_ccl_directory(
    source: &Path,
    destination: &Path,
    relative: &Path,
    ccl_root: &Path,
) -> ProfileResult<()> {
    for entry in fs::read_dir(source).map_err(|e| io(source, e))? {
        let entry = entry.map_err(|e| io(source, e))?;
        let from = entry.path();
        let name = entry.file_name();
        let rel = relative.join(&name);
        let kind = entry.file_type().map_err(|e| io(&from, e))?;
        if kind.is_symlink() {
            if !rel.starts_with(Path::new(".ccl/runtimes")) {
                return Err(ProfileError::UnsafePath(from));
            }
            copy_runtime_symlink(&from, &destination.join(&name), &ccl_root.join("runtimes"))?;
        } else if kind.is_dir() && safe_ccl_prefix(&rel) {
            copy_safe_ccl_directory(&from, &destination.join(&name), &rel, ccl_root)?;
        } else if kind.is_file() && safe_ccl_relative(&rel) {
            ensure_directory(destination)?;
            let to = destination.join(&name);
            fs::copy(&from, &to).map_err(|e| io(&from, e))?;
        }
    }
    Ok(())
}

fn copy_runtime_symlink(
    source: &Path,
    destination: &Path,
    runtime_root: &Path,
) -> ProfileResult<()> {
    let link = fs::read_link(source).map_err(|e| io(source, e))?;
    if link.is_absolute() || link.to_string_lossy().contains('\\') {
        return Err(ProfileError::UnsafePath(source.into()));
    }
    let root = fs::canonicalize(runtime_root).map_err(|e| io(runtime_root, e))?;
    let resolved = fs::canonicalize(source).map_err(|e| io(source, e))?;
    if !resolved.starts_with(&root) {
        return Err(ProfileError::UnsafePath(source.into()));
    }
    ensure_directory(destination.parent().unwrap())?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&link, destination).map_err(|e| io(destination, e))?;
    }
    #[cfg(windows)]
    {
        fs::hard_link(&resolved, destination).map_err(|e| io(destination, e))?;
    }
    Ok(())
}

fn collect_resource_files(
    base: &Path,
    path: &Path,
    output: &mut Vec<(PathBuf, String)>,
) -> ProfileResult<()> {
    if !path.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(|e| io(path, e))? {
        let entry = entry.map_err(|e| io(path, e))?;
        let p = entry.path();
        let name = entry.file_name();
        let kind = entry.file_type().map_err(|e| io(&p, e))?;
        if kind.is_symlink() {
            return Err(ProfileError::UnsafePath(p));
        }
        if name == ".ccl" {
            if p == base.join(".ccl") && p.is_dir() {
                collect_safe_ccl_files(base, &p, Path::new(".ccl"), output)?;
            }
            continue;
        }
        if name == ".ccl-profile-runtime" || private_data_name(&name.to_string_lossy()) {
            continue;
        }
        if kind.is_dir() {
            collect_resource_files(base, &p, output)?;
        } else if kind.is_file() {
            output.push((
                p.strip_prefix(base).unwrap().to_path_buf(),
                file_digest(&p)?,
            ));
        }
    }
    Ok(())
}

fn collect_safe_ccl_files(
    base: &Path,
    path: &Path,
    relative: &Path,
    output: &mut Vec<(PathBuf, String)>,
) -> ProfileResult<()> {
    if relative.starts_with(Path::new(".ccl/runtimes")) {
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(|e| io(path, e))? {
        let entry = entry.map_err(|e| io(path, e))?;
        let p = entry.path();
        let rel = relative.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| io(&p, e))?;
        if kind.is_symlink() {
            return Err(ProfileError::UnsafePath(p));
        }
        if kind.is_dir() && safe_ccl_prefix(&rel) {
            collect_safe_ccl_files(base, &p, &rel, output)?;
        } else if kind.is_file() && safe_ccl_relative(&rel) {
            output.push((
                p.strip_prefix(base).unwrap().to_path_buf(),
                file_digest(&p)?,
            ));
        }
    }
    Ok(())
}

fn plan_workspace_root(
    workspace: &Path,
    source: &Path,
    baseline: &[(PathBuf, String)],
    profile_id: &str,
) -> ProfileResult<Vec<JournalEntry>> {
    let mut desired = Vec::new();
    collect_resource_files(workspace, workspace, &mut desired)?;
    let mut current = Vec::new();
    collect_resource_files(source, source, &mut current)?;
    let baseline: std::collections::HashMap<_, _> = baseline.iter().cloned().collect();
    let desired: std::collections::HashMap<_, _> = desired.into_iter().collect();
    let current: std::collections::HashMap<_, _> = current.into_iter().collect();
    let mut paths = std::collections::BTreeSet::new();
    paths.extend(baseline.keys().cloned());
    paths.extend(desired.keys().cloned());
    let mut changes = Vec::new();
    for relative in paths {
        let before = baseline.get(&relative);
        let want = desired.get(&relative);
        let now = current.get(&relative);
        if want == now {
            continue;
        }
        if now != before && !(before.is_none() && now.is_none()) {
            return Err(ProfileError::Conflict(format!(
                "source changed during profile edit: {}",
                relative.display()
            )));
        }
        if immutable_resource(&relative) && before.is_some() {
            return Err(ProfileError::Conflict(format!(
                "shared resource is immutable: {}",
                relative.display()
            )));
        }
        let is_current_profile = relative.starts_with(Path::new("versions").join(profile_id));
        if relative.starts_with("versions") && before.is_some() && !is_current_profile {
            return Err(ProfileError::Conflict(format!(
                "cannot modify another version profile file: {}",
                relative.display()
            )));
        }
        if want.is_none() && relative.starts_with("versions") && !is_current_profile {
            return Err(ProfileError::Conflict(format!(
                "cannot remove another version profile file: {}",
                relative.display()
            )));
        }
        changes.push(JournalEntry {
            target_root: source.to_path_buf(),
            workspace_root: workspace.to_path_buf(),
            relative,
            had_source: now.is_some(),
            desired: want.is_some(),
        });
    }
    Ok(changes)
}

fn plan_external_runtime(
    workspace: &Path,
    source: &Path,
    baseline: &[(PathBuf, String)],
) -> ProfileResult<Vec<JournalEntry>> {
    let mut desired = Vec::new();
    collect_resource_files(workspace, workspace, &mut desired)?;
    let mut current = Vec::new();
    collect_resource_files(source, source, &mut current)?;
    let baseline: std::collections::HashMap<_, _> = baseline.iter().cloned().collect();
    let desired: std::collections::HashMap<_, _> = desired.into_iter().collect();
    let current: std::collections::HashMap<_, _> = current.into_iter().collect();
    let mut paths = std::collections::BTreeSet::new();
    paths.extend(baseline.keys().cloned());
    paths.extend(desired.keys().cloned());
    let mut changes = Vec::new();
    for relative in paths {
        let before = baseline.get(&relative);
        let want = desired.get(&relative);
        let now = current.get(&relative);
        if want == now {
            continue;
        }
        if now != before && !(before.is_none() && now.is_none()) {
            return Err(ProfileError::Conflict(format!(
                "external runtime changed during edit: {}",
                relative.display()
            )));
        }
        changes.push(JournalEntry {
            target_root: source.to_path_buf(),
            workspace_root: workspace.to_path_buf(),
            relative,
            had_source: now.is_some(),
            desired: want.is_some(),
        });
    }
    Ok(changes)
}

fn immutable_resource(path: &Path) -> bool {
    matches!(
        path.components()
            .next()
            .and_then(|c| c.as_os_str().to_str()),
        Some("assets" | "libraries")
    )
}

fn apply_journaled_changes(
    root: &Path,
    id: &str,
    history: &Path,
    runtime_root: &Path,
    changes: &[JournalEntry],
) -> ProfileResult<()> {
    recover_profile_journal(root, id, runtime_root)?;
    let journal = root.join(".ccl/profiles/journals").join(id);
    ensure_directory(journal.parent().unwrap())?;
    ensure_directory(&journal)?;
    let result = (|| {
        for (i, change) in changes.iter().enumerate() {
            validate_relative(&change.relative)?;
            check_existing_chain(&change.target_root)?;
            let source = change.target_root.join(&change.relative);
            let backup = journal.join("backup").join(i.to_string());
            let stage = journal.join("stage").join(i.to_string());
            if change.had_source {
                ensure_directory(backup.parent().unwrap())?;
                fs::copy(&source, &backup).map_err(|e| io(&source, e))?;
            }
            if change.desired {
                let from = change.workspace_root.join(&change.relative);
                ensure_directory(stage.parent().unwrap())?;
                fs::copy(&from, &stage).map_err(|e| io(&from, e))?;
            }
        }
        let marker = journal.join("journal.json");
        let mut data = JournalData {
            profile_id: id.to_owned(),
            resource_root: root.to_path_buf(),
            runtime_root: runtime_root.to_path_buf(),
            changes: changes.to_vec(),
            committed: false,
            history_path: history.to_path_buf(),
        };
        write_journal(&marker, &data)?;
        for (i, change) in changes.iter().enumerate() {
            let target = change.target_root.join(&change.relative);
            check_existing_chain(&target)?;
            check_existing_chain(target.parent().unwrap())?;
            ensure_directory(target.parent().unwrap())?;
            if change.desired {
                rename_replace(&journal.join("stage").join(i.to_string()), &target)?;
            } else if target.exists() {
                fs::remove_file(&target).map_err(|e| io(&target, e))?;
            }
        }
        data.committed = true;
        write_journal(&marker, &data)?;
        fs::remove_dir_all(&journal).map_err(|e| io(&journal, e))?;
        Ok(())
    })();
    if let Err(error) = result {
        recover_profile_journal(root, id, runtime_root)?;
        return Err(error);
    }
    Ok(())
}

fn recover_profile_journal(root: &Path, id: &str, runtime_root: &Path) -> ProfileResult<()> {
    let journal = root.join(".ccl/profiles/journals").join(id);
    let marker = journal.join("journal.json");
    if !marker.is_file() {
        if journal.exists() {
            fs::remove_dir_all(&journal).map_err(|e| io(&journal, e))?;
        }
        return Ok(());
    }
    check_existing_chain(&marker)?;
    let data: JournalData = serde_json::from_slice(&fs::read(&marker).map_err(|e| io(&marker, e))?)
        .map_err(|source| ProfileError::Metadata {
            path: marker.clone(),
            source,
        })?;
    if data.profile_id != id || data.resource_root != root || data.runtime_root != runtime_root {
        return Err(ProfileError::UnsafePath(marker));
    }
    let expected_history_parent = root.join(".ccl/profiles/history").join(id);
    if data.history_path.parent() != Some(expected_history_parent.as_path()) {
        return Err(ProfileError::UnsafePath(data.history_path));
    }
    check_existing_chain(&data.history_path)?;
    validate_transaction_changes(root, id, runtime_root, &data.changes)?;
    if !data.committed {
        for (i, change) in data.changes.iter().enumerate().rev() {
            validate_relative(&change.relative)?;
            check_existing_chain(&change.target_root)?;
            let target = change.target_root.join(&change.relative);
            check_existing_chain(&target)?;
            let backup = journal.join("backup").join(i.to_string());
            if change.had_source {
                if backup.is_file() {
                    check_existing_chain(&backup)?;
                    ensure_directory(target.parent().unwrap())?;
                    let restore = target.with_extension(format!("ccl-restore-{}", unique()));
                    fs::copy(&backup, &restore).map_err(|e| io(&backup, e))?;
                    rename_replace(&restore, &target)?;
                }
            } else if target.exists() {
                fs::remove_file(&target).map_err(|e| io(&target, e))?;
            }
        }
        if data.history_path.exists() {
            let _ = fs::remove_dir_all(&data.history_path);
        }
    }
    fs::remove_dir_all(&journal).map_err(|e| io(&journal, e))?;
    Ok(())
}

fn validate_transaction_changes(
    root: &Path,
    id: &str,
    runtime_root: &Path,
    changes: &[JournalEntry],
) -> ProfileResult<()> {
    for change in changes {
        validate_relative(&change.relative)?;
        if change.target_root != root && change.target_root != runtime_root {
            return Err(ProfileError::UnsafePath(change.target_root.clone()));
        }
        if change.target_root == root {
            let mut parts = change.relative.components();
            if parts.next().is_some_and(|c| c.as_os_str() == "versions") {
                let version = parts.next().and_then(|c| c.as_os_str().to_str());
                if version != Some(id) && change.had_source {
                    return Err(ProfileError::UnsafePath(change.relative.clone()));
                }
            }
            if immutable_resource(&change.relative) && change.had_source {
                return Err(ProfileError::UnsafePath(change.relative.clone()));
            }
        }
    }
    Ok(())
}

fn recover_if_pending(root: &Path, id: &str, runtime_root: &Path) -> ProfileResult<bool> {
    let journal = root.join(".ccl/profiles/journals").join(id);
    if journal.exists() {
        let _lock = acquire_lock_file(root, "resource-root")?;
        recover_profile_journal(root, id, runtime_root)?;
        return Ok(true);
    }
    Ok(false)
}

fn owned_workspace_dirs(root: &Path, id: &str) -> ProfileResult<Vec<PathBuf>> {
    let base = root.join(".ccl/profile-workspaces");
    if !base.is_dir() {
        return Ok(Vec::new());
    }
    check_existing_chain(&base)?;
    let mut result = Vec::new();
    for entry in fs::read_dir(&base).map_err(|e| io(&base, e))? {
        let entry = entry.map_err(|e| io(&base, e))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| io(&path, e))?;
        if kind.is_symlink() {
            return Err(ProfileError::UnsafePath(path));
        }
        if !kind.is_dir() {
            continue;
        }
        let marker = path.join(".ccl/profile-workspace-owner.json");
        if !marker.is_file() {
            continue;
        }
        check_existing_chain(&marker)?;
        let owner: WorkspaceOwner = serde_json::from_slice(
            &fs::read(&marker).map_err(|e| io(&marker, e))?,
        )
        .map_err(|source| ProfileError::Metadata {
            path: marker,
            source,
        })?;
        if owner.profile_id == id && owner.resource_root == root {
            result.push(path);
        }
    }
    Ok(result)
}

fn cleanup_stale_workspaces(root: &Path, id: &str) -> ProfileResult<()> {
    for path in owned_workspace_dirs(root, id)? {
        check_existing_chain(&path)?;
        fs::remove_dir_all(&path).map_err(|e| io(&path, e))?;
    }
    Ok(())
}

fn validate_relative(path: &Path) -> ProfileResult<()> {
    let sidecar_allowed = safe_ccl_relative(path);
    if path.as_os_str().is_empty()
        || path.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(private_data_name)
        })
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || path
            .components()
            .next()
            .is_some_and(|c| c.as_os_str() == ".ccl")
            && !sidecar_allowed
    {
        return Err(ProfileError::UnsafePath(path.into()));
    }
    Ok(())
}

fn write_journal(path: &Path, data: &JournalData) -> ProfileResult<()> {
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(data).unwrap();
    let mut file = File::options()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|e| io(&temporary, e))?;
    file.write_all(&bytes).map_err(|e| io(&temporary, e))?;
    file.sync_all().map_err(|e| io(&temporary, e))?;
    rename_replace(&temporary, path)
}

fn rename_replace(source: &Path, destination: &Path) -> ProfileResult<()> {
    #[cfg(unix)]
    {
        fs::rename(source, destination).map_err(|e| io(destination, e))?;
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let from: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                    | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            return Err(io(destination, std::io::Error::last_os_error()));
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        fs::rename(source, destination).map_err(|e| io(destination, e))?;
        Ok(())
    }
}
fn runtime_paths(source: &Path, resource_root: &Path) -> ProfileResult<Vec<PathBuf>> {
    let mut result = Vec::new();
    if !source.exists() {
        return Ok(result);
    }
    check_existing_chain(source)?;
    for entry in fs::read_dir(source).map_err(|e| io(source, e))? {
        let entry = entry.map_err(|e| io(source, e))?;
        let name = entry.file_name();
        let text = name.to_string_lossy();
        let path = entry.path();
        if entry.file_type().map_err(|e| io(&path, e))?.is_symlink() {
            return Err(ProfileError::UnsafePath(path));
        }
        if text == ".ccl"
            || text.starts_with(".ccl-profile-")
            || matches!(
                text.to_ascii_lowercase().as_str(),
                "launcher_profiles.json"
                    | "launcher_profiles_microsoft_store.json"
                    | "launcher_accounts.json"
                    | "accounts.json"
                    | "hmcl_accounts.json"
            )
            || text == "versions" && source == resource_root
            || text == "libraries" && source == resource_root
            || text == "assets" && source == resource_root
        {
            continue;
        }
        if source.starts_with(resource_root)
            && source != resource_root
            && (text.ends_with(".json") || text.ends_with(".jar"))
        {
            continue;
        }
        result.push(PathBuf::from(name));
    }
    Ok(result)
}

fn copy_runtime_data(source: &Path, destination: &Path, resource_root: &Path) -> ProfileResult<()> {
    ensure_directory(destination)?;
    for name in runtime_paths(source, resource_root)? {
        let from = source.join(&name);
        let to = destination.join(name);
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            check_existing_chain(&from)?;
            fs::copy(&from, &to).map_err(|e| io(&from, e))?;
        }
    }
    copy_safe_game_sidecars(&source.join(".ccl"), &destination.join(".ccl"))?;
    Ok(())
}

fn restore_runtime_data(archive: &Path, source: &Path, resource_root: &Path) -> ProfileResult<()> {
    ensure_directory(source)?;
    let mut names = runtime_paths(source, resource_root)?;
    for item in runtime_paths(archive, resource_root)? {
        if !names.contains(&item) {
            names.push(item);
        }
    }
    for name in names {
        let from = archive.join(&name);
        let to = source.join(&name);
        if !from.exists() {
            if to.exists() {
                remove_path(&to)?;
            }
            continue;
        }
        let label = name.file_name().unwrap_or_default().to_string_lossy();
        let stage = source.join(format!(".ccl-profile-restore-{}-{label}", unique()));
        if from.is_dir() {
            copy_tree(&from, &stage)?;
        } else {
            fs::copy(&from, &stage).map_err(|e| io(&from, e))?;
        }
        let backup = source.join(format!(".ccl-profile-restore-backup-{}-{label}", unique()));
        if to.exists() {
            fs::rename(&to, &backup).map_err(|e| io(&to, e))?;
        }
        if let Err(error) = fs::rename(&stage, &to) {
            if backup.exists() {
                let _ = fs::rename(&backup, &to);
            }
            return Err(io(&to, error));
        }
        if backup.exists() {
            remove_path(&backup)?;
        }
    }
    Ok(())
}

fn file_digest(path: &Path) -> ProfileResult<String> {
    let bytes = fs::read(path).map_err(|e| io(path, e))?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}
fn remove_path(path: &Path) -> ProfileResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|e| io(path, e))?;
    if metadata.file_type().is_symlink() {
        return Err(ProfileError::UnsafePath(path.into()));
    }
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|e| io(path, e))?;
    } else {
        fs::remove_file(path).map_err(|e| io(path, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("ccl-profile-test-{}", unique()))
    }
    fn profile(root: &Path, id: &str, json: &str) {
        let dir = root.join("versions").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{id}.json")), json).unwrap();
        fs::write(dir.join(format!("{id}.jar")), b"profile jar").unwrap();
    }

    #[test]
    fn delete_preserves_shared_root_and_blocks_dependents() {
        let root = root();
        profile(&root, "base", r#"{"id":"base","libraries":[]}"#);
        profile(
            &root,
            "child",
            r#"{"id":"child","inheritsFrom":"base","libraries":[]}"#,
        );
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::write(root.join("assets/shared.bin"), b"shared").unwrap();
        let base = RootProfile::open(&root, "base").unwrap();
        assert_eq!(base.dependents().unwrap(), ["child"]);
        assert!(matches!(base.delete(), Err(ProfileError::Referenced(_))));
        fs::remove_dir_all(root.join("versions/child")).unwrap();
        base.delete().unwrap();
        assert!(!root.join("versions/base").exists());
        assert_eq!(fs::read(root.join("assets/shared.bin")).unwrap(), b"shared");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn open_does_not_create_or_migrate_profile_data() {
        let root = root();
        profile(&root, "plain", r#"{"id":"plain","libraries":[]}"#);
        fs::write(root.join("options.txt"), b"owned by root").unwrap();
        let before = fs::read(root.join("options.txt")).unwrap();
        let found = RootProfile::open(&root, "plain").unwrap();
        assert_eq!(found.game_directory(), fs::canonicalize(&root).unwrap());
        assert_eq!(fs::read(root.join("options.txt")).unwrap(), before);
        assert!(!root.join(".ccl/profiles").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinked_profile_path_is_rejected() {
        let root = root();
        fs::create_dir_all(&root).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(std::env::temp_dir(), root.join("versions")).unwrap();
            assert!(matches!(
                RootProfile::open(&root, "outside"),
                Err(ProfileError::UnsafePath(_))
            ));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_to_creates_only_an_independent_managed_instance() {
        let root = root();
        profile(&root, "1.20.1", r#"{"id":"1.20.1","libraries":[]}"#);
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::write(root.join("options.txt"), b"source options").unwrap();
        let profile = RootProfile::open(&root, "1.20.1").unwrap();
        let metadata = profile
            .copy_to(&root, "copy", DownloadConfig::default())
            .unwrap();
        let game = root.join(".ccl/instances/copy/game");
        assert_eq!(
            metadata.game_directory,
            fs::canonicalize(game.parent().unwrap())
                .unwrap()
                .join("game")
        );
        assert!(game.join("versions/1.20.1/1.20.1.json").is_file());
        assert!(game.join("versions/1.20.1/1.20.1.json").is_file());
        assert!(!root.join("versions/copy").exists());
        fs::write(root.join("options.txt"), b"changed source").unwrap();
        assert_eq!(
            fs::read(game.join("options.txt")).unwrap(),
            b"source options"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_flattens_custom_vanilla_metadata_into_effective_minecraft_version() {
        let root = root();
        profile(
            &root,
            "1.20.1",
            r#"{"id":"1.20.1","mainClass":"net.minecraft.client.main.Main","libraries":[],"downloads":{"client":{"url":"https://example.invalid/client.jar"}}}"#,
        );
        profile(
            &root,
            "MyWorld",
            r#"{"id":"MyWorld","inheritsFrom":"1.20.1","minecraftVersion":"1.20.1","mainClass":"custom.Main","arguments":{"game":["--demo"]},"libraries":[]}"#,
        );
        let profile = RootProfile::open(&root, "MyWorld").unwrap();
        assert_eq!(profile.metadata().minecraft, "1.20.1");
        let metadata = profile
            .copy_to(&root, "MyWorld-Copy", DownloadConfig::default())
            .unwrap();
        assert_eq!(metadata.minecraft, "1.20.1");
        let effective = root.join(".ccl/instances/MyWorld-Copy/game/versions/1.20.1/1.20.1.json");
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(effective).unwrap()).unwrap();
        assert_eq!(value["id"], "1.20.1");
        assert_eq!(value["mainClass"], "custom.Main");
        assert_eq!(value["arguments"]["game"][0], "--demo");
        assert!(
            value
                .get("inheritsFrom")
                .is_none_or(serde_json::Value::is_null)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_preserves_nested_loader_inheritance_and_client_jar_owner() {
        let root = root();
        profile(
            &root,
            "1.20.1",
            r#"{"id":"1.20.1","mainClass":"net.minecraft.client.main.Main","libraries":[{"name":"base:library:1"}],"downloads":{"client":{"url":"https://example.invalid/client.jar","sha1":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}}"#,
        );
        profile(
            &root,
            "fabric-loader-0.16.0-1.20.1",
            r#"{"id":"fabric-loader-0.16.0-1.20.1","inheritsFrom":"1.20.1","libraries":[{"name":"net.fabricmc:fabric-loader:0.16.0"}]}"#,
        );
        profile(
            &root,
            "MyNestedFabric",
            r#"{"id":"MyNestedFabric","inheritsFrom":"fabric-loader-0.16.0-1.20.1","mainClass":"custom.Main","arguments":{"game":["--custom"]},"libraries":[{"name":"child:library:1"}]}"#,
        );
        profile(
            &root,
            "client-override",
            r#"{"id":"client-override","inheritsFrom":"1.20.1"}"#,
        );
        profile(
            &root,
            "MyVanillaJar",
            r#"{"id":"MyVanillaJar","inheritsFrom":"1.20.1","jar":"client-override","mainClass":"custom.vanilla.Main","arguments":{"game":["--vanilla-custom"]}}"#,
        );

        let nested = RootProfile::open(&root, "MyNestedFabric").unwrap();
        assert_eq!(nested.metadata().loader, Some(LoaderKind::Fabric));
        let copied = nested
            .copy_to(&root, "nested-copy", DownloadConfig::default())
            .unwrap();
        let game = &copied.game_directory;
        let installed = launch::load_installed_metadata(game, "nested-copy").unwrap();
        assert!(
            installed
                .libraries
                .iter()
                .any(|library| library.name == "net.fabricmc:fabric-loader:0.16.0")
        );
        assert_eq!(installed.main_class.as_deref(), Some("custom.Main"));
        assert!(
            game.join("versions/fabric-loader-0.16.0-1.20.1/fabric-loader-0.16.0-1.20.1.json")
                .is_file()
        );
        assert!(game.join("versions/1.20.1/1.20.1.jar").is_file());

        let vanilla = RootProfile::open(&root, "MyVanillaJar").unwrap();
        assert_eq!(vanilla.metadata().loader, None);
        let copied = vanilla
            .copy_to(&root, "vanilla-copy", DownloadConfig::default())
            .unwrap();
        let flattened = launch::load_installed_metadata(&copied.game_directory, "1.20.1").unwrap();
        assert_eq!(flattened.main_class.as_deref(), Some("custom.vanilla.Main"));
        assert_eq!(
            flattened
                .extra
                .get("jar")
                .and_then(serde_json::Value::as_str),
            Some("client-override")
        );
        let flattened_json = serde_json::to_value(&flattened).unwrap();
        assert_eq!(flattened_json["arguments"]["game"][0], "--vanilla-custom");
        assert!(
            copied
                .game_directory
                .join("versions/client-override/client-override.jar")
                .is_file()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_edits_profile_runtime_and_restore_uses_history() {
        let root = root();
        profile(&root, "1.20.1", r#"{"id":"1.20.1","libraries":[]}"#);
        fs::create_dir_all(root.join("versions/1.20.1/mods")).unwrap();
        fs::write(root.join("versions/1.20.1/mods/original.jar"), b"old").unwrap();
        fs::create_dir_all(root.join(".ccl/instances/unrelated")).unwrap();
        fs::write(root.join(".ccl/instances/unrelated/keep.txt"), b"keep").unwrap();
        let profile = RootProfile::open(&root, "1.20.1").unwrap();
        assert_eq!(
            profile.game_directory(),
            fs::canonicalize(root.join("versions/1.20.1")).unwrap()
        );
        let workspace = profile.begin_workspace(DownloadConfig::default()).unwrap();
        assert_eq!(
            workspace.runtime_subdirectory(),
            Path::new("versions/1.20.1")
        );
        let source = root.join("incoming.jar");
        fs::write(&source, b"added").unwrap();
        workspace
            .content_service(DownloadConfig::default())
            .unwrap()
            .add(
                workspace.id(),
                crate::content::ContentKind::Mod,
                &source,
                None,
            )
            .unwrap();
        workspace.commit().unwrap();
        assert!(
            owned_workspace_dirs(profile.resource_root(), "1.20.1")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            fs::read(root.join("versions/1.20.1/mods/incoming.jar")).unwrap(),
            b"added"
        );
        assert_eq!(
            fs::read(root.join(".ccl/instances/unrelated/keep.txt")).unwrap(),
            b"keep"
        );
        let history = profile.history().unwrap();
        assert_eq!(history.len(), 1);
        profile.restore_revision(&history[0].revision).unwrap();
        assert!(!root.join("versions/1.20.1/mods/incoming.jar").exists());
        assert_eq!(
            fs::read(root.join("versions/1.20.1/mods/original.jar")).unwrap(),
            b"old"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_rejects_external_source_changes_without_overwriting() {
        let root = root();
        profile(&root, "plain", r#"{"id":"plain","libraries":[]}"#);
        fs::write(root.join("options.txt"), b"old").unwrap();
        let profile = RootProfile::open(&root, "plain").unwrap();
        let workspace = profile.begin_workspace(DownloadConfig::default()).unwrap();
        let private = workspace
            .root()
            .join(".ccl/instances/plain/game/options.txt");
        fs::write(&private, b"workspace").unwrap();
        fs::write(root.join("options.txt"), b"outside change").unwrap();
        assert!(matches!(workspace.commit(), Err(ProfileError::Conflict(_))));
        assert_eq!(
            fs::read(root.join("options.txt")).unwrap(),
            b"outside change"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_keeps_known_offline_caches_but_skips_account_data() {
        let root = root();
        profile(&root, "plain", r#"{"id":"plain","libraries":[]}"#);
        fs::create_dir_all(root.join(".ccl/cache")).unwrap();
        fs::create_dir_all(root.join(".ccl/runtimes/java-test")).unwrap();
        fs::create_dir_all(root.join(".ccl/profiles/private")).unwrap();
        fs::write(
            root.join(".ccl/cache/version_manifest.json"),
            b"old manifest",
        )
        .unwrap();
        fs::write(
            root.join(".ccl/runtimes/java-test/release"),
            b"runtime marker",
        )
        .unwrap();
        #[cfg(unix)]
        {
            fs::create_dir_all(root.join(".ccl/runtimes/java-test/lib")).unwrap();
            fs::create_dir_all(root.join(".ccl/runtimes/java-test/bin")).unwrap();
            fs::write(
                root.join(".ccl/runtimes/java-test/lib/libjvm.so"),
                b"runtime lib",
            )
            .unwrap();
            std::os::unix::fs::symlink(
                "../lib/libjvm.so",
                root.join(".ccl/runtimes/java-test/bin/java"),
            )
            .unwrap();
        }
        fs::write(root.join(".ccl/accounts.json"), b"secret").unwrap();
        fs::write(
            root.join(".ccl/profiles/private/bookkeeping.json"),
            b"private",
        )
        .unwrap();
        let root = fs::canonicalize(root).unwrap();
        let profile = RootProfile::open(&root, "plain").unwrap();
        let staged_check = root.with_extension("cache-copy-check");
        copy_resource_root(&root, &staged_check).unwrap();
        fs::remove_dir_all(&staged_check).unwrap();
        let workspace = profile.begin_workspace(DownloadConfig::default()).unwrap();
        let game = workspace.root().join(".ccl/instances/plain/game/.ccl");
        assert!(game.join("cache/version_manifest.json").is_file());
        assert!(game.join("runtimes/java-test/release").is_file());
        #[cfg(unix)]
        assert_eq!(
            fs::read_link(game.join("runtimes/java-test/bin/java")).unwrap(),
            PathBuf::from("../lib/libjvm.so")
        );
        assert!(!game.join("accounts.json").exists());
        assert!(!game.join("profiles/private/bookkeeping.json").exists());
        fs::write(game.join("cache/version_manifest.json"), b"new manifest").unwrap();
        workspace.commit().unwrap();
        assert_eq!(
            fs::read(root.join(".ccl/cache/version_manifest.json")).unwrap(),
            b"new manifest"
        );
        assert_eq!(
            fs::read(root.join(".ccl/accounts.json")).unwrap(),
            b"secret"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn offline_runtime_copy_rejects_links_escaping_component_root() {
        let root = root();
        profile(&root, "plain", r#"{"id":"plain","libraries":[]}"#);
        fs::create_dir_all(root.join(".ccl/runtimes/java-test/bin")).unwrap();
        fs::write(root.join("outside.txt"), b"outside").unwrap();
        std::os::unix::fs::symlink(
            "../../../../outside.txt",
            root.join(".ccl/runtimes/java-test/bin/escape"),
        )
        .unwrap();
        let profile = RootProfile::open(&root, "plain").unwrap();
        assert!(matches!(
            profile.begin_workspace(DownloadConfig::default()),
            Err(ProfileError::UnsafePath(_))
        ));
        assert_eq!(fs::read(root.join("outside.txt")).unwrap(), b"outside");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_only_targets_workspaces_with_matching_owner_marker() {
        let root = root();
        profile(&root, "plain", r#"{"id":"plain","libraries":[]}"#);
        let profile = RootProfile::open(&root, "plain").unwrap();
        let root = profile.resource_root().to_path_buf();
        let base = root.join(".ccl/profile-workspaces");
        let stale = base.join("some-random-directory");
        let other = base.join("other-profile-workspace");
        for (path, id) in [(&stale, "plain"), (&other, "another")] {
            let marker = path.join(".ccl/profile-workspace-owner.json");
            ensure_directory(marker.parent().unwrap()).unwrap();
            fs::write(
                marker,
                serde_json::to_vec(&WorkspaceOwner {
                    resource_root: fs::canonicalize(&root).unwrap(),
                    profile_id: id.into(),
                })
                .unwrap(),
            )
            .unwrap();
            fs::write(path.join("large.bin"), b"bytes").unwrap();
        }
        assert!(profile.disk_usage().unwrap().stage_bytes >= 5);
        let preview = profile.cleanup(0, true, false).unwrap();
        assert!(
            preview
                .targets
                .iter()
                .any(|target| target.revision == "workspace:some-random-directory")
        );
        assert!(stale.exists());
        profile.cleanup(0, true, true).unwrap();
        assert!(!stale.exists());
        assert!(other.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runtime_guards_allow_isolated_roots_and_block_shared_runtime() {
        let root = root();
        let first_runtime = absolute(&root.with_extension("runtime-a")).unwrap();
        let second_runtime = absolute(&root.with_extension("runtime-b")).unwrap();
        fs::create_dir_all(&first_runtime).unwrap();
        fs::create_dir_all(&second_runtime).unwrap();
        for id in ["alpha", "beta", "shared"] {
            profile(&root, id, &format!(r#"{{"id":"{id}","libraries":[]}}"#));
        }
        let root = fs::canonicalize(root).unwrap();
        for (id, path) in [
            ("alpha", &first_runtime),
            ("beta", &second_runtime),
            ("shared", &first_runtime),
        ] {
            let settings =
                serde_json::json!({"usesGlobal":false,"gameDirType":"CUSTOM","gameDir":path});
            fs::write(
                root.join("versions").join(id).join("hmclversion.cfg"),
                serde_json::to_vec(&settings).unwrap(),
            )
            .unwrap();
        }
        let alpha = RootProfile::open(&root, "alpha").unwrap();
        let beta = RootProfile::open(&root, "beta").unwrap();
        let shared = RootProfile::open(&root, "shared").unwrap();
        let alpha_guard = alpha.guard().unwrap();
        let beta_guard = beta.guard().unwrap();
        assert!(matches!(shared.guard(), Err(ProfileError::Conflict(_))));
        drop(beta_guard);
        drop(alpha_guard);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(first_runtime).unwrap();
        fs::remove_dir_all(second_runtime).unwrap();
    }

    #[test]
    fn guard_rejects_runtime_directory_changed_since_profile_open() {
        let root = root();
        profile(&root, "custom", r#"{"id":"custom","libraries":[]}"#);
        let settings = root.join("versions/custom/hmclversion.cfg");
        fs::write(
            &settings,
            serde_json::to_vec(
                &serde_json::json!({"usesGlobal":false,"gameDirType":"ROOT_FOLDER"}),
            )
            .unwrap(),
        )
        .unwrap();
        let profile = RootProfile::open(&root, "custom").unwrap();
        assert_eq!(profile.game_directory(), fs::canonicalize(&root).unwrap());
        let external = absolute(&root.with_extension("runtime-changed")).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(
            &settings,
            serde_json::to_vec(
                &serde_json::json!({"usesGlobal":false,"gameDirType":"CUSTOM","gameDir":external}),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(profile.guard(), Err(ProfileError::Conflict(_))));
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(external).unwrap();
    }

    #[test]
    fn opening_profile_recovers_interrupted_two_root_publish() {
        let root = root();
        let external = absolute(&root.with_extension("external")).unwrap();
        profile(&root, "custom", r#"{"id":"custom","libraries":[]}"#);
        let root = fs::canonicalize(root).unwrap();
        fs::create_dir_all(&external).unwrap();
        let hmcl =
            serde_json::json!({"usesGlobal":false,"gameDirType":"CUSTOM","gameDir":external});
        fs::write(
            root.join("versions/custom/hmclversion.cfg"),
            serde_json::to_vec(&hmcl).unwrap(),
        )
        .unwrap();
        fs::write(root.join("options.txt"), b"root-before").unwrap();
        fs::write(external.join("options.txt"), b"runtime-before").unwrap();
        let profile = RootProfile::open(&root, "custom").unwrap();
        let workspace = profile.begin_workspace(DownloadConfig::default()).unwrap();
        let game = workspace.root().join(".ccl/instances/custom/game");
        fs::write(game.join("options.txt"), b"root-after").unwrap();
        fs::write(
            game.join(".ccl-profile-runtime/options.txt"),
            b"runtime-after",
        )
        .unwrap();
        let manifest: WorkspaceManifest = serde_json::from_slice(
            &fs::read(workspace.root().join(".ccl/profile-workspace.json")).unwrap(),
        )
        .unwrap();
        let mut changes = plan_external_runtime(
            &game.join(".ccl-profile-runtime"),
            &external,
            &manifest.external_runtime,
        )
        .unwrap();
        changes.extend(plan_workspace_root(&game, &root, &manifest.resources, "custom").unwrap());
        let history = root.join(".ccl/profiles/history/custom/interrupted");
        ensure_directory(&history).unwrap();
        let journal = root.join(".ccl/profiles/journals/custom");
        ensure_directory(&journal).unwrap();
        for (i, change) in changes.iter().enumerate() {
            let target = change.target_root.join(&change.relative);
            let backup = journal.join("backup").join(i.to_string());
            let stage = journal.join("stage").join(i.to_string());
            ensure_directory(backup.parent().unwrap()).unwrap();
            ensure_directory(stage.parent().unwrap()).unwrap();
            fs::copy(&target, &backup).unwrap();
            fs::copy(change.workspace_root.join(&change.relative), &stage).unwrap();
        }
        write_journal(
            &journal.join("journal.json"),
            &JournalData {
                profile_id: "custom".into(),
                resource_root: root.clone(),
                runtime_root: external.clone(),
                changes: changes.clone(),
                committed: false,
                history_path: history.clone(),
            },
        )
        .unwrap();
        rename_replace(&journal.join("stage/0"), &external.join("options.txt")).unwrap();
        drop(workspace);
        RootProfile::open(&root, "custom").unwrap();
        assert_eq!(
            fs::read(external.join("options.txt")).unwrap(),
            b"runtime-before"
        );
        assert_eq!(fs::read(root.join("options.txt")).unwrap(), b"root-before");
        assert!(!history.exists());
        assert!(!journal.exists());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(external).unwrap();
    }
}
