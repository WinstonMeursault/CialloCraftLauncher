//! Toolkit-independent orchestration for the desktop application.

use crate::{GuiError, GuiResult};
use ccl_core::{
    accounts::{OsProviderKeyStore, OsSecretStore},
    config::LauncherConfig,
    import_formats::{ImportFormatService, ImportOptionsExt},
    install::{InstallOptions, InstallService},
    instances::InstanceService,
    launch::{LaunchRequest as CoreLaunchRequest, LaunchResult, LaunchService},
    loaders::{LoaderKind, LoaderRequest, LoaderService},
    task::TaskContext,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
    pub id: String,
    pub minecraft: String,
    pub loader: Option<LoaderKind>,
    pub loader_version: Option<String>,
    pub shared: bool,
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
    pub memory_mb: Option<u32>,
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
        if request.shared {
            if request.loader.is_some() || request.loader_version.is_some() {
                return Err(GuiError::new(
                    "invalid_install",
                    "Shared installs support vanilla Minecraft versions only.",
                ));
            }
            let summary = self
                .installer()?
                .install(
                    &request.minecraft,
                    &InstallOptions::for_current_platform(&self.root),
                    Some(context),
                )
                .await
                .map_err(install_error)?;
            return Ok(json!({"ok": true, "shared": true, "installation": summary}));
        }

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
        let import = ImportFormatService::new(self.root.clone(), self.config.downloads.clone());
        let inspected =
            ImportFormatService::inspect(&request.archive).map_err(import_format_error)?;
        let is_curseforge = inspected["format"] == "curseforge";
        let key = if is_curseforge {
            provider_key(&self.config_path)?
        } else {
            None
        };
        let options = ImportOptionsExt {
            include_optional: request.include_optional,
            replace_modified: request.replace_modified,
            curseforge_api_key: key,
            allowed_hosts: request.allowed_hosts,
        };
        let instance = if request.update_existing {
            // Updating an existing pack is explicitly selected in the GUI.
            import
                .import(&request.id, &request.archive, &options, Some(context))
                .await
        } else {
            import
                .import_new(&request.id, &request.archive, &options, Some(context))
                .await
        }
        .map_err(import_format_error)?;
        Ok(json!({"ok": true, "instance": instance, "pack_imported": true}))
    }

    pub async fn launch(&self, request: LaunchRequest, context: TaskContext) -> GuiResult<Value> {
        let (game_directory, version, _guard) = self.resolve_launch_key(&request.key)?;
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
        let launch_request = CoreLaunchRequest {
            game_directory: game_directory.clone(),
            version: version.clone(),
            username,
            java: request.java.or(self.config.launch.java.clone()),
            max_memory_mb: request
                .memory_mb
                .unwrap_or(self.config.launch.max_memory_mb),
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
        let root = self.game_root_for_key(key)?;
        let directory = root.join(".ccl/logs");
        if !check_log_directory(&directory)? {
            return Err(GuiError::new("log_read", "No launch logs found"));
        }
        let run_id = match run_id {
            Some(run_id) if valid_run_id(run_id) => run_id.to_owned(),
            Some(_) => return Err(GuiError::new("log_read", "Invalid run ID")),
            None => latest_run_id(&directory)?,
        };
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
        let root = self.game_root_for_key(key)?;
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
        for entry in entries.take(10_000) {
            let entry = entry.map_err(|error| GuiError::new("log_read", error.to_string()))?;
            let path = entry.path();
            if entry
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
                if let Ok(run) = serde_json::from_slice::<LaunchResult>(&bytes) {
                    runs.push(run);
                }
            }
        }
        runs.sort_by_key(|run| std::cmp::Reverse(run.started_at_ms));
        serde_json::to_value(runs).map_err(|error| GuiError::new("log_read", error.to_string()))
    }

    fn resolve_launch_key(
        &self,
        key: &str,
    ) -> GuiResult<(
        PathBuf,
        String,
        Option<ccl_core::instances::InstanceLaunchGuard>,
    )> {
        let (kind, id) = key.split_once(':').ok_or_else(invalid_launch_key)?;
        if id.is_empty() || id.contains(':') {
            return Err(invalid_launch_key());
        }
        match kind {
            "vanilla" => {
                validate_key_id(id)?;
                Ok((self.root.clone(), id.to_owned(), None))
            }
            "managed" => {
                let service =
                    InstanceService::new(self.root.clone(), self.config.downloads.clone())
                        .map_err(instance_error)?;
                let guard = service.acquire_launch(id).map_err(instance_error)?;
                let game = guard.game_directory().to_path_buf();
                let version = guard.version().to_owned();
                Ok((game, version, Some(guard)))
            }
            _ => Err(invalid_launch_key()),
        }
    }

    fn game_root_for_key(&self, key: &str) -> GuiResult<PathBuf> {
        let (kind, id) = key.split_once(':').ok_or_else(invalid_launch_key)?;
        if id.is_empty() || id.contains(':') {
            return Err(invalid_launch_key());
        }
        match kind {
            "vanilla" => {
                validate_key_id(id)?;
                Ok(self.root.clone())
            }
            "managed" => InstanceService::new(self.root.clone(), self.config.downloads.clone())
                .map_err(instance_error)?
                .game_directory(id)
                .map_err(instance_error),
            _ => Err(invalid_launch_key()),
        }
    }
}

fn provider_key(config_path: &Path) -> GuiResult<Option<String>> {
    let store = OsProviderKeyStore::for_config_file(config_path).map_err(account_error)?;
    match store.get_curseforge() {
        Err(ccl_core::accounts::AccountError::SecureStorage(_)) => Ok(None),
        result => result.map_err(account_error),
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
fn latest_run_id(directory: &Path) -> GuiResult<String> {
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
        {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    ids.pop()
        .ok_or_else(|| GuiError::new("log_read", "No launch logs found"))
}
fn validate_key_id(id: &str) -> GuiResult<()> {
    let stem = id.split('.').next().unwrap_or(id).trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && matches!(upper.as_bytes()[3], b'1'..=b'9'));
    if id.is_empty()
        || matches!(id, "." | "..")
        || reserved
        || id.ends_with(['.', ' '])
        || id
            .chars()
            .any(|character| character.is_control() || "<>:\"/\\|?*".contains(character))
    {
        return Err(invalid_launch_key());
    }
    Ok(())
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
            "launch_failed"
        },
        error.to_string(),
    )
}
fn account_error(error: ccl_core::accounts::AccountError) -> GuiError {
    GuiError::new("account_storage", error.to_string())
}

#[cfg(test)]
mod launch_key_tests {
    use super::validate_key_id;

    #[test]
    fn launch_path_components_accept_unicode_and_safe_punctuation() {
        for id in ["1.20.1", "我的整合包", "日本語 pack! (1)"] {
            assert!(validate_key_id(id).is_ok(), "{id}");
        }
        for id in [
            "",
            ".",
            "..",
            "../outside",
            "a/b",
            "a\\b",
            "name:",
            "CON.txt",
            "COM1.save",
        ] {
            assert!(validate_key_id(id).is_err(), "{id}");
        }
    }
}
