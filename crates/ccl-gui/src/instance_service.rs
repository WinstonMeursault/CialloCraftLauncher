//! Typed GUI adapter for managed instances and their local content.
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
    Rename {
        id: String,
        destination: String,
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
}

/// Core-backed instance/content adapter independent of the desktop toolkit.
#[derive(Clone)]
pub struct InstanceOperations {
    root: PathBuf,
    config: DownloadConfig,
}

impl InstanceOperations {
    pub fn new(root: PathBuf, config: DownloadConfig) -> Self {
        Self { root, config }
    }

    fn instances(&self) -> GuiResult<InstanceService> {
        InstanceService::new(self.root.clone(), self.config.clone()).map_err(instance_error)
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
        match request {
            InstanceRequest::List => Ok(
                json!({"ok":true,"managed_instances":self.instances()?.list().map_err(instance_error)?}),
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
            InstanceRequest::Rename { id, destination } => Ok(
                json!({"ok":true,"instance":self.instances()?.rename(&id, &destination).map_err(instance_error)?}),
            ),
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
