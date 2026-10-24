//! GUI adapter for updating an explicitly selected CCL CLI installation.
//!
//! The selected root must contain the platform's `ccl` executable. This service
//! never targets the GUI process or its executable.

use ccl_core::{
    CancellationToken,
    config::DownloadConfig,
    task::TaskContext,
    update::{BuildIdentity, Channel, Installation, UpdateCheck, UpdateService, UpdateStatus},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{GuiError, GuiResult};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpdateRequest {
    Status {
        installation_root: PathBuf,
    },
    Check {
        installation_root: PathBuf,
        channel: Channel,
    },
    Apply {
        installation_root: PathBuf,
        channel: Channel,
    },
    Rollback {
        installation_root: PathBuf,
    },
    Recover {
        installation_root: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum UpdateResponse {
    Status {
        build: BuildIdentity,
        update_status: Option<UpdateStatus>,
    },
    Checked {
        update: UpdateCheck,
    },
    Started {
        update_status: UpdateStatus,
        helper_pid: u32,
    },
    RecoveryStarted {
        update_status: Option<UpdateStatus>,
        helper_pid: u32,
    },
}

/// Adapter settings should describe the user's normal CCL data, not the GUI binary.
pub struct UpdateGuiService {
    downloads: DownloadConfig,
    config_path: PathBuf,
    game_root: PathBuf,
}

impl UpdateGuiService {
    pub fn new(downloads: DownloadConfig, config_path: PathBuf, game_root: PathBuf) -> Self {
        Self {
            downloads,
            config_path,
            game_root,
        }
    }

    pub async fn execute(
        &self,
        request: UpdateRequest,
        cancellation: CancellationToken,
    ) -> GuiResult<UpdateResponse> {
        self.execute_inner(request, cancellation, None).await
    }

    /// Execute with the GUI task context so update downloads report progress and
    /// share task cancellation with the rest of the desktop adapter.
    pub async fn execute_with_context(
        &self,
        request: UpdateRequest,
        context: TaskContext,
    ) -> GuiResult<UpdateResponse> {
        self.execute_inner(request, context.cancellation.clone(), Some(context))
            .await
    }

    async fn execute_inner(
        &self,
        request: UpdateRequest,
        cancellation: CancellationToken,
        context: Option<TaskContext>,
    ) -> GuiResult<UpdateResponse> {
        let root = match &request {
            UpdateRequest::Status { installation_root }
            | UpdateRequest::Check {
                installation_root, ..
            }
            | UpdateRequest::Apply {
                installation_root, ..
            }
            | UpdateRequest::Rollback { installation_root }
            | UpdateRequest::Recover { installation_root } => installation_root,
        };
        let install = if matches!(&request, UpdateRequest::Recover { .. }) {
            open_cli_installation_for_recovery(root)?
        } else {
            open_cli_installation(root)?
        };

        match request {
            UpdateRequest::Status { .. } => Ok(UpdateResponse::Status {
                build: install.identity().map_err(update_error)?,
                update_status: install.status().map_err(update_error)?,
            }),
            UpdateRequest::Check { channel, .. } => {
                install.set_channel(channel).map_err(update_error)?;
                let update_service = make_update_service(&self.downloads, context.clone())?;
                let update = update_service
                    .check(
                        channel,
                        install.identity().map_err(update_error)?,
                        &cancellation,
                    )
                    .await
                    .map_err(update_error)?;
                Ok(UpdateResponse::Checked { update })
            }
            UpdateRequest::Apply { channel, .. } => {
                install.set_channel(channel).map_err(update_error)?;
                let update_service = make_update_service(&self.downloads, context.clone())?;
                let identity = install.identity().map_err(update_error)?;
                let check = update_service
                    .check(channel, identity, &cancellation)
                    .await
                    .map_err(update_error)?;
                if !check.available {
                    return Err(GuiError::new(
                        "update_unavailable",
                        "No published update is available for the selected CLI installation",
                    ));
                }
                let candidate = check.candidate.as_ref().ok_or_else(|| {
                    GuiError::new("update_unavailable", "No published update is available")
                })?;
                let status = update_service
                    .stage(
                        candidate,
                        &install,
                        &self.config_path,
                        &self.game_root,
                        &cancellation,
                    )
                    .await
                    .map_err(update_error)?;
                let helper_pid = install.launch_helper().map_err(update_error)?;
                Ok(UpdateResponse::Started {
                    update_status: status,
                    helper_pid,
                })
            }
            UpdateRequest::Rollback { .. } => {
                let status = install.prepare_rollback().map_err(update_error)?;
                let helper_pid = install.launch_helper().map_err(update_error)?;
                Ok(UpdateResponse::Started {
                    update_status: status,
                    helper_pid,
                })
            }
            UpdateRequest::Recover { .. } => {
                let update_status = install.status().map_err(update_error)?;
                let helper_pid = install.launch_helper().map_err(update_error)?;
                Ok(UpdateResponse::RecoveryStarted {
                    update_status,
                    helper_pid,
                })
            }
        }
    }
}

fn open_cli_installation(root: &Path) -> GuiResult<Installation> {
    let install = installation_at_root(root, false)?;
    // Running the executable's private identity command rejects arbitrary files that
    // happen to use the expected filename before any update operation is exposed.
    install.identity().map_err(update_error)?;
    Ok(install)
}

fn open_cli_installation_for_recovery(root: &Path) -> GuiResult<Installation> {
    let install = installation_at_root(root, true)?;
    let executable = root.join(if cfg!(windows) { "ccl.exe" } else { "ccl" });
    match fs::symlink_metadata(&executable) {
        Ok(_) => {
            // Validate a surviving target. If it is missing, core recovery validates
            // the journal binding and retained helper image instead.
            install.identity().map_err(update_error)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(update_error(error)),
    }
    Ok(install)
}

fn installation_at_root(root: &Path, recovery: bool) -> GuiResult<Installation> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|_| GuiError::new("update_installation", "Selected CLI root is unavailable"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(GuiError::new(
            "update_installation",
            "Select the folder containing an existing CCL CLI installation",
        ));
    }
    let executable = root.join(if cfg!(windows) { "ccl.exe" } else { "ccl" });
    if recovery {
        Installation::for_recovery(&executable).map_err(update_error)
    } else {
        Installation::open(&executable).map_err(update_error)
    }
}

fn make_update_service(
    downloads: &DownloadConfig,
    context: Option<TaskContext>,
) -> GuiResult<UpdateService> {
    let service = UpdateService::new(downloads).map_err(update_error)?;
    Ok(match context {
        Some(context) => service.with_task_context(context),
        None => service,
    })
}

fn update_error(error: impl std::fmt::Display) -> GuiError {
    GuiError::new("update_error", error.to_string())
}
