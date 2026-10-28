//! Settings and diagnostics without a dependency on the desktop toolkit.
use crate::{GuiError, GuiResult};
use ccl_core::{
    accounts::OsProviderKeyStore,
    config::{ConfigStore, LauncherConfig},
    diagnostics::{DiagnosticsService, DoctorOptions, ExportOptions},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Maintenance {
    pub root: PathBuf,
    pub config: PathBuf,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum MaintenanceRequest {
    Settings,
    Set {
        key: String,
        value: Value,
        expected: Value,
    },
    Reset {
        key: Option<String>,
        expected: Value,
    },
    ProviderStatus,
    ProviderSet {
        key: String,
    },
    ProviderRemove,
    Doctor {
        network_probe: bool,
        write_probe: bool,
    },
    Diagnose {
        key: Option<String>,
        run: Option<String>,
    },
    ExportDiagnostics {
        key: Option<String>,
        run: Option<String>,
        output: PathBuf,
        include_log_preview: bool,
    },
    About,
}
impl Maintenance {
    pub fn execute(&self, request: MaintenanceRequest) -> GuiResult<Value> {
        let store = ConfigStore::new(&self.config);
        let error = |e: ccl_core::Error| GuiError::new("configuration_error", e.to_string());
        match request {
            MaintenanceRequest::Settings => Ok(
                json!({"settings":LauncherConfig::read_or_default(&self.config).map_err(error)?.display_json().map_err(error)?,"path":self.config}),
            ),
            MaintenanceRequest::Set {
                key,
                value,
                expected,
            } => {
                // Compare inside the core cross-process lock; another CLI/GUI edit must not be overwritten.
                store
                    .update(|config| {
                        if config.display_json()? != expected {
                            return Err(ccl_core::Error::InvalidConfig(
                                "Settings changed in another process; reload before saving".into(),
                            ));
                        }
                        let mut document =
                            serde_json::to_value(&*config).expect("configuration serializes");
                        let mut parts = key.split('.');
                        let group = parts.next().unwrap_or_default();
                        let field = parts.next().unwrap_or_default();
                        if parts.next().is_some()
                            || document.get(group).and_then(|v| v.get(field)).is_none()
                        {
                            return Err(ccl_core::Error::InvalidConfig("Unknown setting".into()));
                        }
                        document[group][field] = value;
                        *config = serde_json::from_value(document).map_err(|_| {
                            ccl_core::Error::InvalidConfig("Invalid setting value".into())
                        })?;
                        Ok(())
                    })
                    .map_err(error)?;
                self.execute(MaintenanceRequest::Settings)
            }
            MaintenanceRequest::Reset { key, expected } => {
                if key.is_none() && LauncherConfig::read_or_default(&self.config).is_err() {
                    // Whole reset is the explicit recovery path for malformed configuration.
                    store.reset(None).map_err(error)?;
                } else {
                    store
                        .update(|config| {
                            if config.display_json()? != expected {
                                return Err(ccl_core::Error::InvalidConfig(
                                    "Settings changed; reload before resetting".into(),
                                ));
                            }
                            if let Some(key) = key {
                                let defaults = serde_json::to_value(LauncherConfig::default())
                                    .expect("configuration serializes");
                                let mut document = serde_json::to_value(&*config)
                                    .expect("configuration serializes");
                                let parts: Vec<_> = key.split('.').collect();
                                if parts.len() != 2
                                    || defaults
                                        .get(parts[0])
                                        .and_then(|v| v.get(parts[1]))
                                        .is_none()
                                {
                                    return Err(ccl_core::Error::InvalidConfig(
                                        "Unknown setting".into(),
                                    ));
                                }
                                document[parts[0]][parts[1]] = defaults[parts[0]][parts[1]].clone();
                                *config = serde_json::from_value(document)
                                    .expect("default setting matches schema");
                            } else {
                                *config = LauncherConfig::default();
                            }
                            Ok(())
                        })
                        .map_err(error)?;
                }
                self.execute(MaintenanceRequest::Settings)
            }
            MaintenanceRequest::ProviderStatus => {
                let vault =
                    OsProviderKeyStore::for_config_file(&self.config).map_err(vault_error)?;
                Ok(
                    json!({"saved":vault.get_curseforge().map_err(vault_error)?.is_some(),"environment_override":std::env::var_os("CCL_CURSEFORGE_API_KEY").is_some()}),
                )
            }
            MaintenanceRequest::ProviderSet { key } => {
                OsProviderKeyStore::for_config_file(&self.config)
                    .map_err(vault_error)?
                    .set_curseforge(&key)
                    .map_err(vault_error)?;
                Ok(json!({"saved":true}))
            }
            MaintenanceRequest::ProviderRemove => {
                OsProviderKeyStore::for_config_file(&self.config)
                    .map_err(vault_error)?
                    .remove_curseforge()
                    .map_err(vault_error)?;
                Ok(json!({"saved":false}))
            }
            MaintenanceRequest::Doctor {
                network_probe,
                write_probe,
            } => {
                let config = LauncherConfig::read_or_default(&self.config).map_err(error)?;
                if network_probe && config.downloads.offline {
                    return Err(GuiError::new(
                        "offline",
                        "Network probe is unavailable in offline mode",
                    ));
                }
                let service = DiagnosticsService::new(&self.root, self.root.join(".ccl"));
                Ok(json!(service.doctor(&DoctorOptions {
                    java: config.launch.java,
                    http_proxy: config.downloads.http_proxy,
                    network_url: network_probe.then(|| {
                        "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json".into()
                    }),
                    write_probe,
                    ..Default::default()
                })))
            }
            MaintenanceRequest::Diagnose { key, run } => {
                let service = self.diagnostic_service(key.as_deref())?;
                match run {
                    Some(run) => Ok(json!(service.diagnose_run(&run).map_err(diagnostic_error)?)),
                    None => Ok(json!(service.doctor(&Default::default()))),
                }
            }
            MaintenanceRequest::ExportDiagnostics {
                key,
                run,
                output,
                include_log_preview,
            } => {
                let report = self
                    .diagnostic_service(key.as_deref())?
                    .export(&ExportOptions {
                        run_id: run,
                        include_raw_log_preview: include_log_preview,
                    })
                    .map_err(diagnostic_error)?;
                write_private_new(
                    &output,
                    &serde_json::to_vec_pretty(&report)
                        .map_err(|_| GuiError::new("export_error", "Cannot serialize report"))?,
                )?;
                Ok(json!({"path":output,"notice":report.raw_log_preview_notice}))
            }
            MaintenanceRequest::About => Ok(
                json!({"gui_version":env!("CARGO_PKG_VERSION"),"core_version":ccl_core::update::BuildIdentity::current().version,"os":std::env::consts::OS,"architecture":std::env::consts::ARCH,"repository":"https://github.com/WinstonMeursault/CialloCraftLauncher","os_signed":false,"update_manifest_signatures_required":true}),
            ),
        }
    }
    fn diagnostic_service(&self, key: Option<&str>) -> GuiResult<DiagnosticsService> {
        let Some(key) = key else {
            return Ok(DiagnosticsService::new(&self.root, self.root.join(".ccl")));
        };
        let config = LauncherConfig::read_or_default(&self.config)
            .map_err(|e| GuiError::new("configuration_error", e.to_string()))?;
        crate::discover_entries(&self.root, &config.downloads)?
            .into_iter()
            .find(|entry| entry.key == key)
            .map(|entry| {
                DiagnosticsService::new(&entry.game_directory, entry.resource_root.join(".ccl"))
                    .with_log_root(entry.resource_root)
                    .with_profile(entry.id, entry.launch_version)
            })
            .ok_or_else(|| GuiError::new("invalid_key", "Select an available instance"))
    }
}
fn vault_error(error: ccl_core::accounts::AccountError) -> GuiError {
    GuiError::new("provider_storage", error.to_string())
}
fn diagnostic_error(error: ccl_core::diagnostics::DiagnosticError) -> GuiError {
    GuiError::new("diagnostic_error", error.to_string())
}
fn write_private_new(path: &Path, bytes: &[u8]) -> GuiResult<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| GuiError::new("export_error", e.to_string()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| GuiError::new("export_error", e.to_string()))
}
