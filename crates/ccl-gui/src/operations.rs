//! Toolkit-independent orchestration for the desktop application.

use crate::{GuiError, GuiResult};
use ccl_core::{
    accounts::OsSecretStore,
    config::{DownloadConfig, LauncherConfig},
    import_formats::{ImportFormatService, ImportOptionsExt},
    install::InstallService,
    instances::InstanceService,
    launch::{LaunchRequest as CoreLaunchRequest, LaunchResult, LaunchService},
    loaders::{LoaderKind, LoaderRequest, LoaderService},
    profiles::RootProfile,
    task::TaskContext,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
};

static PROFILE_IMPORT_ID: AtomicU64 = AtomicU64::new(0);

struct ResolvedLaunchTarget {
    resource_root: PathBuf,
    working_directory: PathBuf,
    version: String,
    profile_id: String,
    instance_guard: Option<ccl_core::instances::InstanceLaunchGuard>,
    profile_guard: Option<ccl_core::profiles::ProfileGuard>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
    pub id: String,
    pub minecraft: String,
    pub loader: Option<LoaderKind>,
    pub loader_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LaunchIdentityKind {
    Offline,
    Microsoft,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchIdentity {
    pub kind: LaunchIdentityKind,
    pub username: Option<String>,
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRequest {
    pub key: String,
    pub identity: LaunchIdentity,
    pub java: Option<PathBuf>,
    pub memory: Option<ccl_core::memory::MemoryPolicy>,
    pub auto_install_java: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    #[serde(default)]
    pub update_existing: bool,
    pub id: String,
    pub archive: PathBuf,
    pub include_optional: bool,
    pub replace_modified: bool,
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug)]
enum ImportTarget {
    CreateNew,
    ManagedInstance,
    RootProfile(RootProfile),
}

#[derive(Clone)]
pub struct GuiOperations {
    root: PathBuf,
    config_path: PathBuf,
    config: LauncherConfig,
    install_service: Option<InstallService>,
}

impl GuiOperations {
    pub fn new(root: PathBuf, config_path: PathBuf) -> GuiResult<Self> {
        let config = LauncherConfig::read_or_default(&config_path)
            .map_err(|error| GuiError::new("configuration_error", error.to_string()))?;
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()
                .map_err(|error| GuiError::new("directory_error", error.to_string()))?
                .join(root)
        };
        Ok(Self {
            root,
            config_path,
            config,
            install_service: None,
        })
    }

    /// Inject a configured installer for Rust embedding and local service fixtures.
    /// Desktop IPC should construct operations with `new` and the configured production sources.
    pub fn with_install_service(mut self, installer: InstallService) -> Self {
        self.install_service = Some(installer);
        self
    }

    fn installer(&self) -> GuiResult<InstallService> {
        match &self.install_service {
            Some(installer) => Ok(installer.clone()),
            None => InstallService::new(self.config.downloads.clone()).map_err(install_error),
        }
    }

    pub async fn list_versions(&self) -> GuiResult<Value> {
        let versions = self
            .installer()?
            .list_versions_for(&self.root)
            .await
            .map_err(install_error)?;
        Ok(Value::Array(
            versions
                .versions
                .into_iter()
                .map(|version| json!({"id": version.id, "kind": version.version_type}))
                .collect(),
        ))
    }

    pub async fn list_loaders(&self, minecraft: &str, kind: LoaderKind) -> GuiResult<Value> {
        let versions = LoaderService::new(self.config.downloads.clone())
            .map_err(loader_error)?
            .versions(kind, minecraft)
            .await
            .map_err(loader_error)?;
        serde_json::to_value(versions)
            .map_err(|error| GuiError::new("loader_failed", error.to_string()))
    }

    pub async fn install(&self, request: InstallRequest, context: TaskContext) -> GuiResult<Value> {
        let instances = InstanceService::new(self.root.clone(), self.config.downloads.clone())
            .map_err(instance_error)?;
        match (request.loader, request.loader_version.as_deref()) {
            (None, None) => {
                let instances = if let Some(installer) = &self.install_service {
                    instances.with_install_service(installer.clone())
                } else {
                    instances
                };
                let instance = instances
                    .install_vanilla(&request.id, &request.minecraft, Some(context))
                    .await
                    .map_err(instance_error)?;
                Ok(json!({"ok": true, "instance": instance}))
            }
            (Some(kind), Some(version)) => {
                if instances.metadata(&request.id).is_ok() {
                    return Err(GuiError::new(
                        "instance_conflict",
                        format!("Instance already exists: {}", request.id),
                    ));
                }
                let instance = instances
                    .install_new_loader(
                        &request.id,
                        &LoaderRequest {
                            kind,
                            minecraft: request.minecraft,
                            version: version.into(),
                        },
                        Some(context),
                    )
                    .await
                    .map_err(instance_error)?;
                Ok(json!({"ok": true, "instance": instance}))
            }
            _ => Err(GuiError::new(
                "invalid_install",
                "Choose both a loader and loader version, or neither.",
            )),
        }
    }

    pub fn inspect_import(&self, path: &Path) -> GuiResult<Value> {
        let manifest = ImportFormatService::inspect(path).map_err(import_format_error)?;
        Ok(json!({"ok": true, "manifest": manifest}))
    }

    pub async fn import(&self, request: ImportRequest, context: TaskContext) -> GuiResult<Value> {
        let target = select_import_target(
            &self.root,
            &self.config.downloads,
            &request.id,
            request.update_existing,
        )?;
        let inspected =
            ImportFormatService::inspect(&request.archive).map_err(import_format_error)?;
        let is_curseforge = inspected["format"] == "curseforge";
        let key = if is_curseforge {
            crate::provider_credentials::curseforge_key(
                &self.config_path,
                self.config.downloads.offline,
            )?
        } else {
            None
        };
        let options = ImportOptionsExt {
            include_optional: request.include_optional,
            replace_modified: request.replace_modified,
            curseforge_api_key: key,
            allowed_hosts: request.allowed_hosts.clone(),
        };
        if matches!(&target, ImportTarget::CreateNew) {
            let instance =
                ImportFormatService::new(self.root.clone(), self.config.downloads.clone())
                    .import_new(&request.id, &request.archive, &options, Some(context))
                    .await
                    .map_err(import_format_error)?;
            return Ok(json!({"ok": true, "instance": instance, "pack_imported": true}));
        }
        let instance = match target {
            ImportTarget::CreateNew => unreachable!(),
            ImportTarget::ManagedInstance => {
                let service =
                    ImportFormatService::new(self.root.clone(), self.config.downloads.clone());
                if request.update_existing {
                    service
                        .import_existing(&request.id, &request.archive, &options, Some(context))
                        .await
                } else {
                    service
                        .import(&request.id, &request.archive, &options, Some(context))
                        .await
                }
                .map_err(import_format_error)?
            }
            ImportTarget::RootProfile(profile) => {
                self.import_into_root_profile(profile, &request, &options, context)
                    .await?
            }
        };
        Ok(json!({"ok": true, "instance": instance, "pack_imported": true}))
    }

    async fn import_into_root_profile(
        &self,
        profile: RootProfile,
        request: &ImportRequest,
        options: &ImportOptionsExt,
        context: TaskContext,
    ) -> GuiResult<ccl_core::instances::InstanceMetadata> {
        let workspace = profile
            .begin_workspace(self.config.downloads.clone())
            .map_err(profile_error)?;
        let root = workspace.root().to_path_buf();
        let runtime = workspace.runtime_directory();
        let receipt = runtime.join(".ccl/modpack.json");
        if receipt.is_file() {
            let imported = ImportFormatService::new_with_runtime_subdirectory(
                root,
                self.config.downloads.clone(),
                workspace.runtime_subdirectory().to_path_buf(),
            )
            .import_existing(&request.id, &request.archive, options, Some(context))
            .await
            .map_err(import_format_error);
            return match imported {
                Ok(_) => workspace.commit().map_err(profile_error),
                Err(error) => {
                    let _ = workspace.abort();
                    Err(error)
                }
            };
        }
        let scratch = format!(
            "ccl-import-{}-{}",
            std::process::id(),
            PROFILE_IMPORT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let imported = ImportFormatService::new(root.clone(), self.config.downloads.clone())
            .import_new(&scratch, &request.archive, options, Some(context))
            .await
            .map_err(import_format_error);
        if let Err(error) = imported {
            let _ = workspace.abort();
            return Err(error);
        }
        let merge =
            ccl_core::online_content::OnlineContentService::merge_imported_pack_into_profile(
                ccl_core::online_content::PackMergeTarget {
                    root: root.clone(),
                    config: self.config.downloads.clone(),
                    profile_id: profile.id().to_owned(),
                    runtime_subdirectory: workspace.runtime_subdirectory().to_path_buf(),
                },
                &scratch,
            );
        let _ = InstanceService::new(root, self.config.downloads.clone())
            .and_then(|instances| instances.delete(&scratch));
        match merge {
            Ok(_) => workspace.commit().map_err(profile_error),
            Err(error) => {
                let _ = workspace.abort();
                Err(profile_pack_error(error))
            }
        }
    }

    pub async fn launch(&self, request: LaunchRequest, context: TaskContext) -> GuiResult<Value> {
        let ResolvedLaunchTarget {
            resource_root,
            working_directory,
            version,
            profile_id,
            instance_guard: _instance_guard,
            profile_guard: _profile_guard,
        } = self.resolve_launch_key(&request.key)?;
        let identity = match request.identity.kind {
            LaunchIdentityKind::Offline => None,
            LaunchIdentityKind::Microsoft => {
                let id = request.identity.account_id.as_deref().ok_or_else(|| {
                    GuiError::new("account_missing", "Select a Microsoft account first.")
                })?;
                let account_path = self.config_path.with_file_name("accounts.json");
                let secrets = Arc::new(
                    OsSecretStore::for_account_file(&account_path).map_err(account_error)?,
                );
                let identity = crate::account_service::AccountService::new(
                    &self.config_path,
                    secrets,
                    self.config.clone(),
                )
                .launch_identity(id, &context.cancellation)
                .await?;
                Some(identity)
            }
        };
        let username = match (&identity, request.identity.username) {
            (Some(identity), _) => identity.username.clone(),
            (None, Some(username)) => username,
            (None, None) => "CCLPlayer".into(),
        };
        let memory = self
            .config
            .launch
            .memory_policy(&resource_root, &profile_id, request.memory)
            .map_err(|error| GuiError::new("configuration_error", error.to_string()))?;
        let launch_request = CoreLaunchRequest {
            game_directory: resource_root,
            working_directory: Some(working_directory),
            version: version.clone(),
            profile_id: Some(profile_id.clone()),
            username,
            java: request.java.or(self.config.launch.java.clone()),
            memory,
            identity,
            auto_install_java: request.auto_install_java,
        };
        let service = LaunchService::new(self.config.downloads.clone());
        if request.dry_run {
            let prepared = service
                .prepare(&launch_request, Some(context))
                .await
                .map_err(launch_error)?;
            return Ok(json!({"ok": true, "prepared": prepared}));
        }
        let result = service
            .launch(&launch_request, Some(context))
            .await
            .map_err(launch_error)?;
        Ok(json!({
            "ok": result.exit_code == Some(0) && !result.cancelled,
            "launch": result
        }))
    }

    pub fn read_run_log(&self, key: &str, run_id: Option<&str>, lines: usize) -> GuiResult<Value> {
        let entry = self.library_entry(key)?;
        let root = entry.resource_root.clone();
        let directory = root.join(".ccl/logs");
        if !check_log_directory(&directory)? {
            return Err(GuiError::new("log_read", "No launch logs found"));
        }
        let run_id = match run_id {
            Some(run_id) if valid_run_id(run_id) => run_id.to_owned(),
            Some(_) => return Err(GuiError::new("log_read", "Invalid run ID")),
            None => latest_run_id(&directory, &entry.id, &entry.launch_version)?,
        };
        if !run_belongs_to_profile(&directory, &run_id, &entry.id, &entry.launch_version)? {
            return Err(GuiError::new(
                "log_read",
                "Launch log does not belong to this profile",
            ));
        }
        let path = directory.join(format!("{run_id}.log"));
        let linked = fs::symlink_metadata(&path)
            .map_err(|error| GuiError::new("log_read", error.to_string()))?;
        if linked.file_type().is_symlink() || !linked.is_file() {
            return Err(GuiError::new(
                "log_read",
                "Launch log is not a regular file",
            ));
        }
        const MAX_BYTES: u64 = 1024 * 1024;
        const MAX_LINES: usize = 1000;
        const MAX_LINE_CHARS: usize = 8192;
        let mut file =
            fs::File::open(&path).map_err(|error| GuiError::new("log_read", error.to_string()))?;
        let length = linked.len();
        let offset = length.saturating_sub(MAX_BYTES);
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| GuiError::new("log_read", error.to_string()))?;
        let mut bytes = Vec::with_capacity((length - offset) as usize);
        file.take(MAX_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|error| GuiError::new("log_read", error.to_string()))?;
        let mut truncated = offset > 0;
        if offset > 0
            && let Some(newline) = bytes.iter().position(|byte| *byte == b'\n')
        {
            bytes.drain(..=newline);
        }
        let requested_lines = lines.min(MAX_LINES);
        let mut tail = std::collections::VecDeque::new();
        for line in String::from_utf8_lossy(&bytes).lines() {
            let mut line = line.to_owned();
            if line.chars().count() > MAX_LINE_CHARS {
                line = line
                    .chars()
                    .skip(line.chars().count() - MAX_LINE_CHARS)
                    .collect();
                truncated = true;
            }
            if requested_lines > 0 {
                tail.push_back(line);
                if tail.len() > requested_lines {
                    tail.pop_front();
                }
            }
        }
        Ok(
            json!({"run_id": run_id, "text": tail.into_iter().collect::<Vec<_>>().join("\n"), "truncated": truncated}),
        )
    }

    pub fn log_runs(&self, key: &str) -> GuiResult<Value> {
        let entry = self.library_entry(key)?;
        let root = entry.resource_root.clone();
        let directory = root.join(".ccl/logs");
        if !check_log_directory(&directory)? {
            return Ok(json!([]));
        }
        let mut runs = Vec::<LaunchResult>::new();
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(json!([]));
            }
            Err(error) => return Err(GuiError::new("log_read", error.to_string())),
        };
        for file_entry in entries.take(10_000) {
            let file_entry =
                file_entry.map_err(|error| GuiError::new("log_read", error.to_string()))?;
            let path = file_entry.path();
            if file_entry
                .file_type()
                .map_err(|error| GuiError::new("log_read", error.to_string()))?
                .is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                && fs::metadata(&path)
                    .map(|metadata| metadata.len() <= 64 * 1024)
                    .unwrap_or(false)
                && runs.len() < 1000
            {
                let bytes = fs::read(&path)
                    .map_err(|error| GuiError::new("log_read", error.to_string()))?;
                if let Ok(run) = serde_json::from_slice::<LaunchResult>(&bytes)
                    && run_belongs_to_entry(&run, &entry.id, &entry.launch_version)
                {
                    runs.push(run);
                }
            }
        }
        runs.sort_by_key(|run| std::cmp::Reverse(run.started_at_ms));
        serde_json::to_value(runs).map_err(|error| GuiError::new("log_read", error.to_string()))
    }

    fn resolve_launch_key(&self, key: &str) -> GuiResult<ResolvedLaunchTarget> {
        let entry = self.library_entry(key)?;
        if entry.has_revision_storage {
            let service = InstanceService::new(self.root.clone(), self.config.downloads.clone())
                .map_err(instance_error)?;
            let guard = service.acquire_launch(&entry.id).map_err(instance_error)?;
            Ok(ResolvedLaunchTarget {
                resource_root: entry.resource_root,
                working_directory: entry.game_directory,
                version: guard.version().to_owned(),
                profile_id: entry.id,
                instance_guard: Some(guard),
                profile_guard: None,
            })
        } else {
            let guard = ccl_core::profiles::RootProfile::open(&self.root, &entry.id)
                .and_then(|profile| profile.guard())
                .map_err(|error| GuiError::new("instance_busy", error.to_string()))?;
            Ok(ResolvedLaunchTarget {
                resource_root: entry.resource_root,
                working_directory: entry.game_directory,
                version: entry.launch_version,
                profile_id: entry.id,
                instance_guard: None,
                profile_guard: Some(guard),
            })
        }
    }

    fn library_entry(&self, key: &str) -> GuiResult<crate::LibraryEntry> {
        crate::discover_entries(&self.root, &self.config.downloads)?
            .into_iter()
            .find(|entry| entry.key == key)
            .ok_or_else(invalid_launch_key)
    }
}

