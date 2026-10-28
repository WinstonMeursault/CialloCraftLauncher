//! Typed GUI adapter for instance files, revisions and local content.
//!
//! This adapter deliberately accepts paths only for local archive/content inputs and
//! outputs. The desktop command layer is responsible for obtaining them from native
//! file pickers and applying its own allowlist before constructing a request.

use ccl_core::{
    config::DownloadConfig,
    content::{ContentKind, ContentService},
    import_formats::ImportFormatService,
    instances::InstanceService,
    loaders::{LoaderKind, LoaderRequest},
    modpacks::ModpackService,
    profiles::RootProfile,
    task::TaskContext,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use crate::{GuiError, GuiResult};

/// One strongly typed request for the instance and content commands exposed by the GUI.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum InstanceRequest {
    List,
    Show {
        id: String,
    },
    Install {
        id: String,
        minecraft: String,
        loader: LoaderKind,
        loader_version: String,
    },
    Update {
        id: String,
        minecraft: String,
        loader: LoaderKind,
        loader_version: String,
    },
    UninstallLoader {
        id: String,
    },
    Copy {
        id: String,
        destination: String,
    },
    Delete {
        id: String,
    },
    History {
        id: String,
    },
    Restore {
        id: String,
        revision: String,
    },
    Rollback {
        id: String,
    },
    DiskUsage {
        id: String,
    },
    Clean {
        id: String,
        keep: usize,
        include_previous: bool,
        apply: bool,
    },
    PackInspect {
        archive: PathBuf,
    },
    PackShow {
        id: String,
    },
    PackExport {
        id: String,
        output: PathBuf,
        include_local_content: bool,
    },
    ContentList {
        id: String,
        kind: ContentKind,
    },
    ContentAdd {
        id: String,
        kind: ContentKind,
        source: PathBuf,
        name: Option<String>,
    },
    ContentRemove {
        id: String,
        kind: ContentKind,
        name: String,
    },
    ContentEnable {
        id: String,
        name: String,
    },
    ContentDisable {
        id: String,
        name: String,
    },
    ContentBackup {
        id: String,
        output: PathBuf,
    },
    ContentRestore {
        id: String,
        archive: PathBuf,
    },
}

impl InstanceRequest {
    pub fn target_id(&self) -> Option<&str> {
        match self {
            Self::Show { id }
            | Self::Install { id, .. }
            | Self::Update { id, .. }
            | Self::UninstallLoader { id }
            | Self::Copy { id, .. }
            | Self::Delete { id }
            | Self::History { id }
            | Self::Restore { id, .. }
            | Self::Rollback { id }
            | Self::DiskUsage { id }
            | Self::Clean { id, .. }
            | Self::PackShow { id }
            | Self::PackExport { id, .. }
            | Self::ContentList { id, .. }
            | Self::ContentAdd { id, .. }
            | Self::ContentRemove { id, .. }
            | Self::ContentEnable { id, .. }
            | Self::ContentDisable { id, .. }
            | Self::ContentBackup { id, .. }
            | Self::ContentRestore { id, .. } => Some(id),
            Self::List | Self::PackInspect { .. } => None,
        }
    }
    /// Whether execution can change instance files or metadata.
    pub fn mutates(&self) -> bool {
        !matches!(
            self,
            Self::List
                | Self::Show { .. }
                | Self::History { .. }
                | Self::DiskUsage { .. }
                | Self::PackInspect { .. }
                | Self::PackShow { .. }
                | Self::ContentList { .. }
                | Self::Clean { apply: false, .. }
        )
    }

    fn changes_instance(&self) -> bool {
        self.mutates() && !matches!(self, Self::PackExport { .. } | Self::ContentBackup { .. })
    }
}

/// Core-backed instance/content adapter independent of the desktop toolkit.
#[derive(Clone)]
pub struct InstanceOperations {
    root: PathBuf,
    config: DownloadConfig,
    profile: Option<RootProfile>,
    runtime_service: Option<InstanceService>,
}

