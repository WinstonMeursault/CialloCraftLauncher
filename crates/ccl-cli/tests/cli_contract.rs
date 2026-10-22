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

fn write_managed_instance(root: &Path, id: &str) -> PathBuf {
    let instance = root.join(".ccl/instances").join(id);
    let game = instance.join("game");
    fs::create_dir_all(&game).unwrap();
    fs::write(
        instance.join("instance.json"),
        serde_json::json!({
            "id": id,
            "minecraft": "1.21.1",
            "loader": null,
            "loader_version": null,
            "game_directory": fs::canonicalize(&game).unwrap(),
        })
        .to_string(),
    )
    .unwrap();
    game
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
fn isolated_instance_list_is_read_only_and_rejects_unsafe_ids() {
    let root = sandbox();
    let game = root.join("missing game");
    let output = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["instance", "list"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        json_output(&output)["managed_instances"],
        serde_json::json!([])
    );
    assert!(!game.exists());
    let output = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["instance", "show", "../outside"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(12));
    assert_eq!(json_output(&output)["error"]["code"], "instance_error");
    assert!(!game.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn launch_requires_exactly_one_version_or_instance() {
    let root = sandbox();
    for args in [
        vec!["launch", "--offline"],
        vec!["launch", "1.21.1", "--instance", "test"],
    ] {
        let output = command(&root).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
    let output = command(&root)
        .args(["loaders", "unknown", "1.21.1"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn instance_logs_select_the_isolated_game_directory() {
    let root = sandbox();
    let game = root.join("game");
    let instance = game.join(".ccl/instances/test");
    let isolated = instance.join("game");
    fs::create_dir_all(isolated.join(".ccl/logs")).unwrap();
    let isolated = fs::canonicalize(isolated).unwrap();
    fs::write(
        instance.join("instance.json"),
        serde_json::json!({
            "id":"test", "minecraft":"1.21.1", "loader":null,
            "loader_version":null, "game_directory": isolated,
        })
        .to_string(),
    )
    .unwrap();
    fs::write(isolated.join(".ccl/logs/run1.log"), "first\nlast\n").unwrap();
    let output = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args([
            "logs",
            "--instance",
            "test",
            "--run",
            "run1",
            "--lines",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(json_output(&output)["log"], "last");
    assert_eq!(json_output(&output)["run_id"], "run1");
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

#[test]
fn settings_persist_validate_and_reset_across_processes() {
    let root = sandbox();
    let config = root.join("new directory/config.json");
    let run = |args: &[&str]| {
        command(&root)
            .args(["--json", "--config"])
            .arg(&config)
            .args(args)
            .output()
            .unwrap()
    };
    let set = run(&["settings", "set", "launch.max_memory_mb", "1024"]);
    assert!(
        set.status.success(),
        "{}",
        String::from_utf8_lossy(&set.stdout)
    );
    assert_eq!(json_output(&set)["value"], 1024);
    let before = fs::read(&config).unwrap();
    let invalid = run(&["settings", "set", "downloads.max_parallel", "0"]);
    assert_eq!(invalid.status.code(), Some(3));
    assert_eq!(fs::read(&config).unwrap(), before);
    let get = run(&["settings", "get", "launch.max_memory_mb"]);
    assert_eq!(json_output(&get)["value"], 1024);
    let proxy = run(&[
        "settings",
        "set",
        "downloads.http_proxy",
        "http://proxy-user:proxy-secret@localhost:8080",
    ]);
    assert!(proxy.status.success());
    for args in [
        vec!["settings", "show"],
        vec!["settings", "get", "downloads.http_proxy"],
    ] {
        let output = run(&args);
        assert!(output.status.success());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("proxy-secret"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("proxy-secret"));
    }
    assert!(
        run(&["settings", "reset", "launch.max_memory_mb"])
            .status
            .success()
    );
    assert_eq!(
        json_output(&run(&["settings", "get", "launch.max_memory_mb"]))["value"],
        2048
    );
    assert!(run(&["settings", "reset", "--all"]).status.success());
    assert_eq!(
        json_output(&run(&["settings", "get", "downloads.http_proxy"]))["value"],
        serde_json::Value::Null
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn login_without_client_id_gives_registration_guidance_without_network() {
    let root = sandbox();
    let config = root.join("config.json");
    fs::write(&config, "{}").unwrap();
    let output = command(&root)
        .args(["--json", "--config"])
        .arg(&config)
        .args(["account", "login"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let value = json_output(&output);
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("client_id")
    );
    assert!(!root.join("accounts.json").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn account_selection_survives_restart_and_logout_requires_secure_store() {
    let root = sandbox();
    let config = root.join("config.json");
    fs::write(&config, "{}").unwrap();
    let ids = [
        "0123456789abcdef0123456789abcdef",
        "1123456789abcdef0123456789abcdef",
    ];
    fs::write(root.join("accounts.json"), serde_json::to_vec(&serde_json::json!({"selected":ids[0],"accounts":ids.iter().enumerate().map(|(i,id)|serde_json::json!({"id":id,"uuid":id,"username":format!("Player{i}"),"xuid":null,"account_type":"microsoft","last_used_unix":null,"oauth_client_id":"00000000-0000-0000-0000-000000000001"})).collect::<Vec<_>>()})).unwrap()).unwrap();
    let run = |args: &[&str]| {
        command(&root)
            .args(["--json", "--config"])
            .arg(&config)
            .args(args)
            .output()
            .unwrap()
    };
    let select = run(&["account", "select", ids[1]]);
    assert!(
        select.status.success(),
        "{}",
        String::from_utf8_lossy(&select.stdout)
    );
    let list = run(&["account", "list"]);
    assert!(list.status.success());
    assert_eq!(json_output(&list)["selected"], ids[1]);
    let missing = run(&["account", "select", "missing"]);
    assert_eq!(missing.status.code(), Some(10));
    assert_eq!(json_output(&run(&["account", "list"]))["selected"], ids[1]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reset_all_recovers_a_corrupt_configuration() {
    let root = sandbox();
    let config = root.join("config.json");
    fs::write(&config, "not valid JSON").unwrap();
    let output = command(&root)
        .args(["--json", "--config"])
        .arg(&config)
        .args(["settings", "reset", "--all"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        json_output(&output)["settings"]["launch"]["max_memory_mb"],
        2048
    );
    assert!(serde_json::from_slice::<serde_json::Value>(&fs::read(&config).unwrap()).is_ok());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_mrpack_inspection_returns_one_json_error() {
    let root = sandbox();
    let archive = root.join("broken.mrpack");
    fs::write(&archive, b"not a zip archive").unwrap();
    let output = command(&root)
        .args(["--json", "pack", "inspect"])
        .arg(&archive)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(13));
    let value = json_output(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "modpack_error");
    assert!(output.stderr.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_pack_import_fails_preflight_before_creating_instance() {
    let root = sandbox();
    let game = root.join("game");
    let archive = root.join("broken.mrpack");
    fs::write(&archive, b"not a zip archive").unwrap();
    let output = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["pack", "import", "new-pack"])
        .arg(&archive)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(13));
    assert_eq!(json_output(&output)["error"]["code"], "modpack_error");
    assert!(!game.join(".ccl/instances/new-pack").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_list_does_not_create_missing_collection() {
    let root = sandbox();
    let game = root.join("game");
    let instance_game = write_managed_instance(&game, "read-only");
    let output = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["content", "list", "read-only", "mods"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value = json_output(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["content"], serde_json::json!([]));
    assert!(!instance_game.join("mods").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn modpack_conflict_has_stable_json_error_and_exit_code() {
    let root = sandbox();
    let game = root.join("game");
    write_managed_instance(&game, "ordinary");
    let archive = root.join("valid.mrpack");
    let manifest = ccl_core::modpack_format::Manifest {
        format_version: 1,
        game: "minecraft".into(),
        version_id: "v1".into(),
        name: "Pack".into(),
        summary: None,
        files: vec![],
        dependencies: [("minecraft".into(), "1.21.1".into())].into(),
    };
    ccl_core::modpack_format::write_pack(&archive, &manifest, &Default::default()).unwrap();
    let output = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["pack", "import", "ordinary"])
        .arg(&archive)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    let value = json_output(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "pack_conflict");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not a managed modpack")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pack_import_parser_exposes_safety_controls_and_content_kind_is_checked() {
    let root = sandbox();
    let output = command(&root)
        .args(["pack", "import", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--include-optional"));
    assert!(help.contains("--replace-modified"));
    assert!(help.contains("--allow-host"));
    let invalid = command(&root)
        .args(["content", "list", "some-instance", "unknown"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("expected mods"));
    fs::remove_dir_all(root).unwrap();
}