fn invalid_launch_key() -> GuiError {
    GuiError::new(
        "invalid_launch_key",
        "Select an installed game from the library.",
    )
}
fn check_log_directory(directory: &Path) -> GuiResult<bool> {
    let game = directory
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| GuiError::new("log_read", "Invalid launch log directory"))?;
    for path in [game.join(".ccl"), directory.to_path_buf()] {
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(GuiError::new(
                    "log_read",
                    "Launch log path contains a symlink",
                ));
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(GuiError::new(
                    "log_read",
                    "Launch log path is not a directory",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(GuiError::new("log_read", error.to_string())),
        }
    }
    Ok(true)
}
fn valid_run_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}
fn latest_run_id(directory: &Path, profile_id: &str, legacy_version: &str) -> GuiResult<String> {
    let entries =
        fs::read_dir(directory).map_err(|error| GuiError::new("log_read", error.to_string()))?;
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| GuiError::new("log_read", error.to_string()))?;
        let path = entry.path();
        if entry
            .file_type()
            .map_err(|error| GuiError::new("log_read", error.to_string()))?
            .is_file()
            && path.extension().is_some_and(|extension| extension == "log")
            && let Some(id) = path.file_stem().and_then(|stem| stem.to_str())
            && valid_run_id(id)
            && run_belongs_to_profile(directory, id, profile_id, legacy_version)?
        {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    ids.pop()
        .ok_or_else(|| GuiError::new("log_read", "No launch logs found"))
}