impl InstanceOperations {
    pub fn new(root: PathBuf, config: DownloadConfig) -> Self {
        Self {
            root,
            config,
            profile: None,
            runtime_service: None,
        }
    }

    pub fn with_profile(mut self, profile: RootProfile) -> Self {
        self.profile = Some(profile);
        self
    }

    fn with_runtime_service(mut self, service: InstanceService) -> Self {
        self.runtime_service = Some(service);
        self
    }

    fn instances(&self) -> GuiResult<InstanceService> {
        self.runtime_service
            .clone()
            .map(Ok)
            .unwrap_or_else(|| InstanceService::new(self.root.clone(), self.config.clone()))
            .map_err(instance_error)
    }

    /// Execute a request. Supply a task context for cancellable loader changes and rollback.
    /// Core synchronous filesystem calls are short and remain synchronous as they are in CLI.
    pub async fn execute(
        &self,
        request: InstanceRequest,
        context: Option<TaskContext>,
    ) -> GuiResult<Value> {
        if let Some(context) = &context {
            context
                .cancellation
                .check()
                .map_err(|e| GuiError::new("cancelled", e.to_string()))?;
        }
        if let Some(profile) = self.profile.clone() {
            return self.execute_profile(profile, request, context).await;
        }
        match request {
            InstanceRequest::List => Ok(
                json!({"ok":true,"instances":crate::discover_entries(&self.root, &self.config)?}),
            ),
            InstanceRequest::Show { id } => Ok(
                json!({"ok":true,"instance":self.instances()?.metadata(&id).map_err(instance_error)?}),
            ),
            InstanceRequest::Install {
                id,
                minecraft,
                loader,
                loader_version,
            }
            | InstanceRequest::Update {
                id,
                minecraft,
                loader,
                loader_version,
            } => {
                let context = required_context(context)?;
                let instance = self
                    .instances()?
                    .install(
                        &id,
                        &LoaderRequest {
                            kind: loader,
                            minecraft,
                            version: loader_version,
                        },
                        context,
                    )
                    .await
                    .map_err(instance_error)?;
                Ok(json!({"ok":true,"instance":instance}))
            }
            InstanceRequest::UninstallLoader { id } => {
                let context = required_context(context)?;
                let instance = self
                    .instances()?
                    .uninstall_loader(&id, context)
                    .await
                    .map_err(instance_error)?;
                Ok(json!({"ok":true,"instance":instance}))
            }
            InstanceRequest::Copy { id, destination } => Ok(
                json!({"ok":true,"instance":self.instances()?.duplicate(&id, &destination).map_err(instance_error)?}),
            ),
            InstanceRequest::Delete { id } => {
                self.instances()?.delete(&id).map_err(instance_error)?;
                Ok(json!({"ok":true,"deleted":id}))
            }
            InstanceRequest::History { id } => Ok(
                json!({"ok":true,"history":self.instances()?.history(&id).map_err(instance_error)?}),
            ),
            InstanceRequest::Restore { id, revision } => Ok(
                json!({"ok":true,"instance":self.instances()?.restore_revision(&id, &revision).map_err(instance_error)?}),
            ),
            InstanceRequest::Rollback { id } => Ok(
                json!({"ok":true,"instance":self.instances()?.rollback(&id).await.map_err(instance_error)?}),
            ),
            InstanceRequest::DiskUsage { id } => Ok(
                json!({"ok":true,"disk_usage":self.instances()?.disk_usage(&id).map_err(instance_error)?}),
            ),
            InstanceRequest::Clean {
                id,
                keep,
                include_previous,
                apply,
            } => Ok(
                json!({"ok":true,"cleanup":self.instances()?.cleanup(&id, keep, include_previous, apply).map_err(instance_error)?}),
            ),
            InstanceRequest::PackInspect { archive } => {
                let manifest = if is_mrpack(&archive) {
                    json!(ModpackService::inspect(&archive).map_err(modpack_error)?)
                } else {
                    ImportFormatService::inspect(&archive).map_err(import_error)?
                };
                Ok(json!({"ok":true,"manifest":manifest}))
            }
            InstanceRequest::PackShow { id } => {
                let receipt = ModpackService::new(self.root.clone(), self.config.clone())
                    .map_err(modpack_error)?
                    .show(&id)
                    .map_err(modpack_error)?;
                Ok(json!({"ok":true,"receipt":receipt}))
            }
            InstanceRequest::PackExport {
                id,
                output,
                include_local_content,
            } => {
                let manifest = ModpackService::new(self.root.clone(), self.config.clone())
                    .map_err(modpack_error)?
                    .export(&id, &output, include_local_content)
                    .map_err(modpack_error)?;
                Ok(json!({"ok":true,"archive":output,"manifest":manifest}))
            }
            InstanceRequest::ContentList { id, kind } => {
                let content = ContentService::new(self.root.clone(), self.config.clone())
                    .map_err(content_error)?
                    .list(&id, kind)
                    .map_err(content_error)?;
                Ok(json!({"ok":true,"id":id,"kind":kind,"content":content}))
            }
            InstanceRequest::ContentAdd {
                id,
                kind,
                source,
                name,
            } => {
                let entry = ContentService::new(self.root.clone(), self.config.clone())
                    .map_err(content_error)?
                    .add(&id, kind, &source, name.as_deref())
                    .map_err(content_error)?;
                Ok(json!({"ok":true,"id":id,"kind":kind,"entry":entry}))
            }
            InstanceRequest::ContentRemove { id, kind, name } => {
                ContentService::new(self.root.clone(), self.config.clone())
                    .map_err(content_error)?
                    .remove(&id, kind, &name)
                    .map_err(content_error)?;
                Ok(json!({"ok":true,"id":id,"kind":kind,"removed":name}))
            }
            InstanceRequest::ContentEnable { id, name } => {
                let enabled = true;
                let entry = ContentService::new(self.root.clone(), self.config.clone())
                    .map_err(content_error)?
                    .set_enabled(&id, &name, enabled)
                    .map_err(content_error)?;
                Ok(json!({"ok":true,"id":id,"entry":entry}))
            }
            InstanceRequest::ContentDisable { id, name } => {
                let enabled = false;
                let entry = ContentService::new(self.root.clone(), self.config.clone())
                    .map_err(content_error)?
                    .set_enabled(&id, &name, enabled)
                    .map_err(content_error)?;
                Ok(json!({"ok":true,"id":id,"entry":entry}))
            }
            InstanceRequest::ContentBackup { id, output } => {
                ContentService::new(self.root.clone(), self.config.clone())
                    .map_err(content_error)?
                    .backup(&id, &output)
                    .map_err(content_error)?;
                Ok(json!({"ok":true,"id":id,"backup":output}))
            }
            InstanceRequest::ContentRestore { id, archive } => {
                ContentService::new(self.root.clone(), self.config.clone())
                    .map_err(content_error)?
                    .restore(&id, &archive)
                    .map_err(content_error)?;
                Ok(json!({"ok":true,"id":id,"restored":true}))
            }
        }
    }

