//! Typed GUI adapter for the same online content operations exposed by the CLI.
//!
//! CurseForge credentials are read from the config-scoped OS vault, with an explicit
//! `CCL_CURSEFORGE_API_KEY` environment override for managed deployments.

use crate::{GuiError, GuiResult};
use ccl_core::{
    config::LauncherConfig,
    online_content::{
        OnlineContentService, OnlineError, OnlineInstallRequest, PackImportMode,
        PackInstallOptions, Provider, SearchQuery,
    },
    profiles::{ProfileWorkspace, RootProfile},
    task::{TaskContext, TaskEvent},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum OnlineQuery {
    Search {
        provider: Provider,
        query: String,
        minecraft: Option<String>,
        #[serde(default)]
        loader: Vec<String>,
        #[serde(default = "default_kind")]
        kind: String,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Versions {
        provider: Provider,
        project: String,
        minecraft: Option<String>,
        loader: Option<String>,
    },
    Resolve {
        provider: Provider,
        project: String,
        #[serde(default)]
        version: Option<String>,
        minecraft: String,
        loader: Option<String>,
    },
    Updates {
        id: String,
    },
    Analyze {
        id: String,
    },
}

impl OnlineQuery {
    pub fn target_id(&self) -> Option<&str> {
        match self {
            Self::Updates { id } | Self::Analyze { id } => Some(id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum OnlineMutation {
    Install {
        id: String,
        provider: Provider,
        project: String,
        version: Option<String>,
    },
    PackInstall {
        id: String,
        provider: Provider,
        project: String,
        version: Option<String>,
        #[serde(default)]
        include_optional: bool,
        #[serde(default)]
        mode: PackImportMode,
    },
}

impl OnlineMutation {
    pub fn target_id(&self) -> Option<&str> {
        match self {
            Self::Install { id, .. } => Some(id),
            Self::PackInstall { id, mode, .. } => {
                (*mode == PackImportMode::UpdateExisting).then_some(id)
            }
        }
    }
}

fn default_kind() -> String {
    "mod".into()
}
fn default_limit() -> usize {
    20
}

#[derive(Clone)]
pub struct OnlineService {
    root: PathBuf,
    config_path: PathBuf,
    config: LauncherConfig,
    endpoints: Option<(String, String)>,
    profile: Option<RootProfile>,
}

impl OnlineService {
    pub fn new(root: PathBuf, config_path: PathBuf) -> GuiResult<Self> {
        let config = LauncherConfig::read_or_default(&config_path)
            .map_err(|error| GuiError::new("configuration_error", error.to_string()))?;
        Ok(Self {
            root,
            config_path,
            config,
            endpoints: None,
            profile: None,
        })
    }

    pub fn with_profile(mut self, profile: RootProfile) -> Self {
        self.profile = Some(profile);
        self
    }

    /// Override provider endpoints for local fixtures and embedding applications.
    /// Desktop IPC should use `new`, which always uses provider production endpoints.
    pub fn with_endpoints(
        mut self,
        modrinth: impl Into<String>,
        curseforge: impl Into<String>,
    ) -> Self {
        self.endpoints = Some((modrinth.into(), curseforge.into()));
        self
    }

    fn core(&self) -> GuiResult<OnlineContentService> {
        let key = crate::provider_credentials::curseforge_key(
            &self.config_path,
            self.config.downloads.offline,
        )?;
        let service = OnlineContentService::from_download_config(&self.config.downloads, key)
            .map_err(online_error)?;
        Ok(match &self.endpoints {
            Some((modrinth, curseforge)) => service.with_endpoints(modrinth, curseforge),
            None => service,
        })
    }

    fn begin_profile_workspace(&self) -> GuiResult<ProfileWorkspace> {
        let profile = self
            .profile
            .as_ref()
            .ok_or_else(|| GuiError::new("profile_operation_failed", "No selected root profile"))?;
        profile
            .begin_workspace(self.config.downloads.clone())
            .map_err(profile_error)
    }

    async fn merge_pack_into_profile(
        &self,
        service: &OnlineContentService,
        workspace_root: PathBuf,
        profile_id: &str,
        version: &ccl_core::online_content::VersionHit,
        options: PackInstallOptions,
        context: TaskContext,
    ) -> GuiResult<ccl_core::instances::InstanceMetadata> {
        service
            .download_and_merge_pack_into_profile(
                workspace_root,
                &self.config.downloads,
                profile_id,
                version,
                options,
                Some(context),
            )
            .await
            .map_err(online_error)
    }

    pub async fn query(&self, request: OnlineQuery) -> GuiResult<Value> {
        let service = self.core()?;
        let result = match request {
            OnlineQuery::Search {
                provider,
                query,
                minecraft,
                loader,
                kind,
                limit,
            } => {
                let projects = service
                    .search(
                        provider,
                        &SearchQuery {
                            query,
                            game_version: minecraft,
                            loaders: loader,
                            project_type: Some(kind),
                            limit,
                        },
                    )
                    .await
                    .map_err(online_error)?;
                json!({"ok": true, "projects": projects})
            }
            OnlineQuery::Versions {
                provider,
                project,
                minecraft,
                loader,
            } => {
                let versions = service
                    .versions(provider, &project, minecraft.as_deref(), loader.as_deref())
                    .await
                    .map_err(online_error)?;
                json!({"ok": true, "content_versions": versions})
            }
            OnlineQuery::Resolve {
                provider,
                project,
                version,
                minecraft,
                loader,
            } => {
                let versions = service
                    .resolve_project_version(
                        provider,
                        &project,
                        version.as_deref(),
                        &minecraft,
                        loader.as_deref(),
                    )
                    .await
                    .map_err(online_error)?;
                let conflicts = OnlineContentService::analyze_conflicts(&versions);
                json!({"ok": true, "resolved": versions, "conflicts": conflicts})
            }
            OnlineQuery::Updates { id } => {
                let updates = if self.profile.is_some() {
                    let workspace = self.begin_profile_workspace()?;
                    let workspace_root = workspace.root().to_path_buf();
                    let workspace_service = service
                        .clone()
                        .with_runtime_subdirectory(workspace.runtime_subdirectory().to_path_buf());
                    let result = workspace_service
                        .updates_for_instance(workspace_root, &self.config.downloads, &id)
                        .await
                        .map_err(online_error);
                    workspace.abort().map_err(profile_error)?;
                    result?
                } else {
                    service
                        .updates_for_instance(self.root.clone(), &self.config.downloads, &id)
                        .await
                        .map_err(online_error)?
                };
                json!({"ok": true, "updates": updates})
            }
            OnlineQuery::Analyze { id } => {
                let conflicts = if self.profile.is_some() {
                    let workspace = self.begin_profile_workspace()?;
                    let workspace_root = workspace.root().to_path_buf();
                    let workspace_service = service
                        .clone()
                        .with_runtime_subdirectory(workspace.runtime_subdirectory().to_path_buf());
                    let result = workspace_service
                        .analyze_instance(workspace_root, &self.config.downloads, &id)
                        .await
                        .map_err(online_error);
                    workspace.abort().map_err(profile_error)?;
                    result?
                } else {
                    service
                        .analyze_instance(self.root.clone(), &self.config.downloads, &id)
                        .await
                        .map_err(online_error)?
                };
                json!({"ok": true, "conflicts": conflicts})
            }
        };
        Ok(result)
    }

    pub async fn mutate(&self, request: OnlineMutation, context: TaskContext) -> GuiResult<Value> {
        context
            .emit(TaskEvent::Started {
                task: "online_content".into(),
            })
            .map_err(|_| GuiError::new("cancelled", "Online content operation was cancelled."))?;
        let service = self.core()?;
        let result = match request {
            OnlineMutation::Install {
                id,
                provider,
                project,
                version,
            } => {
                let request = OnlineInstallRequest {
                    root: self.root.clone(),
                    instance_id: id,
                    provider,
                    project_id: project,
                    version_id: version,
                };
                let installed = if self.profile.is_some() {
                    let workspace = self.begin_profile_workspace()?;
                    let workspace_service = service
                        .clone()
                        .with_runtime_subdirectory(workspace.runtime_subdirectory().to_path_buf());
                    let request = OnlineInstallRequest {
                        root: workspace.root().to_path_buf(),
                        ..request
                    };
                    match workspace_service
                        .install_project_with_context(
                            &self.config.downloads,
                            request,
                            Some(context.clone()),
                        )
                        .await
                    {
                        Ok(installed) => {
                            workspace.commit().map_err(profile_error)?;
                            installed
                        }
                        Err(error) => {
                            let _ = workspace.abort();
                            return Err(online_error(error));
                        }
                    }
                } else {
                    service
                        .install_project_with_context(
                            &self.config.downloads,
                            request,
                            Some(context.clone()),
                        )
                        .await
                        .map_err(online_error)?
                };
                json!({"ok": true, "installed_content": installed})
            }
            OnlineMutation::PackInstall {
                id,
                provider,
                project,
                version,
                include_optional,
                mode,
            } => {
                let selected = service
                    .versions(provider, &project, None, None)
                    .await
                    .map_err(online_error)?
                    .into_iter()
                    .find(|candidate| version.as_ref().is_none_or(|id| candidate.id == *id))
                    .ok_or_else(|| {
                        GuiError::new(
                            "online_content_error",
                            "Requested pack version is unavailable.",
                        )
                    })?;
                let instance = if let Some(profile) = &self.profile {
                    if mode != PackImportMode::UpdateExisting || profile.id() != id {
                        return Err(GuiError::new(
                            "instance_key_mismatch",
                            "Selected profile does not match the requested pack target",
                        ));
                    }
                    let workspace = self.begin_profile_workspace()?;
                    let workspace_root = workspace.root().to_path_buf();
                    let runtime_directory = workspace.runtime_directory().to_path_buf();
                    let runtime_subdirectory = workspace.runtime_subdirectory().to_path_buf();
                    let pack_receipt = runtime_directory.join(".ccl/modpack.json");
                    let import =
                        if mode == PackImportMode::UpdateExisting && !pack_receipt.is_file() {
                            self.merge_pack_into_profile(
                                &service,
                                workspace_root.clone(),
                                &id,
                                &selected,
                                PackInstallOptions {
                                    include_optional,
                                    mode,
                                },
                                context.clone(),
                            )
                            .await
                        } else {
                            service
                                .clone()
                                .with_runtime_subdirectory(runtime_subdirectory)
                                .download_and_import_pack(
                                    workspace_root,
                                    &self.config.downloads,
                                    &id,
                                    &selected,
                                    PackInstallOptions {
                                        include_optional,
                                        mode,
                                    },
                                    Some(context.clone()),
                                )
                                .await
                                .map_err(online_error)
                        };
                    match import {
                        Ok(instance) => {
                            let committed = workspace.commit().map_err(profile_error)?;
                            ccl_core::instances::InstanceMetadata {
                                game_directory: committed.game_directory,
                                ..instance
                            }
                        }
                        Err(error) => {
                            let _ = workspace.abort();
                            return Err(error);
                        }
                    }
                } else {
                    service
                        .download_and_import_pack(
                            self.root.clone(),
                            &self.config.downloads,
                            &id,
                            &selected,
                            PackInstallOptions {
                                include_optional,
                                mode,
                            },
                            Some(context.clone()),
                        )
                        .await
                        .map_err(online_error)?
                };
                json!({"ok": true, "instance": instance, "pack_imported": true})
            }
        };
        context
            .emit(TaskEvent::Finished {
                task: "online_content".into(),
            })
            .map_err(|_| GuiError::new("cancelled", "Online content operation was cancelled."))?;
        Ok(result)
    }
}

fn online_error(error: OnlineError) -> GuiError {
    let code = match &error {
        OnlineError::Offline => "offline",
        OnlineError::MissingCurseForgeKey => "provider_credentials_missing",
        OnlineError::Invalid(_) => "invalid_online_request",
        OnlineError::Conflicts(_) | OnlineError::Conflict { .. } => "online_content_conflict",
        error if error.is_cancelled() => "cancelled",
        _ => "online_content_error",
    };
    GuiError::new(code, error.to_string())
}

fn profile_error(error: ccl_core::profiles::ProfileError) -> GuiError {
    GuiError::new("profile_operation_failed", error.to_string())
}