fn run_belongs_to_profile(
    directory: &Path,
    run_id: &str,
    profile_id: &str,
    legacy_version: &str,
) -> GuiResult<bool> {
    let path = directory.join(format!("{run_id}.json"));
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => metadata,
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(GuiError::new("log_read", error.to_string())),
    };
    if metadata.len() > 64 * 1024 {
        return Ok(false);
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(GuiError::new("log_read", error.to_string())),
    };
    let run: LaunchResult = match serde_json::from_slice(&bytes) {
        Ok(run) => run,
        Err(_) => return Ok(false),
    };
    Ok(run.run_id == run_id && run_belongs_to_entry(&run, profile_id, legacy_version))
}

fn run_belongs_to_entry(run: &LaunchResult, profile_id: &str, legacy_version: &str) -> bool {
    run.profile_id
        .as_deref()
        .map_or(run.version == legacy_version, |id| id == profile_id)
}

#[cfg(test)]
mod log_identity_tests {
    use super::run_belongs_to_entry;
    use ccl_core::launch::LaunchResult;
    use std::path::PathBuf;

    fn run(profile_id: Option<&str>, version: &str) -> LaunchResult {
        LaunchResult {
            run_id: "fixture-run".into(),
            version: version.into(),
            profile_id: profile_id.map(str::to_owned),
            pid: 1,
            exit_code: Some(0),
            cancelled: false,
            log_path: PathBuf::from("fixture.log"),
            started_at_ms: 1,
            finished_at_ms: 2,
        }
    }

