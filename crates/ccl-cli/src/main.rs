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
    config::{LauncherConfig, default_config_path},
    directory::DirectoryInputs,
    install::{InstallError, InstallOptions, InstallService},
    network::HttpClient,
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
    Doctor,
    /// Create the selected game directory if it is missing.
    Init,
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
}

#[tokio::main]
async fn main() -> ExitCode {
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
            ExitCode::SUCCESS
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
    let config = if cli.config.is_some() {
        LauncherConfig::read(&config_path)?
    } else {
        LauncherConfig::read_or_default(&config_path)?
    };
    let _transport = HttpClient::new(&config.downloads)?;

    match &cli.command {
        Command::Doctor => Ok(json!({
            "ok": true,
            "platform": { "os": env::consts::OS, "architecture": env::consts::ARCH },
            "game_directory": resolution,
            "config": { "path": config_path, "exists": config_path.is_file() },
            "downloads": {
                "version_list_source": config.downloads.version_list_source,
                "file_source": config.downloads.file_source,
                "mirror_root": config.downloads.mirror_root,
                "http_proxy_configured": config.downloads.http_proxy.is_some(),
                "max_parallel": config.downloads.max_parallel,
            }
        })),
        Command::Init => {
            resolution.ensure_exists()?;
            Ok(
                json!({ "ok": true, "game_directory": { "path": resolution.path, "origin": resolution.origin, "exists": true } }),
            )
        }
        Command::Versions { limit } => {
            let manifest = InstallService::new(config.downloads)?
                .list_versions()
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
    }
}

fn print_human(value: &Value) {
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
        Error::InvalidConfig(_) | Error::ConfigJson { .. } => "configuration_error",
        Error::Io { .. } | Error::NotDirectory(_) => "filesystem_error",
        Error::Network(_) => "network_error",
        Error::Cancelled => "cancelled",
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
        Error::InvalidConfig(_) | Error::ConfigJson { .. } => 3,
        Error::Io { .. } | Error::NotDirectory(_) => 4,
        Error::Network(_) => 5,
        Error::Cancelled => 130,
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