    async fn execute_profile(
        &self,
        profile: RootProfile,
        request: InstanceRequest,
        context: Option<TaskContext>,
    ) -> GuiResult<Value> {
        use InstanceRequest::*;
        match request {
            Show { id } => {
                ensure_profile_id(profile.id(), &id)?;
                Ok(json!({"ok":true,"instance":profile.metadata()}))
            }
            History { id } => {
                ensure_profile_id(profile.id(), &id)?;
                Ok(json!({"ok":true,"history":profile.history().map_err(profile_error)?}))
            }
            DiskUsage { id } => {
                ensure_profile_id(profile.id(), &id)?;
                Ok(json!({"ok":true,"disk_usage":profile.disk_usage().map_err(profile_error)?}))
            }
            Clean {
                id,
                keep,
                include_previous,
                apply,
            } => {
                ensure_profile_id(profile.id(), &id)?;
                Ok(
                    json!({"ok":true,"cleanup":profile.cleanup(keep, include_previous, apply).map_err(profile_error)?}),
                )
            }
            Delete { id } => {
                ensure_profile_id(profile.id(), &id)?;
                profile.delete().map_err(profile_error)?;
                Ok(json!({"ok":true,"deleted":id}))
            }
            Copy { id, destination } => {
                ensure_profile_id(profile.id(), &id)?;
                let copied = profile
                    .copy_to(&self.root, &destination, self.config.clone())
                    .map_err(profile_error)?;
                Ok(json!({"ok":true,"instance":copied}))
            }
            Restore { id, revision } => {
                ensure_profile_id(profile.id(), &id)?;
                let instance = profile.restore_revision(&revision).map_err(profile_error)?;
                Ok(json!({"ok":true,"instance":instance}))
            }
            Rollback { id } => {
                ensure_profile_id(profile.id(), &id)?;
                let instance = profile.rollback("previous").map_err(profile_error)?;
                Ok(json!({"ok":true,"instance":instance}))
            }
            PackShow { id } => {
                ensure_profile_id(profile.id(), &id)?;
                let workspace = profile
                    .begin_workspace(self.config.clone())
                    .map_err(profile_error)?;
                let service = workspace
                    .modpack_service(self.config.clone())
                    .map_err(profile_error)?;
                let result = service.show(&id).map_err(modpack_error);
                workspace.abort().map_err(profile_error)?;
                Ok(json!({"ok":true,"receipt":result?}))
            }
            PackExport {
                id,
                output,
                include_local_content,
            } => {
                ensure_profile_id(profile.id(), &id)?;
                let workspace = profile
                    .begin_workspace(self.config.clone())
                    .map_err(profile_error)?;
                let service = workspace
                    .modpack_service(self.config.clone())
                    .map_err(profile_error)?;
                let result = service
                    .export(&id, &output, include_local_content)
                    .map_err(modpack_error);
                workspace.abort().map_err(profile_error)?;
                Ok(json!({"ok":true,"archive":output,"manifest":result?}))
            }
            ContentList { id, kind } => {
                ensure_profile_id(profile.id(), &id)?;
                let workspace = profile
                    .begin_workspace(self.config.clone())
                    .map_err(profile_error)?;
                let service = workspace
                    .content_service(self.config.clone())
                    .map_err(profile_error)?;
                let result = service.list(&id, kind).map_err(content_error);
                workspace.abort().map_err(profile_error)?;
                Ok(json!({"ok":true,"id":id,"kind":kind,"content":result?}))
            }
            ContentAdd {
                id,
                kind,
                source,
                name,
            } => {
                ensure_profile_id(profile.id(), &id)?;
                let workspace = profile
                    .begin_workspace(self.config.clone())
                    .map_err(profile_error)?;
                let service = workspace
                    .content_service(self.config.clone())
                    .map_err(profile_error)?;
                match service
                    .add(&id, kind, &source, name.as_deref())
                    .map_err(content_error)
                {
                    Ok(entry) => {
                        workspace.commit().map_err(profile_error)?;
                        Ok(json!({"ok":true,"id":id,"kind":kind,"entry":entry}))
                    }
                    Err(error) => {
                        let _ = workspace.abort();
                        Err(error)
                    }
                }
            }
            ContentRemove { id, kind, name } => {
                ensure_profile_id(profile.id(), &id)?;
                let workspace = profile
                    .begin_workspace(self.config.clone())
                    .map_err(profile_error)?;
                let service = workspace
                    .content_service(self.config.clone())
                    .map_err(profile_error)?;
                match service.remove(&id, kind, &name).map_err(content_error) {
                    Ok(()) => {
                        workspace.commit().map_err(profile_error)?;
                        Ok(json!({"ok":true,"id":id,"kind":kind,"removed":name}))
                    }
                    Err(error) => {
                        let _ = workspace.abort();
                        Err(error)
                    }
                }
            }
            operation @ (ContentEnable { .. } | ContentDisable { .. }) => {
                let enabled = matches!(&operation, ContentEnable { .. });
                let (id, name) = match operation {
                    ContentEnable { id, name } | ContentDisable { id, name } => (id, name),
                    _ => unreachable!(),
                };
                ensure_profile_id(profile.id(), &id)?;
                let workspace = profile
                    .begin_workspace(self.config.clone())
                    .map_err(profile_error)?;
                let service = workspace
                    .content_service(self.config.clone())
                    .map_err(profile_error)?;
                match service
                    .set_enabled(&id, &name, enabled)
                    .map_err(content_error)
                {
                    Ok(entry) => {
                        workspace.commit().map_err(profile_error)?;
                        Ok(json!({"ok":true,"id":id,"entry":entry}))
                    }
                    Err(error) => {
                        let _ = workspace.abort();
                        Err(error)
                    }
                }
            }
            ContentBackup { id, output } => {
                ensure_profile_id(profile.id(), &id)?;
                let workspace = profile
                    .begin_workspace(self.config.clone())
                    .map_err(profile_error)?;
                let service = workspace
                    .content_service(self.config.clone())
                    .map_err(profile_error)?;
                let result = service.backup(&id, &output).map_err(content_error);
                workspace.abort().map_err(profile_error)?;
                result?;
                Ok(json!({"ok":true,"id":id,"backup":output}))
            }
            ContentRestore { id, archive } => {
                ensure_profile_id(profile.id(), &id)?;
                let workspace = profile
                    .begin_workspace(self.config.clone())
                    .map_err(profile_error)?;
                let service = workspace
                    .content_service(self.config.clone())
                    .map_err(profile_error)?;
                match service.restore(&id, &archive).map_err(content_error) {
                    Ok(()) => {
                        workspace.commit().map_err(profile_error)?;
                        Ok(json!({"ok":true,"id":id,"restored":true}))
                    }
                    Err(error) => {
                        let _ = workspace.abort();
                        Err(error)
                    }
                }
            }
            other @ (List | PackInspect { .. }) => {
                Box::pin(self.execute_unprofiled(other, context)).await
            }
            other => {
                let workspace = profile
                    .begin_workspace(self.config.clone())
                    .map_err(profile_error)?;
                let workspace_root = workspace.root().to_path_buf();
                let result = Box::pin(
                    InstanceOperations::new(workspace_root.clone(), self.config.clone())
                        .with_runtime_service(workspace.service().clone())
                        .execute(other.clone(), context),
                )
                .await;
                match result {
                    Ok(mut value) if other.changes_instance() => {
                        let metadata = workspace.commit().map_err(profile_error)?;
                        rewrite_instance_metadata(&mut value, metadata);
                        Ok(value)
                    }
                    Ok(value) => {
                        workspace.abort().map_err(profile_error)?;
                        Ok(value)
                    }
                    Err(error) => {
                        let _ = workspace.abort();
                        Err(error)
                    }
                }
            }
        }
    }