    #[test]
    fn current_and_legacy_logs_are_scoped_to_profile() {
        assert!(run_belongs_to_entry(
            &run(Some("profile-a"), "loader"),
            "profile-a",
            "game-a"
        ));
        assert!(!run_belongs_to_entry(
            &run(Some("profile-b"), "game-a"),
            "profile-a",
            "game-a"
        ));
        assert!(run_belongs_to_entry(
            &run(None, "game-a"),
            "profile-a",
            "game-a"
        ));
        assert!(!run_belongs_to_entry(
            &run(None, "game-b"),
            "profile-a",
            "game-a"
        ));
    }
}

fn install_error(error: ccl_core::install::InstallError) -> GuiError {
    let code = if matches!(
        &error,
        ccl_core::install::InstallError::Cancelled
            | ccl_core::install::InstallError::Download(
                ccl_core::download::DownloadError::Cancelled
            )
    ) {
        "cancelled"
    } else {
        "install_failed"
    };
    GuiError::new(code, error.to_string())
}
fn profile_error(error: ccl_core::profiles::ProfileError) -> GuiError {
    GuiError::new("profile_operation_failed", error.to_string())
}
fn loader_error(error: ccl_core::loaders::LoaderError) -> GuiError {
    GuiError::new(
        if error.is_cancelled() {
            "cancelled"
        } else {
            "loader_failed"
        },
        error.to_string(),
    )
}
fn instance_error(error: ccl_core::instances::InstanceError) -> GuiError {
    GuiError::new(
        if error.is_cancelled() {
            "cancelled"
        } else if matches!(error, ccl_core::instances::InstanceError::Conflict(_)) {
            "instance_conflict"
        } else {
            "instance_failed"
        },
        error.to_string(),
    )
}
fn import_format_error(error: ccl_core::import_formats::ImportFormatError) -> GuiError {
    let conflict = matches!(
        &error,
        ccl_core::import_formats::ImportFormatError::Pack(
            ccl_core::modpacks::ModpackError::Instance(
                ccl_core::instances::InstanceError::Conflict(_)
            ) | ccl_core::modpacks::ModpackError::Conflict { .. }
        )
    );
    GuiError::new(
        if error.is_cancelled() {
            "cancelled"
        } else if conflict {
            "instance_conflict"
        } else {
            "import_failed"
        },
        error.to_string(),
    )
}

