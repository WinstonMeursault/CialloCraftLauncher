use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
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

fn write_instance(root: &Path, id: &str, major: u32) {
    let directory = root.join("versions").join(id);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join(format!("{id}.json")),
        format!(r#"{{"id":"{id}","type":"release","javaVersion":{{"component":"java-runtime-gamma","majorVersion":{major}}}}}"#),
    ).unwrap();
}

fn json_output(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout was not one JSON document ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn java_on_path() -> Option<PathBuf> {
    let executable = if cfg!(windows) { "java.exe" } else { "java" };
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        let candidate = PathBuf::from(home).join("bin").join(executable);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|path| path.join(executable))
        .find(|path| path.is_file())
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

#[test]
fn instances_reads_valid_local_metadata_without_network() {
    let root = sandbox();
    let game = root.join("game files");
    write_instance(&game, "local-release", 17);
    let output = command(&root)
        .args(["instances", "--json", "--game-dir"])
        .arg(&game)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = json_output(&output);
    assert_eq!(response["ok"], true);
    assert_eq!(response["instances"].as_array().unwrap().len(), 1);
    assert_eq!(response["instances"][0]["id"], "local-release");
    assert_eq!(response["instances"][0]["required_java"], 17);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn logs_support_latest_run_selection_and_line_limit() {
    let root = sandbox();
    let game = root.join("game files");
    let logs = game.join(".ccl/logs");
    fs::create_dir_all(&logs).unwrap();
    fs::write(logs.join("run-a.log"), "first\nsecond\nthird\n").unwrap();
    fs::write(logs.join("run-z.log"), "latest-one\nlatest-two\n").unwrap();

    let latest = command(&root)
        .args(["logs", "--json", "--game-dir"])
        .arg(&game)
        .args(["--lines", "1"])
        .output()
        .unwrap();
    assert!(
        latest.status.success(),
        "{}",
        String::from_utf8_lossy(&latest.stderr)
    );
    let latest_json = json_output(&latest);
    assert_eq!(latest_json["ok"], true);
    assert_eq!(latest_json["run_id"], "run-z");
    assert_eq!(latest_json["log"], "latest-two");

    let selected = command(&root)
        .args(["logs", "--json", "--game-dir"])
        .arg(&game)
        .args(["--run", "run-a", "--lines", "2"])
        .output()
        .unwrap();
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let selected_json = json_output(&selected);
    assert_eq!(selected_json["run_id"], "run-a");
    assert_eq!(selected_json["log"], "second\nthird");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_offline_username_fails_with_json_and_launch_exit_code() {
    let root = sandbox();
    let game = root.join("game");
    write_instance(&game, "local", 17);
    let output = command(&root)
        .args(["launch", "local", "--json", "--game-dir"])
        .arg(&game)
        .args(["--username", "not a player", "--java"])
        .arg(root.join("java does not exist"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(8));
    let response = json_output(&output);
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "launch_error");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("username")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_explicit_java_has_java_diagnostic_and_exit_code() {
    let root = sandbox();
    let game = root.join("game");
    write_instance(&game, "local", 17);
    let output = command(&root)
        .args(["launch", "local", "--json", "--game-dir"])
        .arg(&game)
        .args(["--java"])
        .arg(root.join("missing java path"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(9));
    let response = json_output(&output);
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "java_error");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not found")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_java_with_wrong_major_has_java_diagnostic_and_exit_code() {
    let Some(java) = java_on_path() else { return };
    let root = sandbox();
    let game = root.join("game");
    write_instance(&game, "local", 999);
    let output = command(&root)
        .args(["launch", "local", "--json", "--game-dir"])
        .arg(&game)
        .args(["--java"])
        .arg(java)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(9));
    let response = json_output(&output);
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "java_error");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("required")
    );
    fs::remove_dir_all(root).unwrap();
}
