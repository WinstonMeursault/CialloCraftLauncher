mod browser;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use ccl_core::{
    CancellationToken, Error, Result, TaskContext, TaskEvent,
    accounts::{AccountStore, OsProviderKeyStore, OsSecretStore},
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
    online_content::{
        OnlineContentService, PackInstallOptions, PackMergeTarget, Provider, SearchQuery,
    },
    profiles::RootProfile,
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
        /// Select a root profile or managed instance instead of the shared game root.
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
    /// Manage provider API keys in the OS credential vault.
    Credentials {
        #[command(subcommand)]
        command: CredentialsCommand,
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
        /// Launch a root profile or managed instance by ID; use version:ID or instance:ID to disambiguate.
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
        /// Maximum Java heap: auto or an integer MiB limit. Overrides instance/global settings.
        #[arg(long)]
        memory: Option<ccl_core::memory::MemoryPolicy>,
        /// Prepare and print the argument vector without starting the game.
        #[arg(long)]
        dry_run: bool,
    },
    /// Read the latest launch log, or select a run ID.
    Logs {
        /// Read logs for a root profile or managed instance; use version:ID or instance:ID to disambiguate.
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
        /// Update an existing managed pack instance instead of creating a new instance.
        #[arg(long)]
        update_existing: bool,
    },
    Search {
        #[arg(value_parser=parse_provider)]
        provider: Provider,
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
    /// Read or save an installed profile's memory override (auto, MiB, or inherit).
    Memory {
        #[arg(required_unless_present = "instance", conflicts_with = "instance")]
        version: Option<String>,
        #[arg(long)]
        instance: Option<String>,
        /// Save auto, an integer MiB limit, or inherit to remove the override.
        #[arg(long = "set")]
        value: Option<String>,
    },
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
enum CredentialsCommand {
    /// Manage your own CurseForge API key.
    Curseforge {
        #[command(subcommand)]
        command: CredentialKeyCommand,
    },
}

#[derive(Subcommand)]
enum CredentialKeyCommand {
    /// Enter a key through a hidden terminal prompt and save it in the OS credential vault.
    Set {
        /// Captures accidental positional input so clap does not echo a secret in its error.
        #[arg(hide = true)]
        accidental_input: Vec<String>,
    },
    /// Show whether a key is saved and whether an environment override is active.
    Status,
    /// Remove the saved key (does not change an environment override).
    Remove,
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

enum ProfileTarget {
    Root(RootProfile),
    Managed(String),
}

/// Resolve profile selectors without guessing when a root version and a managed instance
/// share an ID. Explicit selectors are `version:<id>` and `instance:<id>`.
fn select_profile_target(
    service: &InstanceService,
    root: &Path,
    selector: &str,
) -> Result<ProfileTarget> {
    let (requested, id) = if let Some(id) = selector.strip_prefix("version:") {
        (Some("version"), id)
    } else if let Some(id) = selector.strip_prefix("instance:") {
        (Some("instance"), id)
    } else {
        (None, selector)
    };
    let root_profiles = RootProfile::list(root).map_err(profile_error)?;
    let root_exists = root_profiles.iter().any(|profile| profile.id() == id);
    let managed_exists = service.list()?.iter().any(|instance| instance.id == id);
    match requested {
        Some("version") => Ok(ProfileTarget::Root(
            RootProfile::open(root, id).map_err(profile_error)?,
        )),
        Some("instance") => {
            if !managed_exists {
                return Err(Error::Instance(
                    ccl_core::instances::InstanceError::NotFound(id.into()),
                ));
            }
            Ok(ProfileTarget::Managed(id.into()))
        }
        _ if root_exists && managed_exists => Err(Error::InvalidConfig(format!(
            "ambiguous profile ID {id}; choose version:{id} or instance:{id}"
        ))),
        _ if root_exists => Ok(ProfileTarget::Root(
            RootProfile::open(root, id).map_err(profile_error)?,
        )),
        _ if managed_exists => Ok(ProfileTarget::Managed(id.into())),
        _ => Err(Error::Instance(
            ccl_core::instances::InstanceError::NotFound(id.into()),
        )),
    }
}

fn is_explicit_profile_selector(value: &str) -> bool {
    value.starts_with("version:") || value.starts_with("instance:")
}

fn profile_error(error: ccl_core::profiles::ProfileError) -> Error {
    use ccl_core::{instances::InstanceError, profiles::ProfileError};
    match error {
        ProfileError::InvalidId(id) => InstanceError::InvalidId(id).into(),
        ProfileError::NotFound(id) => InstanceError::NotFound(id).into(),
        ProfileError::Conflict(message) => InstanceError::Conflict(message).into(),
        ProfileError::Referenced(ids) => {
            InstanceError::Conflict(format!("referenced by: {ids}")).into()
        }
        ProfileError::Metadata { path, source } => InstanceError::Metadata { path, source }.into(),
        ProfileError::Io { path, source } => InstanceError::Io { path, source }.into(),
        ProfileError::Instance(error) => error.into(),
        ProfileError::UnsafePath(path) => {
            Error::InvalidConfig(format!("unsafe profile path: {}", path.display()))
        }
    }
}

fn sanitize_workspace_error(error: Error, workspace: &Path) -> Error {
    let message = error.to_string();
    let workspace = workspace.to_string_lossy();
    if message.contains(workspace.as_ref()) {
        Error::InvalidConfig(message.replace(workspace.as_ref(), "profile workspace"))
    } else {
        error
    }
}

fn read_profile_logs(
    candidate_roots: &[PathBuf],
    profile_id: &str,
    legacy_version: &str,
    requested_run: Option<&str>,
    lines: usize,
) -> Result<(String, String)> {
    let (selected_root, run_id) =
        profile_log_location(candidate_roots, profile_id, legacy_version, requested_run)?;
    Ok(launch::read_logs(&selected_root, Some(&run_id), lines)?)
}

fn profile_log_location(
    candidate_roots: &[PathBuf],
    profile_id: &str,
    legacy_version: &str,
    requested_run: Option<&str>,
) -> Result<(PathBuf, String)> {
    let mut matching = Vec::<(u64, PathBuf, String)>::new();
    for root in candidate_roots {
        let ccl_directory = root.join(".ccl");
        let directory = root.join(".ccl/logs");
        for path in [&ccl_directory, &directory] {
            match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(Error::InvalidConfig(
                        "launch log path contains a symlink".into(),
                    ));
                }
                Ok(metadata) if path == &directory && !metadata.is_dir() => {
                    return Err(Error::InvalidConfig(
                        "launch log path is not a directory".into(),
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(source) => {
                    return Err(Error::Io {
                        path: path.to_path_buf(),
                        source,
                    });
                }
                _ => {}
            }
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(Error::Io {
                    path: directory.clone(),
                    source,
                });
            }
        };
        for entry in entries.take(10_000) {
            let entry = entry.map_err(|source| Error::Io {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            if !entry
                .file_type()
                .map_err(|source| Error::Io {
                    path: path.clone(),
                    source,
                })?
                .is_file()
                || path.extension().is_none_or(|extension| extension != "json")
                || fs::symlink_metadata(&path)
                    .map_err(|source| Error::Io {
                        path: path.clone(),
                        source,
                    })?
                    .len()
                    > 64 * 1024
            {
                continue;
            }
            let record: ccl_core::launch::LaunchResult =
                match serde_json::from_slice(&fs::read(&path).map_err(|source| Error::Io {
                    path: path.clone(),
                    source,
                })?) {
                    Ok(record) => record,
                    Err(_) => continue,
                };
            let Some(id) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if record.run_id != id
                || record
                    .profile_id
                    .as_deref()
                    .map_or(record.version != legacy_version, |id| id != profile_id)
                || requested_run.is_some_and(|requested| requested != id)
            {
                continue;
            }
            matching.push((record.started_at_ms, root.clone(), id.to_owned()));
        }
    }
    matching.sort_by_key(|(started, _, _)| std::cmp::Reverse(*started));
    let (_, selected_root, run_id) = matching.into_iter().next().ok_or_else(|| {
        Error::InvalidConfig(if requested_run.is_some() {
            "selected launch log does not belong to this profile".into()
        } else {
            format!("no launch logs found for profile {profile_id}")
        })
    })?;
    let log_path = selected_root
        .join(".ccl/logs")
        .join(format!("{run_id}.log"));
    let log_metadata = fs::symlink_metadata(&log_path).map_err(|source| Error::Io {
        path: log_path.clone(),
        source,
    })?;
    if log_metadata.file_type().is_symlink() || !log_metadata.is_file() {
        return Err(Error::InvalidConfig(
            "launch log is not a regular file".into(),
        ));
    }
    Ok((selected_root, run_id))
}

fn content_id(command: &ContentCommand) -> &str {
    match command {
        ContentCommand::List { id, .. }
        | ContentCommand::Add { id, .. }
        | ContentCommand::Remove { id, .. }
        | ContentCommand::Enable { id, .. }
        | ContentCommand::Disable { id, .. }
        | ContentCommand::Backup { id, .. }
        | ContentCommand::Restore { id, .. } => id,
    }
}

fn content_command_mutates(command: &ContentCommand) -> bool {
    matches!(
        command,
        ContentCommand::Add { .. }
            | ContentCommand::Remove { .. }
            | ContentCommand::Enable { .. }
            | ContentCommand::Disable { .. }
            | ContentCommand::Restore { .. }
    )
}

fn run_content_command(
    service: &ContentService,
    id: &str,
    command: &ContentCommand,
) -> Result<Value> {
    match command {
        ContentCommand::List { kind, .. } => {
            let content = service.list(id, *kind)?;
            Ok(json!({ "ok": true, "id": id, "kind": kind, "content": content }))
        }
        ContentCommand::Add {
            kind, source, name, ..
        } => {
            let entry = service.add(id, *kind, source, name.as_deref())?;
            Ok(json!({ "ok": true, "id": id, "kind": kind, "entry": entry }))
        }
        ContentCommand::Remove { kind, name, .. } => {
            service.remove(id, *kind, name)?;
            Ok(json!({ "ok": true, "id": id, "kind": kind, "removed": name }))
        }
        ContentCommand::Enable { name, .. } => {
            let entry = service.set_enabled(id, name, true)?;
            Ok(json!({ "ok": true, "id": id, "entry": entry }))
        }
        ContentCommand::Disable { name, .. } => {
            let entry = service.set_enabled(id, name, false)?;
            Ok(json!({ "ok": true, "id": id, "entry": entry }))
        }
        ContentCommand::Backup { output, .. } => {
            service.backup(id, output)?;
            Ok(json!({ "ok": true, "id": id, "backup": output }))
        }
        ContentCommand::Restore { archive, .. } => {
            service.restore(id, archive)?;
            Ok(json!({ "ok": true, "id": id, "restored": true }))
        }
    }
}

fn instance_listing(root: &Path, config: &ccl_core::config::DownloadConfig) -> Result<Value> {
    let profiles = RootProfile::list(root).map_err(profile_error)?;
    let installed = launch::list_installed_versions(root)?;
    let required_java = installed
        .into_iter()
        .map(|version| (version.id, version.required_java))
        .collect::<std::collections::HashMap<_, _>>();
    let managed = InstanceService::new(root.to_path_buf(), config.clone())?.list()?;
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for item in profiles
        .iter()
        .map(RootProfile::id)
        .chain(managed.iter().map(|item| item.id.as_str()))
    {
        *counts.entry(item.to_owned()).or_default() += 1;
    }
    let root_profiles: Vec<Value> = profiles
        .iter()
        .map(|profile| {
            let id = profile.id();
            let mut item = json!({
                "id": id,
                "kind": "root_profile",
                "required_java": required_java.get(id),
                "game_directory": profile.game_directory(),
                "resource_root": profile.resource_root(),
                "profile_directory": profile.profile_directory(),
                "metadata": profile.metadata(),
            });
            if counts.get(id).copied().unwrap_or_default() > 1 {
                item["selector"] = json!(format!("version:{id}"));
            }
            item
        })
        .collect();
    let managed_instances: Vec<Value> = managed
        .iter()
        .map(|instance| {
            let mut item = json!({
                "id": instance.id,
                "kind": "managed_instance",
                "game_directory": instance.game_directory,
                "metadata": instance,
            });
            if counts.get(&instance.id).copied().unwrap_or_default() > 1 {
                item["selector"] = json!(format!("instance:{}", instance.id));
            }
            item
        })
        .collect();
    let instances = root_profiles
        .iter()
        .chain(managed_instances.iter())
        .cloned()
        .collect::<Vec<_>>();
    Ok(json!({
        "ok": true,
        "root_profiles": root_profiles,
        "managed_instances": managed_instances,
        "instances": instances,
    }))
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
        Command::Credentials { command } => match command {
            CredentialsCommand::Curseforge { command } => {
                let store = OsProviderKeyStore::for_config_file(&config_path)?;
                match command {
                    CredentialKeyCommand::Set { accidental_input } => {
                        if !accidental_input.is_empty() {
                            return Err(Error::InvalidConfig(
                                "enter the API key only at the hidden terminal prompt, without command arguments".into(),
                            ));
                        }
                        let key =
                            rpassword::prompt_password("CurseForge API key: ").map_err(|_| {
                                Error::InvalidConfig(
                                    "a terminal is required to enter the CurseForge API key".into(),
                                )
                            })?;
                        store.set_curseforge(&key)?;
                        Ok(json!({"ok":true,"provider":"curseforge","saved":true}))
                    }
                    CredentialKeyCommand::Status => Ok(json!({
                        "ok":true,
                        "provider":"curseforge",
                        "saved":store.get_curseforge()?.is_some(),
                        "environment_override":env::var_os("CCL_CURSEFORGE_API_KEY").is_some(),
                    })),
                    CredentialKeyCommand::Remove => {
                        store.remove_curseforge()?;
                        Ok(json!({"ok":true,"provider":"curseforge","saved":false}))
                    }
                }
            }
        },
        Command::Settings { command } => {
            let store = ConfigStore::new(&config_path);
            match command {
                SettingsCommand::Memory {
                    version,
                    instance,
                    value,
                } => {
                    let instances =
                        InstanceService::new(resolution.path.clone(), config.downloads.clone())?;
                    let (directory, id) = if let Some(selector) = instance {
                        match select_profile_target(&instances, &resolution.path, selector)? {
                            ProfileTarget::Root(profile) => (
                                profile.resource_root().to_path_buf(),
                                profile.id().to_owned(),
                            ),
                            ProfileTarget::Managed(id) => (instances.game_directory(&id)?, id),
                        }
                    } else {
                        let id = version.as_ref().expect("clap requires version or instance");
                        let profile =
                            RootProfile::open(&resolution.path, id).map_err(profile_error)?;
                        (
                            profile.resource_root().to_path_buf(),
                            profile.id().to_owned(),
                        )
                    };
                    let key = ccl_core::memory::profile_key(&directory, &id).map_err(|source| {
                        Error::Io {
                            path: directory.clone(),
                            source,
                        }
                    })?;
                    if let Some(value) = value {
                        let policy = if value == "inherit" {
                            None
                        } else {
                            Some(
                                value
                                    .parse::<ccl_core::memory::MemoryPolicy>()
                                    .map_err(Error::InvalidConfig)?,
                            )
                        };
                        store.update(|config| {
                            if let Some(policy) = policy {
                                config.launch.instance_memory.insert(key.clone(), policy);
                            } else {
                                config.launch.instance_memory.remove(&key);
                            }
                            Ok(())
                        })?;
                    }
                    let current = LauncherConfig::read_or_default(&config_path)?;
                    Ok(
                        json!({"ok":true,"id":id,"override":current.launch.instance_memory.get(&key),"effective":current.launch.memory_policy(&directory, &id, None)?}),
                    )
                }
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
            let mut diagnostic_root = resolution.path.clone();
            let mut log_root = resolution.path.clone();
            let mut launcher_data_root = resolution.path.clone();
            let mut diagnostic_run = run.clone();
            let profile_identity = if let Some(selector) = instance {
                let instances =
                    InstanceService::new(resolution.path.clone(), config.downloads.clone())?;
                let (runtime, id, legacy_version, resource_root) =
                    match select_profile_target(&instances, &resolution.path, selector)? {
                        ProfileTarget::Root(profile) => (
                            profile.game_directory().to_path_buf(),
                            profile.id().to_owned(),
                            profile.id().to_owned(),
                            profile.resource_root().to_path_buf(),
                        ),
                        ProfileTarget::Managed(id) => {
                            let metadata = instances.metadata(&id)?;
                            let legacy_version = metadata
                                .loader_version
                                .clone()
                                .unwrap_or_else(|| metadata.minecraft.clone());
                            (
                                metadata.game_directory,
                                id,
                                legacy_version,
                                resolution.path.clone(),
                            )
                        }
                    };
                diagnostic_root = runtime.clone();
                log_root = resource_root.clone();
                launcher_data_root = resource_root.clone();
                let legacy_roots = if runtime == resource_root {
                    vec![resource_root.clone()]
                } else {
                    vec![resource_root.clone(), runtime]
                };
                if run.is_some() || (*include_log_preview && export.is_some()) {
                    let (selected_root, selected_run) =
                        profile_log_location(&legacy_roots, &id, &legacy_version, run.as_deref())?;
                    log_root = selected_root;
                    diagnostic_run = Some(selected_run);
                }
                Some((id, legacy_version))
            } else {
                None
            };
            let mut service = ccl_core::diagnostics::DiagnosticsService::new(
                &diagnostic_root,
                launcher_data_root.join(".ccl"),
            )
            .with_log_root(log_root);
            if let Some((id, legacy_version)) = profile_identity {
                service = service.with_profile(id, legacy_version);
            }
            if let Some(path) = export {
                let report = service.export(&ccl_core::diagnostics::ExportOptions {
                    run_id: diagnostic_run,
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
            } else if let Some(id) = diagnostic_run {
                Ok(json!({"ok":true,"diagnosis":service.diagnose_run(&id)?}))
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
        Command::Instances => {
            let listing = instance_listing(&resolution.path, &config.downloads)?;
            Ok(
                json!({"ok":true,"game_directory":resolution,"root_profiles":listing["root_profiles"],"managed_instances":listing["managed_instances"],"instances":listing["instances"]}),
            )
        }
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
            let service = InstanceService::new(resolution.path.clone(), config.downloads.clone())?;
            match command {
                InstanceCommand::List => instance_listing(&resolution.path, &config.downloads),
                InstanceCommand::Show { id } => {
                    match select_profile_target(&service, &resolution.path, id)? {
                        ProfileTarget::Root(profile) => Ok(
                            json!({ "ok": true, "instance": profile.metadata(), "kind": "root_profile" }),
                        ),
                        ProfileTarget::Managed(id) => Ok(
                            json!({ "ok": true, "instance": service.metadata(&id)?, "kind": "managed_instance" }),
                        ),
                    }
                }
                InstanceCommand::Install {
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
                    let target = match select_profile_target(&service, &resolution.path, id) {
                        Ok(target) => Some(target),
                        Err(Error::Instance(ccl_core::instances::InstanceError::NotFound(_))) => {
                            None
                        }
                        Err(error) => return Err(error),
                    };
                    let instance = match target {
                        Some(ProfileTarget::Root(profile)) => {
                            let workspace = profile
                                .begin_workspace(config.downloads.clone())
                                .map_err(profile_error)?;
                            let workspace_root = workspace.root().to_path_buf();
                            let profile_id = profile.id().to_owned();
                            match workspace
                                .service()
                                .install(&profile_id, &request, Some(context))
                                .await
                            {
                                Ok(_) => match workspace.commit() {
                                    Ok(instance) => instance,
                                    Err(error) => {
                                        return Err(sanitize_workspace_error(
                                            profile_error(error),
                                            &workspace_root,
                                        ));
                                    }
                                },
                                Err(error) => {
                                    let _ = workspace.abort();
                                    return Err(sanitize_workspace_error(
                                        error.into(),
                                        &workspace_root,
                                    ));
                                }
                            }
                        }
                        Some(ProfileTarget::Managed(id)) => {
                            service.install(&id, &request, Some(context)).await?
                        }
                        None => {
                            service
                                .install_new_loader(id, &request, Some(context))
                                .await?
                        }
                    };
                    Ok(json!({ "ok": true, "instance": instance }))
                }
                InstanceCommand::Update {
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
                    let instance = match select_profile_target(&service, &resolution.path, id)? {
                        ProfileTarget::Root(profile) => {
                            let profile_id = profile.id().to_owned();
                            let workspace = profile
                                .begin_workspace(config.downloads.clone())
                                .map_err(profile_error)?;
                            let workspace_root = workspace.root().to_path_buf();
                            match workspace
                                .service()
                                .install(&profile_id, &request, Some(context))
                                .await
                            {
                                Ok(_) => match workspace.commit() {
                                    Ok(instance) => instance,
                                    Err(error) => {
                                        return Err(sanitize_workspace_error(
                                            profile_error(error),
                                            &workspace_root,
                                        ));
                                    }
                                },
                                Err(error) => {
                                    let _ = workspace.abort();
                                    return Err(sanitize_workspace_error(
                                        error.into(),
                                        &workspace_root,
                                    ));
                                }
                            }
                        }
                        ProfileTarget::Managed(id) => {
                            service.install(&id, &request, Some(context)).await?
                        }
                    };
                    Ok(json!({ "ok": true, "instance": instance }))
                }
                InstanceCommand::Rename { id, destination } => {
                    match select_profile_target(&service, &resolution.path, id)? {
                        ProfileTarget::Root(profile) => Ok(
                            json!({"ok":true,"instance":profile.rename(destination).map_err(profile_error)?,"kind":"root_profile"}),
                        ),
                        ProfileTarget::Managed(id) => Ok(
                            json!({"ok":true,"instance":service.rename(&id,destination)?,"kind":"managed_instance"}),
                        ),
                    }
                }
                InstanceCommand::Copy { id, destination } => {
                    match select_profile_target(&service, &resolution.path, id)? {
                        ProfileTarget::Root(profile) => Ok(
                            json!({"ok":true,"instance":profile.copy_to(&resolution.path,destination,config.downloads.clone()).map_err(profile_error)?,"kind":"managed_instance"}),
                        ),
                        ProfileTarget::Managed(id) => Ok(
                            json!({"ok":true,"instance":service.duplicate(&id,destination)?,"kind":"managed_instance"}),
                        ),
                    }
                }
                InstanceCommand::Delete { id, .. } => {
                    match select_profile_target(&service, &resolution.path, id)? {
                        ProfileTarget::Root(profile) => {
                            profile.delete().map_err(profile_error)?;
                            Ok(json!({"ok":true,"deleted":id,"kind":"root_profile"}))
                        }
                        ProfileTarget::Managed(id) => {
                            service.delete(&id)?;
                            Ok(json!({"ok":true,"deleted":id,"kind":"managed_instance"}))
                        }
                    }
                }
                InstanceCommand::History { id } => {
                    match select_profile_target(&service, &resolution.path, id)? {
                        ProfileTarget::Root(profile) => Ok(
                            json!({"ok":true,"history":profile.history().map_err(profile_error)?,"kind":"root_profile"}),
                        ),
                        ProfileTarget::Managed(id) => Ok(
                            json!({"ok":true,"history":service.history(&id)?,"kind":"managed_instance"}),
                        ),
                    }
                }
                InstanceCommand::Restore { id, revision } => {
                    match select_profile_target(&service, &resolution.path, id)? {
                        ProfileTarget::Root(profile) => Ok(
                            json!({"ok":true,"instance":profile.restore_revision(revision).map_err(profile_error)?,"kind":"root_profile"}),
                        ),
                        ProfileTarget::Managed(id) => Ok(
                            json!({"ok":true,"instance":service.restore_revision(&id,revision)?,"kind":"managed_instance"}),
                        ),
                    }
                }
                InstanceCommand::DiskUsage { id } => {
                    match select_profile_target(&service, &resolution.path, id)? {
                        ProfileTarget::Root(profile) => Ok(
                            json!({"ok":true,"disk_usage":profile.disk_usage().map_err(profile_error)?,"kind":"root_profile"}),
                        ),
                        ProfileTarget::Managed(id) => Ok(
                            json!({"ok":true,"disk_usage":service.disk_usage(&id)?,"kind":"managed_instance"}),
                        ),
                    }
                }
                InstanceCommand::Clean {
                    id,
                    keep,
                    include_previous,
                    apply,
                } => match select_profile_target(&service, &resolution.path, id)? {
                    ProfileTarget::Root(profile) => Ok(
                        json!({"ok":true,"cleanup":profile.cleanup(*keep,*include_previous,*apply).map_err(profile_error)?,"kind":"root_profile"}),
                    ),
                    ProfileTarget::Managed(id) => Ok(
                        json!({"ok":true,"cleanup":service.cleanup(&id,*keep,*include_previous,*apply)?,"kind":"managed_instance"}),
                    ),
                },
                InstanceCommand::Rollback { id } => {
                    match select_profile_target(&service, &resolution.path, id)? {
                        ProfileTarget::Root(profile) => {
                            let revision = profile
                                .history()
                                .map_err(profile_error)?
                                .into_iter()
                                .last()
                                .ok_or_else(|| {
                                    Error::InvalidConfig(format!(
                                        "no rollback history for {}",
                                        profile.id()
                                    ))
                                })?;
                            Ok(
                                json!({"ok":true,"instance":profile.rollback(&revision.revision).map_err(profile_error)?,"kind":"root_profile"}),
                            )
                        }
                        ProfileTarget::Managed(id) => {
                            Ok(json!({ "ok": true, "instance": service.rollback(&id).await? }))
                        }
                    }
                }
                InstanceCommand::UninstallLoader { id } => {
                    let instance = match select_profile_target(&service, &resolution.path, id)? {
                        ProfileTarget::Root(profile) => {
                            let workspace = profile
                                .begin_workspace(config.downloads.clone())
                                .map_err(profile_error)?;
                            let workspace_root = workspace.root().to_path_buf();
                            match workspace
                                .service()
                                .uninstall_loader(id, Some(operation_context()))
                                .await
                            {
                                Ok(_) => match workspace.commit() {
                                    Ok(instance) => instance,
                                    Err(error) => {
                                        return Err(sanitize_workspace_error(
                                            profile_error(error),
                                            &workspace_root,
                                        ));
                                    }
                                },
                                Err(error) => {
                                    let _ = workspace.abort();
                                    return Err(sanitize_workspace_error(
                                        error.into(),
                                        &workspace_root,
                                    ));
                                }
                            }
                        }
                        ProfileTarget::Managed(id) => {
                            service
                                .uninstall_loader(&id, Some(operation_context()))
                                .await?
                        }
                    };
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
                let instances =
                    InstanceService::new(resolution.path.clone(), config.downloads.clone())?;
                match select_profile_target(&instances, &resolution.path, id)? {
                    ProfileTarget::Managed(id) => {
                        let service =
                            ModpackService::new(resolution.path.clone(), config.downloads.clone())?;
                        Ok(json!({ "ok": true, "receipt": service.show(&id)? }))
                    }
                    ProfileTarget::Root(profile) => {
                        let workspace = profile
                            .begin_workspace(config.downloads.clone())
                            .map_err(profile_error)?;
                        let workspace_root = workspace.root().to_path_buf();
                        let receipt = workspace
                            .modpack_service(config.downloads.clone())
                            .map_err(profile_error)
                            .and_then(|service| service.show(profile.id()).map_err(Error::Modpack));
                        match receipt {
                            Ok(receipt) => {
                                if let Err(error) = workspace.abort() {
                                    return Err(sanitize_workspace_error(
                                        profile_error(error),
                                        &workspace_root,
                                    ));
                                }
                                Ok(json!({ "ok": true, "receipt": receipt }))
                            }
                            Err(error) => {
                                let _ = workspace.abort();
                                Err(sanitize_workspace_error(error, &workspace_root))
                            }
                        }
                    }
                }
            }
            PackCommand::Import {
                id,
                archive,
                include_optional,
                replace_modified,
                allow_hosts,
            } => {
                let instances =
                    InstanceService::new(resolution.path.clone(), config.downloads.clone())?;
                let target = match select_profile_target(&instances, &resolution.path, id) {
                    Ok(target) => Some(target),
                    Err(Error::Instance(ccl_core::instances::InstanceError::NotFound(_)))
                        if !is_explicit_profile_selector(id) =>
                    {
                        None
                    }
                    Err(error) => return Err(error),
                };
                let options = ImportOptions {
                    include_optional: *include_optional,
                    replace_modified: *replace_modified,
                    allowed_hosts: allow_hosts.clone(),
                };
                let is_modrinth = archive
                    .extension()
                    .and_then(|v| v.to_str())
                    .is_some_and(|v| v.eq_ignore_ascii_case("mrpack"))
                    || ImportFormatService::is_modrinth(archive)?;
                let import_format_options = ImportOptionsExt {
                    include_optional: *include_optional,
                    replace_modified: *replace_modified,
                    curseforge_api_key: if !is_modrinth
                        && ImportFormatService::inspect(archive)?["format"] == "curseforge"
                    {
                        optional_curseforge_key(&config_path)?
                    } else {
                        None
                    },
                    allowed_hosts: allow_hosts.clone(),
                };
                match target {
                    Some(ProfileTarget::Root(profile)) => {
                        let workspace = profile
                            .begin_workspace(config.downloads.clone())
                            .map_err(profile_error)?;
                        let workspace_root = workspace.root().to_path_buf();
                        let has_pack_receipt =
                            profile.game_directory().join(".ccl/modpack.json").is_file();
                        let imported = async {
                            if !has_pack_receipt {
                                let scratch_id = format!(
                                    "ccl-cli-pack-{}-{}",
                                    std::process::id(),
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_nanos()
                                );
                                let imported = if is_modrinth {
                                    ModpackService::new(
                                        workspace_root.clone(),
                                        config.downloads.clone(),
                                    )?
                                    .import_new(
                                        &scratch_id,
                                        archive,
                                        &options,
                                        Some(operation_context()),
                                    )
                                    .await
                                    .map_err(Error::Modpack)?
                                } else {
                                    ImportFormatService::new(
                                        workspace_root.clone(),
                                        config.downloads.clone(),
                                    )
                                    .import_new(
                                        &scratch_id,
                                        archive,
                                        &import_format_options,
                                        Some(operation_context()),
                                    )
                                    .await
                                    .map_err(Error::ImportFormat)?
                                };
                                OnlineContentService::merge_imported_pack_into_profile(
                                    PackMergeTarget {
                                        root: workspace_root.clone(),
                                        config: config.downloads.clone(),
                                        profile_id: profile.id().to_owned(),
                                        runtime_subdirectory: workspace
                                            .runtime_subdirectory()
                                            .to_path_buf(),
                                    },
                                    &imported.id,
                                )
                                .map_err(Error::Online)
                            } else if is_modrinth {
                                workspace
                                    .modpack_service(config.downloads.clone())
                                    .map_err(profile_error)?
                                    .import(
                                        profile.id(),
                                        archive,
                                        &options,
                                        Some(operation_context()),
                                    )
                                    .await
                                    .map_err(Error::Modpack)
                            } else {
                                ImportFormatService::new_with_runtime_subdirectory(
                                    workspace_root.clone(),
                                    config.downloads.clone(),
                                    workspace.runtime_subdirectory().to_path_buf(),
                                )
                                .import(
                                    profile.id(),
                                    archive,
                                    &import_format_options,
                                    Some(operation_context()),
                                )
                                .await
                                .map_err(Error::ImportFormat)
                            }
                        }
                        .await;
                        match imported {
                            Ok(_) => {
                                let instance = match workspace.commit() {
                                    Ok(instance) => instance,
                                    Err(error) => {
                                        return Err(sanitize_workspace_error(
                                            profile_error(error),
                                            &workspace_root,
                                        ));
                                    }
                                };
                                Ok(json!({"ok":true,"instance":instance,"pack_imported":true}))
                            }
                            Err(error) => {
                                let _ = workspace.abort();
                                Err(sanitize_workspace_error(error, &workspace_root))
                            }
                        }
                    }
                    Some(ProfileTarget::Managed(id)) => {
                        let instance = if is_modrinth {
                            ModpackService::new(resolution.path.clone(), config.downloads.clone())?
                                .import(&id, archive, &options, Some(operation_context()))
                                .await?
                        } else {
                            ImportFormatService::new(
                                resolution.path.clone(),
                                config.downloads.clone(),
                            )
                            .import(
                                &id,
                                archive,
                                &import_format_options,
                                Some(operation_context()),
                            )
                            .await?
                        };
                        Ok(json!({"ok":true,"instance":instance,"pack_imported":true}))
                    }
                    None => {
                        let instance = if is_modrinth {
                            ModpackService::new(resolution.path.clone(), config.downloads.clone())?
                                .import(id, archive, &options, Some(operation_context()))
                                .await?
                        } else {
                            ImportFormatService::new(resolution.path, config.downloads.clone())
                                .import(
                                    id,
                                    archive,
                                    &import_format_options,
                                    Some(operation_context()),
                                )
                                .await?
                        };
                        Ok(json!({"ok":true,"instance":instance,"pack_imported":true}))
                    }
                }
            }
            PackCommand::Export {
                id,
                output,
                include_local_content,
            } => {
                let instances =
                    InstanceService::new(resolution.path.clone(), config.downloads.clone())?;
                match select_profile_target(&instances, &resolution.path, id)? {
                    ProfileTarget::Managed(id) => {
                        let service =
                            ModpackService::new(resolution.path.clone(), config.downloads.clone())?;
                        let manifest = service.export(&id, output, *include_local_content)?;
                        Ok(json!({ "ok": true, "archive": output, "manifest": manifest }))
                    }
                    ProfileTarget::Root(profile) => {
                        let workspace = profile
                            .begin_workspace(config.downloads.clone())
                            .map_err(profile_error)?;
                        let workspace_root = workspace.root().to_path_buf();
                        let manifest = workspace
                            .modpack_service(config.downloads.clone())
                            .map_err(profile_error)
                            .and_then(|service| {
                                service
                                    .export(profile.id(), output, *include_local_content)
                                    .map_err(Error::Modpack)
                            });
                        match manifest {
                            Ok(manifest) => {
                                if let Err(error) = workspace.abort() {
                                    return Err(sanitize_workspace_error(
                                        profile_error(error),
                                        &workspace_root,
                                    ));
                                }
                                Ok(json!({ "ok": true, "archive": output, "manifest": manifest }))
                            }
                            Err(error) => {
                                let _ = workspace.abort();
                                Err(sanitize_workspace_error(error, &workspace_root))
                            }
                        }
                    }
                }
            }
        },
        Command::Content { command } => {
            let instances =
                InstanceService::new(resolution.path.clone(), config.downloads.clone())?;
            let id = content_id(command);
            match select_profile_target(&instances, &resolution.path, id)? {
                ProfileTarget::Managed(id) => {
                    let service = ContentService::new(resolution.path, config.downloads.clone())?;
                    run_content_command(&service, &id, command)
                }
                ProfileTarget::Root(profile) => {
                    let workspace = profile
                        .begin_workspace(config.downloads.clone())
                        .map_err(profile_error)?;
                    let workspace_root = workspace.root().to_path_buf();
                    let service = match workspace.content_service(config.downloads.clone()) {
                        Ok(service) => service,
                        Err(error) => {
                            let _ = workspace.abort();
                            return Err(sanitize_workspace_error(
                                profile_error(error),
                                &workspace_root,
                            ));
                        }
                    };
                    let result = run_content_command(&service, profile.id(), command);
                    match result {
                        Ok(value) if content_command_mutates(command) => {
                            if let Err(error) = workspace.commit() {
                                return Err(sanitize_workspace_error(
                                    profile_error(error),
                                    &workspace_root,
                                ));
                            }
                            Ok(value)
                        }
                        Ok(value) => {
                            if let Err(error) = workspace.abort() {
                                return Err(sanitize_workspace_error(
                                    profile_error(error),
                                    &workspace_root,
                                ));
                            }
                            Ok(value)
                        }
                        Err(error) => {
                            let _ = workspace.abort();
                            Err(sanitize_workspace_error(error, &workspace_root))
                        }
                    }
                }
            }
        }
        Command::Online { command } => {
            let explicit_curseforge = match command {
                OnlineCommand::Install { provider, .. }
                | OnlineCommand::PackInstall { provider, .. }
                | OnlineCommand::Search { provider, .. }
                | OnlineCommand::Versions { provider, .. }
                | OnlineCommand::Resolve { provider, .. } => *provider == Provider::CurseForge,
                OnlineCommand::Updates { .. } | OnlineCommand::Analyze { .. } => false,
            };
            let key = if config.downloads.offline {
                None
            } else if explicit_curseforge {
                curseforge_key(&config_path)?
            } else {
                optional_curseforge_key(&config_path)?
            };
            let service = OnlineContentService::from_download_config(&config.downloads, key)?;
            match command {
                OnlineCommand::Install {
                    id,
                    provider,
                    project,
                    version,
                } => {
                    let instances =
                        InstanceService::new(resolution.path.clone(), config.downloads.clone())?;
                    let target = match select_profile_target(&instances, &resolution.path, id) {
                        Ok(target) => Some(target),
                        Err(Error::Instance(ccl_core::instances::InstanceError::NotFound(_))) => {
                            None
                        }
                        Err(error) => return Err(error),
                    };
                    match target {
                        Some(ProfileTarget::Root(profile)) => {
                            let workspace = profile
                                .begin_workspace(config.downloads.clone())
                                .map_err(profile_error)?;
                            let workspace_root = workspace.root().to_path_buf();
                            let installed = service
                                .clone()
                                .with_runtime_subdirectory(
                                    workspace.runtime_subdirectory().to_path_buf(),
                                )
                                .install_project(
                                    workspace_root.clone(),
                                    &config.downloads,
                                    profile.id(),
                                    *provider,
                                    project,
                                    version.as_deref(),
                                )
                                .await;
                            match installed {
                                Ok(installed) => {
                                    let profile = match workspace.commit() {
                                        Ok(profile) => profile,
                                        Err(error) => {
                                            return Err(sanitize_workspace_error(
                                                profile_error(error),
                                                &workspace_root,
                                            ));
                                        }
                                    };
                                    Ok(
                                        json!({"ok":true,"installed_content":installed,"profile":profile}),
                                    )
                                }
                                Err(error) => {
                                    let _ = workspace.abort();
                                    Err(sanitize_workspace_error(error.into(), &workspace_root))
                                }
                            }
                        }
                        Some(ProfileTarget::Managed(id)) => {
                            let installed = service
                                .install_project(
                                    resolution.path,
                                    &config.downloads,
                                    &id,
                                    *provider,
                                    project,
                                    version.as_deref(),
                                )
                                .await?;
                            Ok(json!({"ok":true,"installed_content":installed}))
                        }
                        None => {
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
                    }
                }
                OnlineCommand::Updates { id } => {
                    let instances =
                        InstanceService::new(resolution.path.clone(), config.downloads.clone())?;
                    match select_profile_target(&instances, &resolution.path, id)? {
                        ProfileTarget::Root(profile) => {
                            let workspace = profile
                                .begin_workspace(config.downloads.clone())
                                .map_err(profile_error)?;
                            let workspace_root = workspace.root().to_path_buf();
                            let workspace_service = service.clone().with_runtime_subdirectory(
                                workspace.runtime_subdirectory().to_path_buf(),
                            );
                            let updates = workspace_service
                                .updates_for_instance(
                                    workspace_root.clone(),
                                    &config.downloads,
                                    profile.id(),
                                )
                                .await;
                            let _ = workspace.abort();
                            let updates = updates.map_err(|error| {
                                sanitize_workspace_error(error.into(), &workspace_root)
                            })?;
                            Ok(json!({"ok":true,"updates":updates}))
                        }
                        ProfileTarget::Managed(id) => Ok(
                            json!({"ok":true,"updates":service.updates_for_instance(resolution.path,&config.downloads,&id).await?}),
                        ),
                    }
                }
                OnlineCommand::Analyze { id } => {
                    let instances =
                        InstanceService::new(resolution.path.clone(), config.downloads.clone())?;
                    match select_profile_target(&instances, &resolution.path, id)? {
                        ProfileTarget::Root(profile) => {
                            let workspace = profile
                                .begin_workspace(config.downloads.clone())
                                .map_err(profile_error)?;
                            let workspace_root = workspace.root().to_path_buf();
                            let workspace_service = service.clone().with_runtime_subdirectory(
                                workspace.runtime_subdirectory().to_path_buf(),
                            );
                            let conflicts = workspace_service
                                .analyze_instance(
                                    workspace_root.clone(),
                                    &config.downloads,
                                    profile.id(),
                                )
                                .await;
                            let _ = workspace.abort();
                            let conflicts = conflicts.map_err(|error| {
                                sanitize_workspace_error(error.into(), &workspace_root)
                            })?;
                            Ok(json!({"ok":true,"conflicts":conflicts}))
                        }
                        ProfileTarget::Managed(id) => Ok(
                            json!({"ok":true,"conflicts":service.analyze_instance(resolution.path,&config.downloads,&id).await?}),
                        ),
                    }
                }
                OnlineCommand::PackInstall {
                    id,
                    provider,
                    project,
                    version,
                    include_optional,
                    update_existing,
                } => {
                    let instances =
                        InstanceService::new(resolution.path.clone(), config.downloads.clone())?;
                    let target =
                        match select_profile_target(&instances, &resolution.path, id) {
                            Ok(target) => Some(target),
                            Err(Error::Instance(ccl_core::instances::InstanceError::NotFound(
                                _,
                            ))) if !update_existing => None,
                            Err(error) => return Err(error),
                        };
                    if !update_existing && target.is_some() {
                        return Err(Error::Instance(
                            ccl_core::instances::InstanceError::Conflict(format!(
                                "profile already exists: {id}; choose --update-existing to replace pack-owned files"
                            )),
                        ));
                    }
                    if *update_existing && target.is_none() {
                        return Err(Error::Instance(
                            ccl_core::instances::InstanceError::NotFound(id.clone()),
                        ));
                    }
                    let selected = service
                        .versions(*provider, project, None, None)
                        .await?
                        .into_iter()
                        .find(|v| version.as_ref().is_none_or(|id| v.id == *id))
                        .ok_or_else(|| {
                            Error::InvalidConfig("requested pack version is unavailable".into())
                        })?;
                    let mode = if *update_existing {
                        ccl_core::online_content::PackImportMode::UpdateExisting
                    } else {
                        ccl_core::online_content::PackImportMode::CreateNew
                    };
                    match target {
                        Some(ProfileTarget::Root(profile)) => {
                            let workspace = profile
                                .begin_workspace(config.downloads.clone())
                                .map_err(profile_error)?;
                            let workspace_root = workspace.root().to_path_buf();
                            let has_pack_receipt =
                                profile.game_directory().join(".ccl/modpack.json").is_file();
                            let imported = if *update_existing && !has_pack_receipt {
                                service
                                    .download_and_merge_pack_into_profile(
                                        workspace_root.clone(),
                                        &config.downloads,
                                        profile.id(),
                                        &selected,
                                        PackInstallOptions {
                                            include_optional: *include_optional,
                                            mode,
                                        },
                                        Some(operation_context()),
                                    )
                                    .await
                            } else if has_pack_receipt {
                                service
                                    .clone()
                                    .with_runtime_subdirectory(
                                        workspace.runtime_subdirectory().to_path_buf(),
                                    )
                                    .download_and_import_pack(
                                        workspace_root.clone(),
                                        &config.downloads,
                                        profile.id(),
                                        &selected,
                                        PackInstallOptions {
                                            include_optional: *include_optional,
                                            mode,
                                        },
                                        Some(operation_context()),
                                    )
                                    .await
                            } else {
                                service
                                    .download_and_import_pack(
                                        workspace_root.clone(),
                                        &config.downloads,
                                        profile.id(),
                                        &selected,
                                        PackInstallOptions {
                                            include_optional: *include_optional,
                                            mode,
                                        },
                                        Some(operation_context()),
                                    )
                                    .await
                            };
                            match imported {
                                Ok(_) => {
                                    let profile = match workspace.commit() {
                                        Ok(profile) => profile,
                                        Err(error) => {
                                            return Err(sanitize_workspace_error(
                                                profile_error(error),
                                                &workspace_root,
                                            ));
                                        }
                                    };
                                    Ok(json!({"ok":true,"instance":profile,"pack_imported":true}))
                                }
                                Err(error) => {
                                    let _ = workspace.abort();
                                    Err(sanitize_workspace_error(error.into(), &workspace_root))
                                }
                            }
                        }
                        Some(ProfileTarget::Managed(id)) => {
                            let instance = service
                                .download_and_import_pack(
                                    resolution.path,
                                    &config.downloads,
                                    &id,
                                    &selected,
                                    PackInstallOptions {
                                        include_optional: *include_optional,
                                        mode,
                                    },
                                    Some(operation_context()),
                                )
                                .await?;
                            Ok(json!({"ok":true,"instance":instance,"pack_imported":true}))
                        }
                        None => {
                            let instance = service
                                .download_and_import_pack(
                                    resolution.path,
                                    &config.downloads,
                                    id,
                                    &selected,
                                    PackInstallOptions {
                                        include_optional: *include_optional,
                                        mode,
                                    },
                                    Some(operation_context()),
                                )
                                .await?;
                            Ok(json!({"ok":true,"instance":instance,"pack_imported":true}))
                        }
                    }
                }
                OnlineCommand::Search {
                    provider,
                    query,
                    minecraft,
                    loader,
                    kind,
                    limit,
                } => {
                    let projects = service
                        .search(
                            *provider,
                            &SearchQuery {
                                query: query.clone(),
                                game_version: minecraft.clone(),
                                loaders: loader.clone(),
                                project_type: Some(kind.clone()),
                                limit: *limit,
                            },
                        )
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
            let service = InstanceService::new(resolution.path.clone(), config.downloads.clone())?;
            let (run_id, text) = match instance {
                Some(selector) => {
                    match select_profile_target(&service, &resolution.path, selector)? {
                        ProfileTarget::Root(profile) => read_profile_logs(
                            &[profile.resource_root().to_path_buf()],
                            profile.id(),
                            profile.id(),
                            run.as_deref(),
                            *lines,
                        )?,
                        ProfileTarget::Managed(id) => {
                            let metadata = service.metadata(&id)?;
                            let legacy_version = metadata
                                .loader_version
                                .as_deref()
                                .unwrap_or(&metadata.minecraft);
                            read_profile_logs(
                                &[resolution.path.clone(), metadata.game_directory],
                                &id,
                                legacy_version,
                                run.as_deref(),
                                *lines,
                            )?
                        }
                    }
                }
                None => launch::read_logs(&resolution.path, run.as_deref(), *lines)?,
            };
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
            let instance_service =
                InstanceService::new(resolution.path.clone(), config.downloads.clone())?;
            let mut _profile_guard = None;
            let mut _instance_guard = None;
            let (resource_root, working_directory, version, profile_id) =
                if let Some(selector) = instance {
                    match select_profile_target(&instance_service, &resolution.path, selector)? {
                        ProfileTarget::Root(profile) => {
                            let id = profile.id().to_owned();
                            _profile_guard = Some(profile.guard().map_err(profile_error)?);
                            (
                                profile.resource_root().to_path_buf(),
                                profile.game_directory().to_path_buf(),
                                id.clone(),
                                id,
                            )
                        }
                        ProfileTarget::Managed(id) => {
                            let guard = instance_service.acquire_launch(&id)?;
                            let version = guard.version().to_owned();
                            let directory = guard.game_directory().to_path_buf();
                            _instance_guard = Some(guard);
                            (directory.clone(), directory, version, id)
                        }
                    }
                } else {
                    let id = version.clone().expect("clap requires version or instance");
                    let profile =
                        RootProfile::open(&resolution.path, &id).map_err(|error| match error {
                            ccl_core::profiles::ProfileError::NotFound(id) => {
                                Error::Launch(ccl_core::launch::LaunchError::Invalid(format!(
                                    "installed profile not found: {id}"
                                )))
                            }
                            other => profile_error(other),
                        })?;
                    _profile_guard = Some(profile.guard().map_err(profile_error)?);
                    (
                        profile.resource_root().to_path_buf(),
                        profile.game_directory().to_path_buf(),
                        id.clone(),
                        id,
                    )
                };
            let service = LaunchService::new(config.downloads.clone());
            let token = signal_token();
            let store = account_store(&config_path)?;
            let selected = if *offline || username.is_some() {
                None
            } else {
                account.clone().or(store.selected()?)
            };
            let mut launch_username = username.clone().unwrap_or_else(|| "CCLPlayer".into());
            let mut identity = None;
            if let Some(id) = selected {
                let metadata = store
                    .list()?
                    .into_iter()
                    .find(|a| a.id == id)
                    .ok_or(ccl_core::accounts::AccountError::Invalid)?;
                if metadata.account_type == "offline" {
                    launch_username = metadata.username;
                } else if config.downloads.offline {
                    if account.is_some() {
                        return Err(Error::InvalidConfig("network offline mode cannot refresh a Microsoft account; launch with --offline identity, or disable --network-offline".into()));
                    }
                } else {
                    identity = Some(
                        auth_service(
                            metadata
                                .oauth_client_id
                                .as_deref()
                                .ok_or(ccl_core::accounts::AccountError::Invalid)?,
                            &config,
                        )?
                        .launch_identity(&store, &id, &token)
                        .await?,
                    );
                }
            }
            let memory_policy =
                config
                    .launch
                    .memory_policy(&resource_root, &profile_id, *memory)?;
            let request = LaunchRequest {
                game_directory: resource_root,
                working_directory: Some(working_directory),
                version,
                profile_id: Some(profile_id),
                username: launch_username,
                java: java.clone().or(config.launch.java.clone()),
                memory: memory_policy,
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

fn curseforge_key(config_path: &Path) -> Result<Option<String>> {
    match env::var("CCL_CURSEFORGE_API_KEY") {
        Ok(key) => return Ok(Some(key)),
        Err(env::VarError::NotUnicode(_)) => {
            return Err(Error::InvalidConfig(
                "CCL_CURSEFORGE_API_KEY must contain text".into(),
            ));
        }
        Err(env::VarError::NotPresent) => {}
    }
    Ok(OsProviderKeyStore::for_config_file(config_path)?.get_curseforge()?)
}

fn optional_curseforge_key(config_path: &Path) -> Result<Option<String>> {
    match curseforge_key(config_path) {
        Err(Error::Account(ccl_core::accounts::AccountError::SecureStorage(_))) => Ok(None),
        result => result,
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
    if let Some(effective) = value.get("effective") {
        let describe = |policy: &Value| match policy {
            Value::String(value) if value == "auto" => "automatic".to_owned(),
            Value::Number(value) => format!("{value} MiB"),
            _ => "inherit".to_owned(),
        };
        println!(
            "Memory for {}: {} (instance override: {})",
            value["id"].as_str().unwrap_or("?"),
            describe(effective),
            describe(&value["override"])
        );
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
            LaunchError::Memory(_) => "memory_allocation_failed",
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