fn select_import_target(
    root: &Path,
    config: &DownloadConfig,
    id: &str,
    update_existing: bool,
) -> GuiResult<ImportTarget> {
    let profile = match RootProfile::open(root, id) {
        Ok(profile) => Some(profile),
        Err(ccl_core::profiles::ProfileError::NotFound(_)) => None,
        Err(error) => return Err(profile_error(error)),
    };
    let managed = match InstanceService::new(root.to_path_buf(), config.clone())
        .map_err(instance_error)?
        .metadata(id)
    {
        Ok(_) => true,
        Err(ccl_core::instances::InstanceError::NotFound(_)) => false,
        Err(error) => return Err(instance_error(error)),
    };
    if !update_existing {
        if profile.is_some() || managed {
            return Err(GuiError::new(
                "instance_conflict",
                format!("Instance already exists: {id}"),
            ));
        }
        return Ok(ImportTarget::CreateNew);
    }
    match (profile, managed) {
        (Some(_), true) => Err(GuiError::new(
            "ambiguous_instance",
            "This ID matches both an ordinary profile and a CCL instance; choose a unique ID before importing",
        )),
        (Some(profile), false) => Ok(ImportTarget::RootProfile(profile)),
        (None, true) => Ok(ImportTarget::ManagedInstance),
        (None, false) => Err(GuiError::new(
            "instance_not_found",
            format!("No existing profile or CCL instance named {id}"),
        )),
    }
}