    async fn execute_unprofiled(
        &self,
        request: InstanceRequest,
        context: Option<TaskContext>,
    ) -> GuiResult<Value> {
        Box::pin(
            InstanceOperations {
                profile: None,
                ..self.clone()
            }
            .execute(request, context),
        )
        .await
    }
}

fn ensure_profile_id(profile_id: &str, request_id: &str) -> GuiResult<()> {
    if profile_id == request_id {
        Ok(())
    } else {
        Err(GuiError::new(
            "instance_key_mismatch",
            "Selected library entry does not match the requested instance",
        ))
    }
}

fn profile_error(error: ccl_core::profiles::ProfileError) -> GuiError {
    GuiError::new("profile_operation_failed", error.to_string())
}

fn rewrite_instance_metadata(value: &mut Value, metadata: ccl_core::instances::InstanceMetadata) {
    if value.get("instance").is_some()
        && let Ok(instance) = serde_json::to_value(metadata)
    {
        value["instance"] = instance;
    }
}

fn is_mrpack(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("mrpack"))
}

fn required_context(context: Option<TaskContext>) -> GuiResult<Option<TaskContext>> {
    context.map(Some).ok_or_else(|| {
        GuiError::new(
            "task_context_required",
            "This operation must run as a cancellable GUI task",
        )
    })
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
fn content_error(error: ccl_core::content::ContentError) -> GuiError {
    match error {
        ccl_core::content::ContentError::Instance(error) => instance_error(error),
        other => GuiError::new("content_failed", other.to_string()),
    }
}
fn modpack_error(error: ccl_core::modpacks::ModpackError) -> GuiError {
    let conflict = matches!(
        &error,
        ccl_core::modpacks::ModpackError::Conflict { .. }
            | ccl_core::modpacks::ModpackError::Instance(
                ccl_core::instances::InstanceError::Conflict(_)
            )
    );
    GuiError::new(
        if error.is_cancelled() {
            "cancelled"
        } else if conflict {
            "instance_conflict"
        } else {
            "pack_failed"
        },
        error.to_string(),
    )
}
fn import_error(error: ccl_core::import_formats::ImportFormatError) -> GuiError {
    GuiError::new("pack_inspect_failed", error.to_string())
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "ccl-gui-profile-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn profile(root: &Path) -> RootProfile {
        let version = root.join("versions/ordinary");
        std::fs::create_dir_all(version.join("mods")).unwrap();
        std::fs::write(
            version.join("ordinary.json"),
            r#"{"id":"ordinary","libraries":[]}"#,
        )
        .unwrap();
        RootProfile::open(root, "ordinary").unwrap()
    }

    #[tokio::test]
    async fn profile_show_uses_root_metadata_without_creating_a_workspace() {
        let root = root();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let profile = profile(&root);
        let expected = profile.game_directory().to_path_buf();
        let value = InstanceOperations::new(root.clone(), DownloadConfig::default())
            .with_profile(profile)
            .execute(
                InstanceRequest::Show {
                    id: "ordinary".into(),
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            value["instance"]["game_directory"],
            expected.to_string_lossy().as_ref()
        );
        let workspaces = root.join(".ccl/profile-workspaces");
        assert!(!workspaces.exists() || std::fs::read_dir(workspaces).unwrap().next().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn ordinary_profile_content_mutations_publish_from_a_workspace() {
        let root = root();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let selected_profile = profile(&root);
        let game = selected_profile.game_directory().to_path_buf();
        let source = root.join("source.jar");
        std::fs::write(&source, b"fixture mod").unwrap();
        let value = InstanceOperations::new(root.clone(), DownloadConfig::default())
            .with_profile(selected_profile)
            .execute(
                InstanceRequest::ContentAdd {
                    id: "ordinary".into(),
                    kind: ContentKind::Mod,
                    source,
                    name: Some("fixture.jar".into()),
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(value["entry"]["path"], "fixture.jar");
        assert_eq!(
            std::fs::read(game.join("mods/fixture.jar")).unwrap(),
            b"fixture mod"
        );
        let history = InstanceOperations::new(root.clone(), DownloadConfig::default())
            .with_profile(profile(&root))
            .execute(
                InstanceRequest::History {
                    id: "ordinary".into(),
                },
                None,
            )
            .await
            .unwrap();
        let revision = history["history"][0]["revision"]
            .as_str()
            .unwrap()
            .to_owned();
        InstanceOperations::new(root.clone(), DownloadConfig::default())
            .with_profile(profile(&root))
            .execute(
                InstanceRequest::Restore {
                    id: "ordinary".into(),
                    revision,
                },
                None,
            )
            .await
            .unwrap();
        assert!(!game.join("mods/fixture.jar").exists());
        InstanceOperations::new(root.clone(), DownloadConfig::default())
            .with_profile(profile(&root))
            .execute(
                InstanceRequest::Rollback {
                    id: "ordinary".into(),
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(game.join("mods/fixture.jar")).unwrap(),
            b"fixture mod"
        );
        let workspaces = root.join(".ccl/profile-workspaces");
        assert!(!workspaces.exists() || std::fs::read_dir(workspaces).unwrap().next().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
