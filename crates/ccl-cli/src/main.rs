use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};

use ccl_core::{
    Error, Result,
    config::{LauncherConfig, default_config_path},
    directory::DirectoryInputs,
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
    match run(&cli) {
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

fn run(cli: &Cli) -> Result<Value> {
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

    match cli.command {
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
    }
}

fn print_human(value: &Value) {
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
    }
}

fn exit_code(error: &Error) -> u8 {
    match error {
        Error::InvalidConfig(_) | Error::ConfigJson { .. } => 3,
        Error::Io { .. } | Error::NotDirectory(_) => 4,
        Error::Network(_) => 5,
        Error::Cancelled => 130,
    }
}
