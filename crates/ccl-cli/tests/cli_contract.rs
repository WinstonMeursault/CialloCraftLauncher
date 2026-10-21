use std::{
    fs,
    io::ErrorKind,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn sandbox() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    loop {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("ccl-cli-test-{}-{nonce}-{id}", std::process::id()));
        match fs::create_dir(&root) {
            Ok(()) => return root,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("cannot create test directory: {error}"),
        }
    }
}

fn command(root: &PathBuf) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ccl"));
    command.current_dir(root);
    command
}

#[test]
fn doctor_is_read_only_and_init_creates_selected_directory() {
    let root = sandbox();
    let game = root.join("game files");
    let output = command(&root)
        .args(["doctor", "--json", "--game-dir", "game files"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["game_directory"]["exists"], false);
    let reported = PathBuf::from(response["game_directory"]["path"].as_str().unwrap());
    assert_eq!(reported.file_name().unwrap(), "game files");
    assert_eq!(
        fs::canonicalize(reported.parent().unwrap()).unwrap(),
        fs::canonicalize(&root).unwrap()
    );
    assert!(!game.exists());

    let output = command(&root)
        .args(["init", "--json", "--game-dir", "game files"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["ok"], true);
    assert!(game.is_dir());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_configuration_has_machine_readable_error_and_exit_code() {
    let root = sandbox();
    let config = root.join("invalid.json");
    fs::write(&config, r#"{"downloads":{"max_parallel":0}}"#).unwrap();
    let output = command(&root)
        .args(["doctor", "--json", "--config", "invalid.json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "configuration_error");
    fs::remove_dir_all(root).unwrap();
}
