//! Process acceptance driver; excluded from normal desktop release builds.
use ccl_gui::self_update::{GuiManifest, Installation, helper_entry};
use std::{fs, path::PathBuf};

fn main() {
    if let Some(code) = helper_entry() {
        std::process::exit(code);
    }
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() < 4 {
        return Err("Usage: gui-update-driver <stage|stage-only|rollback|recover> <target> <state> [manifest signature package]".into());
    }
    let operation = args[1].to_string_lossy();
    let install = Installation::validation_at(PathBuf::from(&args[2]), PathBuf::from(&args[3]))?;
    let _guard = install.process_guard()?;
    match operation.as_ref() {
        "stage" | "stage-only" => {
            if args.len() != 7 {
                return Err("stage requires signed manifest, signature and package".into());
            }
            let raw = fs::read(&args[4])?;
            let manifest: GuiManifest = serde_json::from_slice(&raw)?;
            let signature = fs::read_to_string(&args[5])?;
            let package = fs::read(&args[6])?;
            install.stage(&manifest, &raw, &signature, &package)?;
            if operation == "stage" {
                println!("helper_pid={}", install.launch("apply")?);
            }
        }
        "rollback" | "recover" => {
            install.prepare(&operation)?;
            println!("helper_pid={}", install.launch(&operation)?);
        }
        _ => return Err("Unknown acceptance operation".into()),
    }
    Ok(())
}
