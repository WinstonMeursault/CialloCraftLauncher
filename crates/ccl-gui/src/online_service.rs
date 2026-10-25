//! Typed GUI adapter for the same online content operations exposed by the CLI.
//!
//! CurseForge credentials are read from the config-scoped OS vault, with an explicit
//! `CCL_CURSEFORGE_API_KEY` environment override for managed deployments.

use crate::{GuiError, GuiResult};
use ccl_core::{
    accounts::OsProviderKeyStore,
    config::LauncherConfig,
    online_content::{
        OnlineContentService, OnlineError, OnlineInstallRequest, Provider, SearchQuery,
    },
    task::{TaskContext, TaskEvent},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

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
    },
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
        })
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
        let key = if self.config.downloads.offline {
            None
        } else if let Ok(value) = std::env::var("CCL_CURSEFORGE_API_KEY") {
            (!value.is_empty()).then_some(value)
        } else {
            provider_key(&self.config_path)?
        };
        let service = OnlineContentService::from_download_config(&self.config.downloads, key)
            .map_err(online_error)?;
        Ok(match &self.endpoints {
            Some((modrinth, curseforge)) => service.with_endpoints(modrinth, curseforge),
            None => service,
        })
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
                let updates = service
                    .updates_for_instance(self.root.clone(), &self.config.downloads, &id)
                    .await
                    .map_err(online_error)?;
                json!({"ok": true, "updates": updates})
            }
            OnlineQuery::Analyze { id } => {
                let conflicts = service
                    .analyze_instance(self.root.clone(), &self.config.downloads, &id)
                    .await
                    .map_err(online_error)?;
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
                let installed = service
                    .install_project_with_context(
                        &self.config.downloads,
                        OnlineInstallRequest {
                            root: self.root.clone(),
                            instance_id: id,
                            provider,
                            project_id: project,
                            version_id: version,
                        },
                        Some(context.clone()),
                    )
                    .await
                    .map_err(online_error)?;
                json!({"ok": true, "installed_content": installed})
            }
            OnlineMutation::PackInstall {
                id,
                provider,
                project,
                version,
                include_optional,
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
                let instance = service
                    .download_and_import_pack(
                        self.root.clone(),
                        &self.config.downloads,
                        &id,
                        &selected,
                        include_optional,
                        Some(context.clone()),
                    )
                    .await
                    .map_err(online_error)?;
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

fn provider_key(config_path: &Path) -> GuiResult<Option<String>> {
    let store = OsProviderKeyStore::for_config_file(config_path).map_err(account_error)?;
    match store.get_curseforge() {
        // An unavailable vault is handled like the existing GUI import adapter: Modrinth remains usable.
        Err(ccl_core::accounts::AccountError::SecureStorage(_)) => Ok(None),
        result => result.map_err(account_error),
    }
}

fn account_error(error: ccl_core::accounts::AccountError) -> GuiError {
    GuiError::new("account_error", error.to_string())
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
