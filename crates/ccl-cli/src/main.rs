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
    launch::{self, LaunchError, LaunchRequest, LaunchService},
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
    /// List versions already installed in the selected game directory.
    Instances,
    /// Launch an installed vanilla version with an offline account.
    Launch {
        version: String,
        #[arg(long, default_value = "CCLPlayer")]
        username: String,
        /// Java executable or Java home; otherwise detect a matching installed runtime.
        #[arg(long)]
        java: Option<PathBuf>,
        /// Maximum Java heap in MiB.
        #[arg(long, default_value_t = 2048)]
        memory: u32,
        /// Prepare and print the argument vector without starting the game.
        #[arg(long)]
        dry_run: bool,
    },
    /// Read the latest launch log, or select a run ID.
    Logs {
        #[arg(long)]
        run: Option<String>,
        #[arg(long, default_value_t = 100)]
        lines: usize,
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
        Command::Instances => Ok(
            json!({ "ok": true, "game_directory": resolution, "instances": launch::list_installed_versions(&resolution.path)? }),
        ),
        Command::Logs { run, lines } => {
            let (run_id, text) = launch::read_logs(&resolution.path, run.as_deref(), *lines)?;
            Ok(json!({ "ok": true, "run_id": run_id, "log": text }))
        }
        Command::Launch {
            version,
            username,
            java,
            memory,
            dry_run,
        } => {
            let service = LaunchService::new(config.downloads);
            let request = LaunchRequest {
                game_directory: resolution.path,
                version: version.clone(),
                username: username.clone(),
                java: java.clone(),
                max_memory_mb: *memory,
            };
            let token = CancellationToken::default();
            let signal = token.clone();
            tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    signal.cancel();
                }
            });
            let human = !cli.json;
            let context = TaskContext::new(
                token,
                Arc::new(move |event| match event {
                    TaskEvent::Log { line, .. } if human => eprintln!("{line}"),
                    TaskEvent::ProcessStarted { pid } => eprintln!("Game process: {pid}"),
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

fn print_human(value: &Value) {
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
        Error::InvalidConfig(_) | Error::ConfigJson { .. } => "configuration_error",
        Error::Io { .. } | Error::NotDirectory(_) => "filesystem_error",
        Error::Network(_) => "network_error",
        Error::Cancelled => "cancelled",
        Error::Launch(error) => match error {
            LaunchError::Java(_) => "java_error",
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
        Error::InvalidConfig(_) | Error::ConfigJson { .. } => 3,
        Error::Io { .. } | Error::NotDirectory(_) => 4,
        Error::Network(_) => 5,
        Error::Cancelled => 130,
        Error::Launch(error) => match error {
            LaunchError::Java(_) => 9,
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
