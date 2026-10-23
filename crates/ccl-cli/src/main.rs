mod browser;

use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use ccl_core::{
    CancellationToken, Error, Result, TaskContext, TaskEvent,
    accounts::{AccountStore, OsSecretStore},
    auth::{AuthConfig, MicrosoftAuth},
    config::{ConfigStore, LauncherConfig, default_config_path},
    content::{ContentKind, ContentService},
    directory::DirectoryInputs,
    import_formats::{ImportFormatService, ImportOptionsExt},
    install::{InstallError, InstallOptions, InstallService},
    instances::InstanceService,
    launch::{self, LaunchError, LaunchRequest, LaunchService},
    loaders::{LoaderKind, LoaderRequest, LoaderService},
    modpacks::{ImportOptions, ModpackError, ModpackService},
    network::HttpClient,
    online_content::{OnlineContentService, Provider, SearchQuery},
};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(
    name = "ccl",
    version,
    about = "CialloCraftLauncher command-line client"
)]
struct Cli {
    /// Emit one JSON result on stdout.
    #[arg(long, global = true)]
    json: bool,
    /// Disable HTTP requests and require verified local data (separate from launch --offline identity).
    #[arg(long, global = true)]
    network_offline: bool,
    /// Override the game directory selected by the normal search order.
    #[arg(long, global = true)]
    game_dir: Option<PathBuf>,
    /// Read a JSON configuration file from this path.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect the platform, configuration, and selected game directory without changing files.
    Doctor {
        /// Probe the official version metadata endpoint (otherwise no network probe).
        #[arg(long)]
        network_probe: bool,
        /// Test a temporary write in launcher-owned data.
        #[arg(long)]
        write_probe: bool,
    },
    /// Diagnose a saved run or export an allowlisted local diagnostic report.
    Diagnose {
        /// Select a managed instance instead of the shared game root.
        #[arg(long)]
        instance: Option<String>,
        #[arg(long)]
        run: Option<String>,
        /// Write the structured report to a new local file.
        #[arg(long)]
        export: Option<PathBuf>,
        /// Include an opt-in, redacted raw log preview; review before sharing.
        #[arg(long, requires = "export")]
        include_log_preview: bool,
    },
    /// Check GitHub branches and update this CCL installation.
    Update {
        #[command(subcommand)]
        command: UpdateCommand,
    },
    /// Create the selected game directory if it is missing.
    Init,
    /// View or persist launcher settings.
    Settings {
        #[command(subcommand)]
        command: SettingsCommand,
    },
    /// Sign in and manage Microsoft accounts.
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    /// Show available vanilla versions, newest first.
    Versions {
        /// Maximum entries to print (0 shows all).
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Download and verify a vanilla version and its resources.
    Install {
        /// Version ID from the official version list.
        version: String,
    },
    /// List versions already installed in the selected game directory.
    Instances,
    /// List compatible mod loader releases.
    Loaders {
        #[arg(value_parser = parse_loader)]
        loader: LoaderKind,
        minecraft: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Manage isolated modded instances and loader revisions.
    Instance {
        #[command(subcommand)]
        command: InstanceCommand,
    },
    /// Inspect, import and export Modrinth modpacks.
    Pack {
        #[command(subcommand)]
        command: PackCommand,
    },
    /// Manage content and portable userdata backups for isolated instances.
    Content {
        #[command(subcommand)]
        command: ContentCommand,
    },
    /// Search, resolve, install and inspect provider content.
    Online {
        #[command(subcommand)]
        command: OnlineCommand,
    },
    /// Launch an installed version with the selected Microsoft account or offline identity.
    Launch {
        #[arg(required_unless_present = "instance", conflicts_with = "instance")]
        version: Option<String>,
        /// Launch an isolated instance using its selected loader revision.
        #[arg(long)]
        instance: Option<String>,
        /// Use an offline profile with this name.
        #[arg(long, conflicts_with = "account")]
        username: Option<String>,
        /// Use an offline profile instead of the selected Microsoft account.
        #[arg(long, conflicts_with = "account")]
        offline: bool,
        /// Microsoft account profile ID; defaults to the selected account.
        #[arg(long)]
        account: Option<String>,
        /// Do not download a missing Java runtime.
        #[arg(long)]
        no_java_download: bool,
        /// Java executable or Java home; otherwise detect a matching installed runtime.
        #[arg(long)]
        java: Option<PathBuf>,
        /// Maximum Java heap in MiB.
        #[arg(long)]
        memory: Option<u32>,
        /// Prepare and print the argument vector without starting the game.
        #[arg(long)]
        dry_run: bool,
    },
    /// Read the latest launch log, or select a run ID.
    Logs {
        /// Read logs from an isolated managed instance.
        #[arg(long)]
        instance: Option<String>,
        #[arg(long)]
        run: Option<String>,
        #[arg(long, default_value_t = 100)]
        lines: usize,
    },
}

#[derive(Subcommand)]
enum UpdateCommand {
    Check {
        #[arg(long, value_parser = ["stable", "dev"])]
        channel: Option<String>,
    },
    Apply {
        #[arg(long, value_parser = ["stable", "dev"])]
        channel: Option<String>,
    },
    Status,
    Rollback,
    /// Resume a transaction after an interrupted update helper.
    Recover {
        /// Original executable path when running recovery from a retained helper.
        #[arg(long)]
        installation: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum PackCommand {
    /// Read and validate a Modrinth archive without installing it.
    Inspect { archive: PathBuf },
    /// Show the receipt for an imported pack instance.
    Show { id: String },
    /// Import a Modrinth archive into a managed instance.
    Import {
        id: String,
        archive: PathBuf,
        #[arg(long)]
        include_optional: bool,
        #[arg(long)]
        replace_modified: bool,
        /// Permit downloads from this additional exact HTTPS hostname (repeatable).
        #[arg(long = "allow-host")]
        allow_hosts: Vec<String>,
    },
    /// Export a managed pack. Local modified content requires explicit redistribution permission.
    Export {
        id: String,
        output: PathBuf,
        #[arg(long)]
        include_local_content: bool,
    },
}

#[derive(Subcommand)]
enum OnlineCommand {
    /// Resolve and install a mod plus required dependencies into an existing instance.
    Install {
        id: String,
        #[arg(value_parser=parse_provider)]
        provider: Provider,
        project: String,
        #[arg(long)]
        version: Option<String>,
    },
    /// Check versions recorded in the selected instance receipt.
    Updates { id: String },
    /// Analyze dependency and file conflicts in recorded online content.
    Analyze { id: String },
    /// Download a selected provider pack and create/update a managed instance.
    PackInstall {
        id: String,
        #[arg(value_parser=parse_provider)]
        provider: Provider,
        project: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        include_optional: bool,
    },
    Search {
        /// Search provider. Only Modrinth supports catalog search.
        #[arg(value_name = "PROVIDER", value_parser = ["modrinth"])]
        provider: String,
        query: String,
        #[arg(long)]
        minecraft: Option<String>,
        #[arg(long)]
        loader: Vec<String>,
        #[arg(long, default_value = "mod")]
        kind: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Versions {
        #[arg(value_parser=parse_provider)]
        provider: Provider,
        project: String,
        #[arg(long)]
        minecraft: Option<String>,
        #[arg(long)]
        loader: Option<String>,
    },
    Resolve {
        #[arg(value_parser=parse_provider)]
        provider: Provider,
        project: String,
        minecraft: String,
        loader: String,
    },
}
fn parse_provider(value: &str) -> std::result::Result<Provider, String> {
    match value {
        "modrinth" => Ok(Provider::Modrinth),
        "curseforge" => Ok(Provider::CurseForge),
        _ => Err("expected modrinth or curseforge".into()),
    }
}

#[derive(Subcommand)]
enum ContentCommand {
    /// List instance content in one collection.
    List {
        id: String,
        #[arg(value_parser = parse_content_kind)]
        kind: ContentKind,
    },
    /// Copy a local mod, resource pack, shader pack or save into an instance.
    Add {
        id: String,
        #[arg(value_parser = parse_content_kind)]
        kind: ContentKind,
        source: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    Remove {
        id: String,
        #[arg(value_parser = parse_content_kind)]
        kind: ContentKind,
        name: String,
    },
    Enable {
        id: String,
        name: String,
    },
    Disable {
        id: String,
        name: String,
    },
    Backup {
        id: String,
        output: PathBuf,
    },
    Restore {
        id: String,
        archive: PathBuf,
    },
}

#[derive(Subcommand)]
enum SettingsCommand {
    /// Show all settings with proxy credentials hidden.
    Show,
    /// Read one dotted setting.
    Get { key: String },
    /// Persist a setting; booleans/numbers/null use JSON, other values are strings.
    Set { key: String, value: String },
    /// Reset one setting, or all settings with --all.
    Reset {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        key: Option<String>,
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum InstanceCommand {
    List,
    Show {
        id: String,
    },
    /// Create an isolated instance with a pinned loader release.
    Install {
        id: String,
        minecraft: String,
        #[arg(value_parser = parse_loader)]
        loader: LoaderKind,
        loader_version: String,
    },
    /// Replace an instance loader; retain game data and a rollback revision.
    Update {
        id: String,
        minecraft: String,
        #[arg(value_parser = parse_loader)]
        loader: LoaderKind,
        loader_version: String,
    },
    /// Rename the instance ID and keep all content and revisions.
    Rename {
        id: String,
        destination: String,
    },
    /// Make an independent copy, including saves and retained revisions.
    Copy {
        id: String,
        destination: String,
    },
    /// Delete this instance, including worlds. Requires --yes.
    Delete {
        id: String,
        #[arg(long, required = true)]
        yes: bool,
    },
    /// List current, previous and archived revisions.
    History {
        id: String,
    },
    /// Restore a retained snapshot as a new revision, including its userdata.
    Restore {
        id: String,
        revision: String,
    },
    /// Show logical instance disk usage (hard-linked files may share disk blocks).
    DiskUsage {
        id: String,
    },
    /// Preview cleanup of retained history/stage; --apply performs it.
    Clean {
        id: String,
        #[arg(long, default_value_t = 2)]
        keep: usize,
        #[arg(long)]
        include_previous: bool,
        #[arg(long)]
        apply: bool,
    },
    /// Switch to the previous successful loader revision.
    Rollback {
        id: String,
    },
    /// Return to vanilla while preserving mods, configuration and saves.
    UninstallLoader {
        id: String,
    },
}

fn parse_loader(value: &str) -> std::result::Result<LoaderKind, String> {
    match value {
        "fabric" => Ok(LoaderKind::Fabric),
        "forge" => Ok(LoaderKind::Forge),
        "neoforge" => Ok(LoaderKind::NeoForge),
        "quilt" => Ok(LoaderKind::Quilt),
        "legacyfabric" | "legacy-fabric" => Ok(LoaderKind::LegacyFabric),
        "optifine" => Ok(LoaderKind::OptiFine),
        _ => Err("expected fabric, forge, neoforge, quilt, legacyfabric or optifine".into()),
    }
}

fn parse_content_kind(value: &str) -> std::result::Result<ContentKind, String> {
    match value {
        "mods" => Ok(ContentKind::Mod),
        "resourcepacks" => Ok(ContentKind::ResourcePack),
        "shaderpacks" => Ok(ContentKind::ShaderPack),
        "saves" => Ok(ContentKind::Save),
        _ => Err("expected mods, resourcepacks, shaderpacks or saves".into()),
    }
}

#[derive(Subcommand)]
enum AccountCommand {
    /// Begin device-code login. Credentials are saved only in OS secure storage.
    Login {
        #[arg(long)]
        client_id: Option<String>,
        /// Print the verification URL without opening the default browser.
        #[arg(long)]
        no_browser: bool,
    },
    /// Check that the system credential vault can persist credentials.
    CheckStorage,
    List,
    Select {
        id: String,
    },
    Refresh {
        id: Option<String>,
    },
    /// Remove local credentials; this does not revoke Microsoft consent.
    Logout {
        id: Option<String>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    if let Some(code) = internal_update_command() {
        return ExitCode::from(code);
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    match run(&cli).await {
        Ok(value) => {
            if cli.json {
                println!("{value}");
            } else {
                print_human(&value);
            }
            let code = value.get("launch").map_or(0, |result| {
                if result["cancelled"].as_bool() == Some(true) {
                    130
                } else if result["exit_code"].as_i64() == Some(0) {
                    0
                } else {
                    8
                }
            });
            ExitCode::from(code)
        }
        Err(error) => {
            if cli.json {
                println!(
                    "{}",
                    json!({ "ok": false, "error": { "code": error_code(&error), "message": error.to_string() } })
                );
            } else {
                eprintln!("error: {error}");
            }
            ExitCode::from(exit_code(&error))
        }
    }
}

async fn run(cli: &Cli) -> Result<Value> {
    let inputs = DirectoryInputs::from_environment(cli.game_dir.clone())?;
    let resolution = inputs.resolve()?;
    let config_path = cli.config.clone().unwrap_or_else(|| {
        default_config_path(
            &inputs.home_directory,
            env::var_os("APPDATA").as_deref().map(Path::new),
            env::var_os("XDG_CONFIG_HOME").as_deref().map(Path::new),
        )
    });
    if let Command::Update { command } = &cli.command
        && matches!(
            command,
            UpdateCommand::Status | UpdateCommand::Rollback | UpdateCommand::Recover { .. }
        )
    {
        return run_update(
            command,
            &LauncherConfig::default(),
            &config_path,
            &resolution.path,
        )
        .await;
    }
    let is_settings_write = matches!(
        &cli.command,
        Command::Settings {
            command: SettingsCommand::Set { .. } | SettingsCommand::Reset { .. }
        }
    );
    let reset_all = matches!(
        &cli.command,
        Command::Settings {
            command: SettingsCommand::Reset { all: true, .. }
        }
    );
    let mut config = if reset_all {
        LauncherConfig::default()
    } else if cli.config.is_some() && !is_settings_write {
        LauncherConfig::read(&config_path)?
    } else {
        LauncherConfig::read_or_default(&config_path)?
    };
    config.downloads.offline |= cli.network_offline;
    let _transport = HttpClient::new(&config.downloads)?;

    match &cli.command {
        Command::Settings { command } => {
            let store = ConfigStore::new(&config_path);
            match command {
                SettingsCommand::Show => Ok(
                    json!({ "ok": true, "settings": config.display_json()?, "path": config_path }),
                ),
                SettingsCommand::Get { key } => {
                    Ok(json!({ "ok": true, "key": key, "value": store.get(key)? }))
                }
                SettingsCommand::Set { key, value } => {
                    let value = serde_json::from_str(value)
                        .unwrap_or_else(|_| Value::String(value.clone()));
                    store.set(key, value)?;
                    Ok(json!({ "ok": true, "key": key, "value": store.get(key)? }))
                }
                SettingsCommand::Reset { key, .. } => {
                    store.reset(key.as_deref())?;
                    Ok(
                        json!({ "ok": true, "settings": LauncherConfig::read(&config_path)?.display_json()? }),
                    )
                }
            }
        }
        Command::Account { command } => {
            let store = account_store(&config_path)?;
            match command {
                AccountCommand::CheckStorage => {
                    OsSecretStore::for_account_file(&config_path.with_file_name("accounts.json"))?
                        .probe()?;
                    Ok(json!({"ok": true, "secure_storage": "available"}))
                }
                AccountCommand::List => Ok(
                    json!({ "ok": true, "accounts": store.list()?, "selected": store.selected()? }),
                ),
                AccountCommand::Select { id } => {
                    store.select(id)?;
                    Ok(json!({ "ok": true, "selected": id }))
                }
                AccountCommand::Logout { id } => {
                    let id = account_id(&store, id.as_deref())?;
                    store.remove_cancellable(&id, &signal_token())?;
                    Ok(json!({ "ok": true, "removed": id }))
                }
                AccountCommand::Login {
                    client_id,
                    no_browser,
                } => {
                    if config.downloads.offline {
                        return Err(Error::InvalidConfig("Microsoft login requires network access; disable --network-offline / downloads.offline".into()));
                    }
                    let client_id = client_id.as_ref().or(config.auth.microsoft_client_id.as_ref()).ok_or_else(|| Error::InvalidConfig("set auth.microsoft_client_id to your registered CCL application ID before login; see docs/development.en.md#microsoft-application".into()))?;
                    OsSecretStore::for_account_file(&config_path.with_file_name("accounts.json"))?
                        .probe()?;
                    let auth = auth_service(client_id, &config)?;
                    let token = signal_token();
                    let device = auth.begin_device_login_cancellable(&token).await?;
                    if cli.json {
                        eprintln!(
                            "{}",
                            json!({ "kind": "device_authorization", "verification_uri": device.verification_uri, "user_code": device.user_code, "expires_in": device.expires_in.as_secs() })
                        );
                    } else {
                        eprintln!(
                            "Open {} and enter {} (expires in {} seconds)",
                            device.verification_uri,
                            device.user_code,
                            device.expires_in.as_secs()
                        );
                    }
                    if !no_browser
                        && let Err(message) =
                            browser::open_verification_page(&device.verification_uri).await
                    {
                        if cli.json {
                            eprintln!(
                                "{}",
                                json!({"kind":"browser_open","opened":false,"message":message})
                            );
                        } else {
                            eprintln!("{message}; open the verification URL above manually");
                        }
                    }
                    let authenticated = auth.complete_device_login(device, &token).await?;
                    let metadata = authenticated.metadata.clone();
                    authenticated.persist_cancellable(&store, &token)?;
                    store.select(&metadata.id)?;
                    Ok(json!({ "ok": true, "account": metadata, "selected": metadata.id }))
                }
                AccountCommand::Refresh { id } => {
                    if config.downloads.offline {
                        return Err(Error::InvalidConfig("Microsoft refresh requires network access; disable --network-offline / downloads.offline".into()));
                    }
                    let id = account_id(&store, id.as_deref())?;
                    let metadata = store
                        .list()?
                        .into_iter()
                        .find(|a| a.id == id)
                        .ok_or(ccl_core::accounts::AccountError::Invalid)?;
                    let auth = auth_service(
                        metadata
                            .oauth_client_id
                            .as_deref()
                            .ok_or(ccl_core::accounts::AccountError::Invalid)?,
                        &config,
                    )?;
                    let metadata = auth.refresh_account(&store, &id, &signal_token()).await?;
                    Ok(json!({ "ok": true, "account": metadata }))
                }
            }
        }
        Command::Doctor {
            network_probe,
            write_probe,
        } => {
            if *network_probe && config.downloads.offline {
                return Err(Error::InvalidConfig(
                    "network probe cannot run in offline mode".into(),
                ));
            }
            let diagnostics = ccl_core::diagnostics::DiagnosticsService::new(
                &resolution.path,
                resolution.path.join(".ccl"),
            );
            let report = diagnostics.doctor(&ccl_core::diagnostics::DoctorOptions {
                java: config.launch.java.clone(),
                http_proxy: config.downloads.http_proxy.clone(),
                network_url: network_probe.then(|| {
                    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json".into()
                }),
                write_probe: *write_probe,
                ..Default::default()
            });
            Ok(
                json!({ "ok": true, "platform": { "os":env::consts::OS,"architecture":env::consts::ARCH }, "game_directory":resolution,
                "config":{ "path":config_path,"exists":config_path.is_file() },
                "downloads":{"version_list_source":config.downloads.version_list_source,"file_source":config.downloads.file_source,"mirror_root":config.downloads.mirror_root,"http_proxy_configured":config.downloads.http_proxy.is_some(),"max_parallel":config.downloads.max_parallel},
                "diagnostics":report }),
            )
        }
        Command::Diagnose {
            instance,
            run,
            export,
            include_log_preview,
        } => {
            let diagnostic_root = if let Some(id) = instance {
                InstanceService::new(resolution.path.clone(), config.downloads.clone())?
                    .game_directory(id)?
            } else {
                resolution.path.clone()
            };
            let service = ccl_core::diagnostics::DiagnosticsService::new(
                &diagnostic_root,
                diagnostic_root.join(".ccl"),
            );
            if let Some(path) = export {
                let report = service.export(&ccl_core::diagnostics::ExportOptions {
                    run_id: run.clone(),
                    include_raw_log_preview: *include_log_preview,
                })?;
                let mut options = std::fs::OpenOptions::new();
                options.create_new(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(path).map_err(|source| Error::Io {
                    path: path.clone(),
                    source,
                })?;
                use std::io::Write;
                file.write_all(serde_json::to_string_pretty(&report).unwrap().as_bytes())
                    .and_then(|_| file.sync_all())
                    .map_err(|source| Error::Io {
                        path: path.clone(),
                        source,
                    })?;
                Ok(
                    json!({"ok":true,"diagnostic_export":path,"notice":report.raw_log_preview_notice}),
                )
            } else if let Some(id) = run {
                Ok(json!({"ok":true,"diagnosis":service.diagnose_run(id)?}))
            } else {
                Ok(json!({"ok":true,"diagnostics":service.doctor(&Default::default())}))
            }
        }
        Command::Update { command } => {
            run_update(command, &config, &config_path, &resolution.path).await
        }
        Command::Init => {
            resolution.ensure_exists()?;
            Ok(
                json!({ "ok": true, "game_directory": { "path": resolution.path, "origin": resolution.origin, "exists": true } }),
            )
        }
        Command::Versions { limit } => {
            let manifest = InstallService::new(config.downloads)?
                .list_versions_for(&resolution.path)
                .await?;
            let versions: Vec<_> = manifest.versions.iter().take(if *limit == 0 { usize::MAX } else { *limit })
                .map(|v| json!({ "id": v.id, "type": v.version_type, "release_time": v.release_time }))
                .collect();
            Ok(json!({ "ok": true, "latest": manifest.latest, "versions": versions }))
        }
        Command::Install { version } => {
            resolution.ensure_exists()?;
            let service = InstallService::new(config.downloads)?;
            let token = CancellationToken::default();
            let signal_token = token.clone();
            tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    signal_token.cancel();
                }
            });
            let completed = Arc::new(AtomicUsize::new(0));
            let counter = completed.clone();
            let context = TaskContext::new(
                token,
                Arc::new(move |event| {
                    if matches!(event, TaskEvent::Finished { .. }) {
                        let count = counter.fetch_add(1, Ordering::Relaxed) + 1;
                        if count.is_multiple_of(100) {
                            eprintln!("Verified {count} files...");
                        }
                    }
                }),
            );
            let summary = service
                .install(
                    version,
                    &InstallOptions::for_current_platform(&resolution.path),
                    Some(context),
                )
                .await?;
            Ok(json!({ "ok": true, "install": summary }))
        }
        Command::Instances => Ok(
            json!({ "ok": true, "game_directory": resolution, "instances": launch::list_installed_versions(&resolution.path)? }),
        ),
        Command::Loaders {
            loader,
            minecraft,
            limit,
        } => {
            let versions = LoaderService::new(config.downloads.clone())?
                .versions(*loader, minecraft)
                .await?;
            let versions: Vec<_> = versions
                .into_iter()
                .take(if *limit == 0 { usize::MAX } else { *limit })
                .collect();
            Ok(
                json!({ "ok": true, "loader": loader, "minecraft": minecraft, "loader_versions": versions }),
            )
        }
        Command::Instance { command } => {
            let service = InstanceService::new(resolution.path, config.downloads.clone())?;
            match command {
                InstanceCommand::List => {
                    Ok(json!({ "ok": true, "managed_instances": service.list()? }))
                }
                InstanceCommand::Show { id } => {
                    Ok(json!({ "ok": true, "instance": service.metadata(id)? }))
                }
                InstanceCommand::Install {
                    id,
                    minecraft,
                    loader,
                    loader_version,
                }
                | InstanceCommand::Update {
                    id,
                    minecraft,
                    loader,
                    loader_version,
                } => {
                    let request = LoaderRequest {
                        kind: *loader,
                        minecraft: minecraft.clone(),
                        version: loader_version.clone(),
                    };
                    let context = operation_context();
                    let instance = service.install(id, &request, Some(context)).await?;
                    Ok(json!({ "ok": true, "instance": instance }))
                }
                InstanceCommand::Rename { id, destination } => {
                    Ok(json!({"ok":true,"instance":service.rename(id,destination)?}))
                }
                InstanceCommand::Copy { id, destination } => {
                    Ok(json!({"ok":true,"instance":service.duplicate(id,destination)?}))
                }
                InstanceCommand::Delete { id, .. } => {
                    service.delete(id)?;
                    Ok(json!({"ok":true,"deleted":id}))
                }
                InstanceCommand::History { id } => {
                    Ok(json!({"ok":true,"history":service.history(id)?}))
                }
                InstanceCommand::Restore { id, revision } => {
                    Ok(json!({"ok":true,"instance":service.restore_revision(id,revision)?}))
                }
                InstanceCommand::DiskUsage { id } => {
                    Ok(json!({"ok":true,"disk_usage":service.disk_usage(id)?}))
                }
                InstanceCommand::Clean {
                    id,
                    keep,
                    include_previous,
                    apply,
                } => Ok(
                    json!({"ok":true,"cleanup":service.cleanup(id,*keep,*include_previous,*apply)?}),
                ),
                InstanceCommand::Rollback { id } => {
                    Ok(json!({ "ok": true, "instance": service.rollback(id).await? }))
                }
                InstanceCommand::UninstallLoader { id } => {
                    let instance = service
                        .uninstall_loader(id, Some(operation_context()))
                        .await?;
                    Ok(json!({ "ok": true, "instance": instance }))
                }
            }
        }
        Command::Pack { command } => match command {
            PackCommand::Inspect { archive } => {
                let manifest = if archive
                    .extension()
                    .and_then(|v| v.to_str())
                    .is_some_and(|v| v.eq_ignore_ascii_case("mrpack"))
                {
                    json!(ModpackService::inspect(archive)?)
                } else {
                    ImportFormatService::inspect(archive)?
                };
                Ok(json!({ "ok": true, "manifest": manifest }))
            }
            PackCommand::Show { id } => {
                let service =
                    ModpackService::new(resolution.path.clone(), config.downloads.clone())?;
                let receipt = service.show(id)?;
                Ok(json!({ "ok": true, "receipt": receipt }))
            }
            PackCommand::Import {
                id,
                archive,
                include_optional,
                replace_modified,
                allow_hosts,
            } => {
                let service =
                    ModpackService::new(resolution.path.clone(), config.downloads.clone())?;
                let options = ImportOptions {
                    include_optional: *include_optional,
                    replace_modified: *replace_modified,
                    allowed_hosts: allow_hosts.clone(),
                };
                let instance = if archive
                    .extension()
                    .and_then(|v| v.to_str())
                    .is_some_and(|v| v.eq_ignore_ascii_case("mrpack"))
                    || ImportFormatService::is_modrinth(archive)?
                {
                    service
                        .import(id, archive, &options, Some(operation_context()))
                        .await?
                } else {
                    ImportFormatService::new(resolution.path, config.downloads.clone())
                        .import(
                            id,
                            archive,
                            &ImportOptionsExt {
                                include_optional: *include_optional,
                                replace_modified: *replace_modified,
                                curseforge_api_key: env::var("CCL_CURSEFORGE_API_KEY").ok(),
                                allowed_hosts: allow_hosts.clone(),
                            },
                            Some(operation_context()),
                        )
                        .await?
                };
                Ok(json!({ "ok": true, "instance": instance, "pack_imported": true }))
            }
            PackCommand::Export {
                id,
                output,
                include_local_content,
            } => {
                let service =
                    ModpackService::new(resolution.path.clone(), config.downloads.clone())?;
                let manifest = service.export(id, output, *include_local_content)?;
                Ok(json!({ "ok": true, "archive": output, "manifest": manifest }))
            }
        },
        Command::Content { command } => {
            let service = ContentService::new(resolution.path, config.downloads.clone())?;
            match command {
                ContentCommand::List { id, kind } => {
                    let content = service.list(id, *kind)?;
                    Ok(json!({ "ok": true, "id": id, "kind": kind, "content": content }))
                }
                ContentCommand::Add {
                    id,
                    kind,
                    source,
                    name,
                } => {
                    let entry = service.add(id, *kind, source, name.as_deref())?;
                    Ok(json!({ "ok": true, "id": id, "kind": kind, "entry": entry }))
                }
                ContentCommand::Remove { id, kind, name } => {
                    service.remove(id, *kind, name)?;
                    Ok(json!({ "ok": true, "id": id, "kind": kind, "removed": name }))
                }
                ContentCommand::Enable { id, name } => {
                    let entry = service.set_enabled(id, name, true)?;
                    Ok(json!({ "ok": true, "id": id, "entry": entry }))
                }
                ContentCommand::Disable { id, name } => {
                    let entry = service.set_enabled(id, name, false)?;
                    Ok(json!({ "ok": true, "id": id, "entry": entry }))
                }
                ContentCommand::Backup { id, output } => {
                    service.backup(id, output)?;
                    Ok(json!({ "ok": true, "id": id, "backup": output }))
                }
                ContentCommand::Restore { id, archive } => {
                    service.restore(id, archive)?;
                    Ok(json!({ "ok": true, "id": id, "restored": true }))
                }
            }
        }
        Command::Online { command } => {
            let service = OnlineContentService::from_download_config(
                &config.downloads,
                env::var("CCL_CURSEFORGE_API_KEY").ok(),
            )?;
            match command {
                OnlineCommand::Install {
                    id,
                    provider,
                    project,
                    version,
                } => {
                    let installed = service
                        .install_project(
                            resolution.path,
                            &config.downloads,
                            id,
                            *provider,
                            project,
                            version.as_deref(),
                        )
                        .await?;
                    Ok(json!({"ok":true,"installed_content":installed}))
                }
                OnlineCommand::Updates { id } => Ok(
                    json!({"ok":true,"updates":service.updates_for_instance(resolution.path,&config.downloads,id).await?}),
                ),
                OnlineCommand::Analyze { id } => Ok(
                    json!({"ok":true,"conflicts":service.analyze_instance(resolution.path,&config.downloads,id).await?}),
                ),
                OnlineCommand::PackInstall {
                    id,
                    provider,
                    project,
                    version,
                    include_optional,
                } => {
                    let selected = service
                        .versions(*provider, project, None, None)
                        .await?
                        .into_iter()
                        .find(|v| version.as_ref().is_none_or(|id| v.id == *id))
                        .ok_or_else(|| {
                            Error::InvalidConfig("requested pack version is unavailable".into())
                        })?;
                    let instance = service
                        .download_and_import_pack(
                            resolution.path,
                            &config.downloads,
                            id,
                            &selected,
                            *include_optional,
                            Some(operation_context()),
                        )
                        .await?;
                    Ok(json!({"ok":true,"instance":instance,"pack_imported":true}))
                }
                OnlineCommand::Search {
                    provider: _,
                    query,
                    minecraft,
                    loader,
                    kind,
                    limit,
                } => {
                    let projects = service
                        .search(&SearchQuery {
                            query: query.clone(),
                            game_version: minecraft.clone(),
                            loaders: loader.clone(),
                            project_type: Some(kind.clone()),
                            limit: *limit,
                        })
                        .await?;
                    Ok(json!({"ok":true,"projects":projects}))
                }
                OnlineCommand::Versions {
                    provider,
                    project,
                    minecraft,
                    loader,
                } => Ok(
                    json!({"ok":true,"content_versions":service.versions(*provider,project,minecraft.as_deref(),loader.as_deref()).await?}),
                ),
                OnlineCommand::Resolve {
                    provider,
                    project,
                    minecraft,
                    loader,
                } => {
                    let versions = match provider {
                        Provider::Modrinth => {
                            service
                                .resolve_dependencies(project, minecraft, loader)
                                .await?
                        }
                        Provider::CurseForge => {
                            service
                                .resolve_curseforge_dependencies(project, minecraft, Some(loader))
                                .await?
                        }
                    };
                    let conflicts = OnlineContentService::analyze_conflicts(&versions);
                    Ok(json!({"ok":true,"resolved":versions,"conflicts":conflicts}))
                }
            }
        }
        Command::Logs {
            instance,
            run,
            lines,
        } => {
            let root = match instance {
                Some(id) => InstanceService::new(resolution.path, config.downloads.clone())?
                    .game_directory(id)?,
                None => resolution.path,
            };
            let (run_id, text) = launch::read_logs(&root, run.as_deref(), *lines)?;
            Ok(json!({ "ok": true, "run_id": run_id, "log": text }))
        }
        Command::Launch {
            version,
            instance,
            username,
            offline,
            account,
            no_java_download,
            java,
            memory,
            dry_run,
        } => {
            let instance_service = if instance.is_some() {
                Some(InstanceService::new(
                    resolution.path.clone(),
                    config.downloads.clone(),
                )?)
            } else {
                None
            };
            let instance_guard = match (&instance_service, instance) {
                (Some(service), Some(id)) => Some(service.acquire_launch(id)?),
                _ => None,
            };
            let (game_directory, version) = match &instance_guard {
                Some(guard) => (
                    guard.game_directory().to_path_buf(),
                    guard.version().to_owned(),
                ),
                None => (
                    resolution.path.clone(),
                    version.clone().expect("clap requires version or instance"),
                ),
            };
            let service = LaunchService::new(config.downloads.clone());
            let token = signal_token();
            let store = account_store(&config_path)?;
            if config.downloads.offline && account.is_some() {
                return Err(Error::InvalidConfig("network offline mode cannot refresh a Microsoft account; launch with --offline identity, or disable --network-offline".into()));
            }
            let selected = if *offline || username.is_some() || config.downloads.offline {
                None
            } else {
                account.clone().or(store.selected()?)
            };
            let identity = if let Some(id) = selected {
                let metadata = store
                    .list()?
                    .into_iter()
                    .find(|a| a.id == id)
                    .ok_or(ccl_core::accounts::AccountError::Invalid)?;
                Some(
                    auth_service(
                        metadata
                            .oauth_client_id
                            .as_deref()
                            .ok_or(ccl_core::accounts::AccountError::Invalid)?,
                        &config,
                    )?
                    .launch_identity(&store, &id, &token)
                    .await?,
                )
            } else {
                None
            };
            let request = LaunchRequest {
                game_directory,
                version,
                username: username.clone().unwrap_or_else(|| "CCLPlayer".into()),
                java: java.clone().or(config.launch.java.clone()),
                max_memory_mb: memory.unwrap_or(config.launch.max_memory_mb),
                identity,
                auto_install_java: !no_java_download && config.launch.auto_install_java,
            };
            let human = !cli.json;
            let context = TaskContext::new(
                token,
                Arc::new(move |event| match event {
                    TaskEvent::Log { line, .. } if human => eprintln!("{line}"),
                    TaskEvent::ProcessStarted { pid } => eprintln!("Game process: {pid}"),
                    TaskEvent::Started { task } if task.starts_with("install Java runtime ") => {
                        eprintln!("{task}...")
                    }
                    _ => {}
                }),
            );
            if *dry_run {
                let prepared = service.prepare(&request, Some(context)).await?;
                let value = json!({ "ok": true, "prepared": prepared });
                Ok(value)
            } else {
                let result = service.launch(&request, Some(context)).await?;
                Ok(
                    json!({ "ok": result.exit_code == Some(0) && !result.cancelled, "launch": result }),
                )
            }
        }
    }
}

fn account_store(config_path: &Path) -> Result<AccountStore<OsSecretStore>> {
    let path = config_path.with_file_name("accounts.json");
    Ok(AccountStore::new(
        &path,
        Arc::new(OsSecretStore::for_account_file(&path)?),
    ))
}
fn account_id(store: &AccountStore<OsSecretStore>, explicit: Option<&str>) -> Result<String> {
    explicit
        .map(str::to_owned)
        .or(store.selected()?)
        .ok_or_else(|| ccl_core::accounts::AccountError::Invalid.into())
}
fn auth_service(client_id: &str, config: &LauncherConfig) -> Result<MicrosoftAuth> {
    let mut auth = AuthConfig::new(client_id);
    auth.proxy = config.downloads.http_proxy.clone();
    Ok(MicrosoftAuth::new(auth)?)
}
fn signal_token() -> CancellationToken {
    let token = CancellationToken::default();
    let signal = token.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal.cancel();
        }
    });
    token
}

fn operation_context() -> TaskContext {
    TaskContext::new(
        signal_token(),
        Arc::new(|event| match event {
            TaskEvent::Started { task } if !task.starts_with("download ") => eprintln!("{task}..."),
            TaskEvent::Log { line, .. } => eprintln!("{line}"),
            _ => {}
        }),
    )
}

fn print_human(value: &Value) {
    if [
        "update",
        "update_status",
        "diagnosis",
        "diagnostics",
        "diagnostic_export",
        "history",
        "disk_usage",
        "cleanup",
        "deleted",
        "projects",
        "content_versions",
        "installed_content",
        "resolved",
        "updates",
        "conflicts",
    ]
    .iter()
    .any(|key| value.get(key).is_some())
    {
        println!("{}", serde_json::to_string_pretty(value).unwrap());
        return;
    }
    if let Some(manifest) = value.get("manifest") {
        println!("Pack manifest:");
        println!("{}", serde_json::to_string_pretty(manifest).unwrap());
        if let Some(archive) = value.get("archive") {
            println!("Archive: {}", archive.as_str().unwrap_or("?"));
        }
        return;
    }
    if let Some(receipt) = value.get("receipt") {
        println!("Imported modpack receipt:");
        println!("{}", serde_json::to_string_pretty(receipt).unwrap());
        return;
    }
    if let Some(content) = value.get("content").and_then(Value::as_array) {
        let kind = value["kind"].as_str().unwrap_or("content");
        println!("{kind}:");
        for entry in content {
            println!(
                "{}\t{} bytes{}",
                entry["path"].as_str().unwrap_or("?"),
                entry["bytes"].as_u64().unwrap_or(0),
                if entry["enabled"].as_bool() == Some(false) {
                    "\tdisabled"
                } else {
                    ""
                }
            );
        }
        return;
    }
    if let Some(entry) = value.get("entry") {
        println!(
            "Content {} ({} bytes)",
            entry["path"].as_str().unwrap_or("?"),
            entry["bytes"].as_u64().unwrap_or(0)
        );
        return;
    }
    if value.get("kind").is_some()
        && let Some(removed) = value.get("removed").and_then(Value::as_str)
    {
        println!("Removed content {removed}");
        return;
    }
    if let Some(backup) = value.get("backup") {
        println!("Backup written to {}", backup.as_str().unwrap_or("?"));
        return;
    }
    if value.get("restored") == Some(&Value::Bool(true)) {
        println!(
            "Userdata restored for instance {}",
            value["id"].as_str().unwrap_or("?")
        );
        return;
    }
    if value.get("pack_imported") == Some(&Value::Bool(true)) {
        let instance = &value["instance"];
        println!(
            "Modpack installed in instance {}",
            instance["id"].as_str().unwrap_or("?")
        );
        println!("{}", serde_json::to_string_pretty(instance).unwrap());
        return;
    }
    if value.get("managed_instances").is_some()
        || value.get("instance").is_some()
        || value.get("loader_versions").is_some()
    {
        println!("{}", serde_json::to_string_pretty(value).unwrap());
        return;
    }
    if value.get("settings").is_some()
        || value.get("key").is_some()
        || value.get("accounts").is_some()
        || value.get("account").is_some()
        || value.get("selected").is_some()
        || value.get("removed").is_some()
    {
        println!("{}", serde_json::to_string_pretty(value).unwrap());
        return;
    }
    if let Some(log) = value.get("log").and_then(Value::as_str) {
        println!("{log}");
        return;
    }
    if let Some(instances) = value.get("instances").and_then(Value::as_array) {
        for instance in instances {
            println!(
                "{}\tJava {}",
                instance["id"].as_str().unwrap_or("?"),
                instance["required_java"]
            );
        }
        return;
    }
    if let Some(prepared) = value.get("prepared") {
        println!("{}", serde_json::to_string_pretty(prepared).unwrap());
        return;
    }
    if let Some(result) = value.get("launch") {
        println!(
            "Game exited: {} (cancelled: {}). Log: {}",
            result["exit_code"],
            result["cancelled"],
            result["log_path"].as_str().unwrap_or("?")
        );
        return;
    }
    if let Some(versions) = value.get("versions").and_then(Value::as_array) {
        for version in versions {
            println!(
                "{}\t{}",
                version["id"].as_str().unwrap_or("?"),
                version["type"].as_str().unwrap_or("?")
            );
        }
        return;
    }
    if let Some(install) = value.get("install") {
        println!(
            "Installed {} in {} (downloaded {}, reused {} of {} files)",
            install["version"].as_str().unwrap_or("?"),
            install["game_directory"].as_str().unwrap_or("?"),
            install["downloaded_files"].as_u64().unwrap_or(0),
            install["reused_files"].as_u64().unwrap_or(0),
            install["total_files"].as_u64().unwrap_or(0)
        );
        return;
    }
    if let Some(game) = value.get("game_directory") {
        println!("Game directory: {}", game["path"].as_str().unwrap_or("?"));
        println!(
            "Directory exists: {}",
            game["exists"].as_bool().unwrap_or(false)
        );
    }
    if let Some(platform) = value.get("platform") {
        println!(
            "Platform: {} {}",
            platform["os"].as_str().unwrap_or("?"),
            platform["architecture"].as_str().unwrap_or("?")
        );
    }
    if let Some(config) = value.get("config") {
        println!("Configuration: {}", config["path"].as_str().unwrap_or("?"));
    }
}

fn error_code(error: &Error) -> &'static str {
    match error {
        Error::Update(ccl_core::update::UpdateError::Cancelled) => "cancelled",
        Error::Update(_) => "update_error",
        Error::Diagnostic(_) => "diagnostic_error",
        Error::Online(error) if error.is_cancelled() => "cancelled",
        Error::Online(
            ccl_core::online_content::OnlineError::Conflicts(_)
            | ccl_core::online_content::OnlineError::Conflict { .. },
        ) => "content_conflict",
        Error::Online(ccl_core::online_content::OnlineError::Import(
            ccl_core::import_formats::ImportFormatError::Pack(ModpackError::Conflict { .. }),
        )) => "pack_conflict",
        Error::Online(ccl_core::online_content::OnlineError::Import(
            ccl_core::import_formats::ImportFormatError::Pack(
                ModpackError::Instance(ccl_core::instances::InstanceError::Conflict(_))
                | ModpackError::Install(InstallError::Conflict(_)),
            ),
        )) => "instance_conflict",
        Error::ImportFormat(ccl_core::import_formats::ImportFormatError::Pack(
            ModpackError::Instance(ccl_core::instances::InstanceError::Conflict(_)),
        )) => "instance_conflict",
        Error::Online(ccl_core::online_content::OnlineError::Instance(
            ccl_core::instances::InstanceError::Conflict(_),
        )) => "instance_conflict",
        Error::Online(ccl_core::online_content::OnlineError::Download(
            ccl_core::download::DownloadError::Cancelled,
        )) => "cancelled",
        Error::Online(_) => "online_content_error",
        Error::ImportFormat(ccl_core::import_formats::ImportFormatError::Pack(error))
            if error.is_cancelled() =>
        {
            "cancelled"
        }
        Error::ImportFormat(ccl_core::import_formats::ImportFormatError::Download(
            ccl_core::download::DownloadError::Cancelled,
        )) => "cancelled",
        Error::ImportFormat(ccl_core::import_formats::ImportFormatError::Pack(
            ModpackError::Conflict { .. },
        )) => "pack_conflict",
        Error::ImportFormat(ccl_core::import_formats::ImportFormatError::Pack(_)) => {
            "modpack_error"
        }
        Error::ImportFormat(_) => "import_format_error",
        Error::Modpack(error) if error.is_cancelled() => "cancelled",
        Error::Modpack(ModpackError::Conflict { .. }) => "pack_conflict",
        Error::Modpack(ModpackError::Instance(ccl_core::instances::InstanceError::Conflict(_)))
        | Error::Modpack(ModpackError::Install(InstallError::Conflict(_))) => "instance_conflict",
        Error::Modpack(_) => "modpack_error",
        Error::Content(ccl_core::content::ContentError::Instance(error))
            if error.is_cancelled() =>
        {
            "cancelled"
        }
        Error::Content(ccl_core::content::ContentError::Instance(
            ccl_core::instances::InstanceError::Conflict(_),
        )) => "instance_conflict",
        Error::Content(_) => "content_error",
        Error::Loader(error) if error.is_cancelled() => "cancelled",
        Error::Instance(error) if error.is_cancelled() => "cancelled",
        Error::Loader(_) => "loader_error",
        Error::Instance(ccl_core::instances::InstanceError::Conflict(_)) => "instance_conflict",
        Error::Instance(_) => "instance_error",
        Error::InvalidConfig(_) | Error::ConfigJson { .. } => "configuration_error",
        Error::Io { .. } | Error::NotDirectory(_) => "filesystem_error",
        Error::Network(_) => "network_error",
        Error::Cancelled => "cancelled",
        Error::Auth(ccl_core::auth::AuthError::Cancelled) => "cancelled",
        Error::Auth(_) => "authentication_error",
        Error::Account(ccl_core::accounts::AccountError::Cancelled) => "cancelled",
        Error::Account(_) => "account_error",
        Error::Launch(error) => match error {
            LaunchError::Runtime(
                ccl_core::runtime::RuntimeError::Cancelled
                | ccl_core::runtime::RuntimeError::Download(
                    ccl_core::download::DownloadError::Cancelled,
                ),
            ) => "cancelled",
            LaunchError::Java(_) | LaunchError::Runtime(_) => "java_error",
            LaunchError::Cancelled
            | LaunchError::Download(ccl_core::download::DownloadError::Cancelled) => "cancelled",
            LaunchError::Io { .. } => "filesystem_error",
            _ => "launch_error",
        },
        Error::Install(error) => match error {
            InstallError::VersionNotFound(_)
            | InstallError::InvalidResource(_)
            | InstallError::Metadata(_) => "invalid_version",
            InstallError::Conflict(_) => "instance_conflict",
            InstallError::Io { .. } => "filesystem_error",
            InstallError::Cancelled
            | InstallError::Download(ccl_core::download::DownloadError::Cancelled) => "cancelled",
            _ => "network_error",
        },
    }
}

fn exit_code(error: &Error) -> u8 {
    match error {
        Error::Update(ccl_core::update::UpdateError::Cancelled) => 130,
        Error::Update(_) => 17,
        Error::Diagnostic(_) => 18,
        Error::Online(error) if error.is_cancelled() => 130,
        Error::Online(
            ccl_core::online_content::OnlineError::Conflicts(_)
            | ccl_core::online_content::OnlineError::Conflict { .. },
        ) => 7,
        Error::Online(ccl_core::online_content::OnlineError::Import(
            ccl_core::import_formats::ImportFormatError::Pack(ModpackError::Conflict { .. }),
        )) => 7,
        Error::Online(ccl_core::online_content::OnlineError::Import(
            ccl_core::import_formats::ImportFormatError::Pack(
                ModpackError::Instance(ccl_core::instances::InstanceError::Conflict(_))
                | ModpackError::Install(InstallError::Conflict(_)),
            ),
        )) => 7,
        Error::ImportFormat(ccl_core::import_formats::ImportFormatError::Pack(
            ModpackError::Instance(ccl_core::instances::InstanceError::Conflict(_)),
        )) => 7,
        Error::Online(ccl_core::online_content::OnlineError::Instance(
            ccl_core::instances::InstanceError::Conflict(_),
        )) => 7,
        Error::Online(ccl_core::online_content::OnlineError::Download(
            ccl_core::download::DownloadError::Cancelled,
        )) => 130,
        Error::Online(_) => 15,
        Error::ImportFormat(ccl_core::import_formats::ImportFormatError::Pack(error))
            if error.is_cancelled() =>
        {
            130
        }
        Error::ImportFormat(ccl_core::import_formats::ImportFormatError::Download(
            ccl_core::download::DownloadError::Cancelled,
        )) => 130,
        Error::ImportFormat(ccl_core::import_formats::ImportFormatError::Pack(
            ModpackError::Conflict { .. },
        )) => 7,
        Error::ImportFormat(ccl_core::import_formats::ImportFormatError::Pack(_)) => 13,
        Error::ImportFormat(_) => 16,
        Error::Modpack(error) if error.is_cancelled() => 130,
        Error::Modpack(ModpackError::Conflict { .. })
        | Error::Modpack(ModpackError::Instance(ccl_core::instances::InstanceError::Conflict(_)))
        | Error::Modpack(ModpackError::Install(InstallError::Conflict(_))) => 7,
        Error::Modpack(_) => 13,
        Error::Content(ccl_core::content::ContentError::Instance(error))
            if error.is_cancelled() =>
        {
            130
        }
        Error::Content(ccl_core::content::ContentError::Instance(
            ccl_core::instances::InstanceError::Conflict(_),
        )) => 7,
        Error::Content(_) => 14,
        Error::Loader(error) if error.is_cancelled() => 130,
        Error::Instance(error) if error.is_cancelled() => 130,
        Error::Loader(_) => 11,
        Error::Instance(ccl_core::instances::InstanceError::Conflict(_)) => 7,
        Error::Instance(_) => 12,
        Error::InvalidConfig(_) | Error::ConfigJson { .. } => 3,
        Error::Io { .. } | Error::NotDirectory(_) => 4,
        Error::Network(_) => 5,
        Error::Cancelled => 130,
        Error::Auth(ccl_core::auth::AuthError::Cancelled) => 130,
        Error::Account(ccl_core::accounts::AccountError::Cancelled) => 130,
        Error::Auth(_) | Error::Account(_) => 10,
        Error::Launch(error) => match error {
            LaunchError::Runtime(
                ccl_core::runtime::RuntimeError::Cancelled
                | ccl_core::runtime::RuntimeError::Download(
                    ccl_core::download::DownloadError::Cancelled,
                ),
            ) => 130,
            LaunchError::Java(_) | LaunchError::Runtime(_) => 9,
            LaunchError::Cancelled
            | LaunchError::Download(ccl_core::download::DownloadError::Cancelled) => 130,
            LaunchError::Io { .. } => 4,
            _ => 8,
        },
        Error::Install(error) => match error {
            InstallError::VersionNotFound(_)
            | InstallError::InvalidResource(_)
            | InstallError::Metadata(_) => 6,
            InstallError::Conflict(_) => 7,
            InstallError::Io { .. } => 4,
            InstallError::Cancelled
            | InstallError::Download(ccl_core::download::DownloadError::Cancelled) => 130,
            _ => 5,
        },
    }
}

fn internal_update_command() -> Option<u8> {
    let args: Vec<_> = env::args_os().collect();
    let mode = args.get(1)?.to_str()?;
    if !matches!(
        mode,
        "__build-identity" | "__update-health" | "__update-helper"
    ) {
        return None;
    }
    let result: std::result::Result<Value, String> = (|| match mode {
        "__build-identity" if args.len() == 2 => {
            Ok(serde_json::to_value(ccl_core::update::BuildIdentity::current()).unwrap())
        }
        "__update-health" if args.len() == 5 => {
            let nonce = args[2].to_str().ok_or("invalid nonce")?;
            if nonce.len() != 64 || !nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("invalid nonce".into());
            }
            let report = ccl_core::compatibility::inspect_compatibility(
                Path::new(&args[3]),
                Path::new(&args[4]),
            );
            Ok(
                json!({"nonce":nonce,"build":ccl_core::update::BuildIdentity::current(),"compatible":report.compatible}),
            )
        }
        "__update-helper" if args.len() == 4 => {
            let status = ccl_core::update::execute_helper(
                Path::new(&args[2]),
                args[3].to_str().ok_or("invalid nonce")?,
            )
            .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(status).unwrap())
        }
        _ => Err("invalid internal update invocation".into()),
    })();
    match result {
        Ok(value) => {
            println!("{value}");
            Some(0)
        }
        Err(error) => {
            eprintln!("{error}");
            Some(17)
        }
    }
}

async fn run_update(
    command: &UpdateCommand,
    config: &LauncherConfig,
    config_path: &Path,
    game_root: &Path,
) -> Result<Value> {
    use ccl_core::update::{BuildIdentity, Channel, Installation, UpdateService};
    let install = if let UpdateCommand::Recover {
        installation: Some(path),
    } = command
    {
        Installation::for_recovery(path)?
    } else {
        Installation::for_current_process()?
    };
    match command {
        UpdateCommand::Status => Ok(
            json!({"ok":true,"update_status":install.status()?,"build":BuildIdentity::current()}),
        ),
        UpdateCommand::Rollback => {
            let status = install.prepare_rollback()?;
            let helper_pid = install.launch_helper()?;
            Ok(
                json!({"ok":true,"update_status":status,"helper_pid":helper_pid,"note":"helper completes rollback after this process exits; inspect update status"}),
            )
        }
        UpdateCommand::Recover { .. } => {
            let status = install.status()?;
            let helper_pid = install.launch_helper()?;
            Ok(json!({"ok":true,"update_status":status,"helper_pid":helper_pid}))
        }
        UpdateCommand::Check { channel } | UpdateCommand::Apply { channel } => {
            let selected = match channel.as_deref() {
                Some("stable") => Channel::Stable,
                Some("dev") => Channel::Dev,
                _ => install.channel()?,
            };
            if channel.is_some() {
                install.set_channel(selected)?;
            }
            let service =
                UpdateService::new(&config.downloads)?.with_task_context(operation_context());
            let check = service
                .check(selected, BuildIdentity::current(), &signal_token())
                .await?;
            if matches!(command, UpdateCommand::Check { .. }) || !check.available {
                return Ok(json!({"ok":true,"update":check}));
            }
            let candidate = check
                .candidate
                .as_ref()
                .ok_or_else(|| Error::InvalidConfig("no published update".into()))?;
            let status = service
                .stage(candidate, &install, config_path, game_root, &signal_token())
                .await?;
            let helper_pid = install.launch_helper()?;
            Ok(
                json!({"ok":true,"update_status":status,"helper_pid":helper_pid,"note":"helper completes update after this process exits; inspect update status"}),
            )
        }
    }
}