fn profile_pack_error(error: ccl_core::online_content::OnlineError) -> GuiError {
    GuiError::new(
        if error.is_cancelled() {
            "cancelled"
        } else if matches!(
            error,
            ccl_core::online_content::OnlineError::Conflict { .. }
        ) {
            "online_content_conflict"
        } else {
            "pack_import_failed"
        },
        error.to_string(),
    )
}

#[cfg(test)]
mod import_target_tests {
    use super::*;

    #[test]
    fn root_profile_update_routes_to_the_profile_without_creating_a_managed_id() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let profile = root.join("versions/plain");
        fs::create_dir_all(profile.join("mods")).unwrap();
        fs::write(
            profile.join("plain.json"),
            r#"{"id":"plain","libraries":[]}"#,
        )
        .unwrap();

        assert!(matches!(
            select_import_target(&root, &DownloadConfig::default(), "plain", false),
            Err(GuiError { code, .. }) if code == "instance_conflict"
        ));
        let target =
            select_import_target(&root, &DownloadConfig::default(), "plain", true).unwrap();
        assert!(matches!(target, ImportTarget::RootProfile(_)));
        assert!(!root.join(".ccl/instances/plain").exists());
    }

    #[test]
    fn local_pack_import_rejects_same_id_across_profile_and_managed_instance() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let profile = root.join("versions/same");
        fs::create_dir_all(profile.join("mods")).unwrap();
        fs::write(profile.join("same.json"), r#"{"id":"same","libraries":[]}"#).unwrap();
        let instance = root.join(".ccl/instances/same");
        let game = instance.join("game");
        fs::create_dir_all(&game).unwrap();
        fs::write(
            instance.join("instance.json"),
            serde_json::to_vec(&ccl_core::instances::InstanceMetadata {
                id: "same".into(),
                minecraft: "1.21.1".into(),
                loader: None,
                loader_version: None,
                game_directory: game,
            })
            .unwrap(),
        )
        .unwrap();
        let target = select_import_target(&root, &DownloadConfig::default(), "same", true);
        assert!(
            matches!(target, Err(GuiError { code, .. }) if code == "ambiguous_instance"),
            "{target:?}"
        );
    }
}
fn launch_error(error: ccl_core::launch::LaunchError) -> GuiError {
    let cancelled = matches!(
        &error,
        ccl_core::launch::LaunchError::Cancelled
            | ccl_core::launch::LaunchError::Download(ccl_core::download::DownloadError::Cancelled)
            | ccl_core::launch::LaunchError::Runtime(ccl_core::runtime::RuntimeError::Cancelled)
            | ccl_core::launch::LaunchError::Runtime(ccl_core::runtime::RuntimeError::Download(
                ccl_core::download::DownloadError::Cancelled
            ))
    );
    GuiError::new(
        if cancelled {
            "cancelled"
        } else {
            // Keep useful, non-sensitive failure categories in launcher.log.
            match &error {
                ccl_core::launch::LaunchError::Plan(
                    ccl_core::launch_plan::PlanError::UnknownPlaceholder(_),
                ) => "launch_unsupported_placeholder",
                ccl_core::launch::LaunchError::Plan(_) => "launch_plan_failed",
                ccl_core::launch::LaunchError::Memory(_) => "memory_allocation_failed",
                ccl_core::launch::LaunchError::Java(_) => "launch_java_failed",
                _ => "launch_failed",
            }
        },
        error.to_string(),
    )
}
fn account_error(error: ccl_core::accounts::AccountError) -> GuiError {
    GuiError::new("account_storage", error.to_string())
}

#[cfg(test)]
mod launch_key_tests {
    use super::*;

    #[test]
    fn launch_errors_keep_safe_diagnostic_categories() {
        use ccl_core::{launch::LaunchError, launch_plan::PlanError};
        let error = launch_error(LaunchError::Plan(PlanError::UnknownPlaceholder(
            "private-placeholder-value".into(),
        )));
        assert_eq!(error.code, "launch_unsupported_placeholder");
        assert_eq!(
            launch_error(LaunchError::Plan(PlanError::InvalidVersion)).code,
            "launch_plan_failed"
        );
    }

    #[test]
    fn launch_keys_must_resolve_to_a_discovered_instance() {
        let temp = tempfile::tempdir().unwrap();
        let operations =
            GuiOperations::new(temp.path().into(), temp.path().join("config.json")).unwrap();
        for key in [
            "vanilla:1.20.1",
            "managed:旧实例",
            "../outside",
            "instance:missing",
        ] {
            assert_eq!(
                operations.resolve_launch_key(key).err().unwrap().code,
                "invalid_launch_key"
            );
        }
    }
}
