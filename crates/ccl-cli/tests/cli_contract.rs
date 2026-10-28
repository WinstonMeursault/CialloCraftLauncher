use sha2::{Digest, Sha256};
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

#[test]
fn curseforge_key_command_never_accepts_a_key_argument_or_echoes_input() {
    let root = sandbox();
    let game = root.join("game");
    let output = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args([
            "credentials",
            "curseforge",
            "set",
            "SECRET_SHOULD_NOT_APPEAR",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("SECRET_SHOULD_NOT_APPEAR"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("SECRET_SHOULD_NOT_APPEAR"));
    assert!(!game.exists());
    fs::remove_dir_all(root).unwrap();
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

// Keep the Java error contract independent of a runner's JVM startup time.
fn java_fixture(root: &Path) -> PathBuf {
    let source = root.join("java_fixture.rs");
    let executable = root.join(if cfg!(windows) { "java.exe" } else { "java" });
    fs::write(
        &source,
        format!(
            r#"fn main() {{ eprintln!("    java.version = 17.0.1\n    java.specification.version = 17\n    os.arch = {}"); }}"#,
            std::env::consts::ARCH
        ),
    )
    .unwrap();
    let output = Command::new("rustc")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    executable
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
fn launch_uses_saved_offline_account_by_default_and_with_network_offline() {
    let root = sandbox();
    let game = root.join("game");
    write_instance(&game, "1.21.1", 21);
    fs::create_dir_all(&game).unwrap();
    let config = root.join("config.json");
    fs::write(&config, "{}").unwrap();
    let id = "offline-playerfixture";
    fs::write(
        root.join("accounts.json"),
        serde_json::json!({
            "accounts": [{
                "id": id,
                "username": "PlayerFixture",
                "uuid": "01234567-89ab-3cde-8fab-0123456789ab",
                "xuid": null,
                "account_type": "offline",
                "oauth_client_id": null,
                "last_used_unix": null
            }],
            "selected": id
        })
        .to_string(),
    )
    .unwrap();

    for explicit in [false, true] {
        let mut command = command(&root);
        command
            .args(["--json", "--network-offline", "--game-dir"])
            .arg(&game)
            .args(["--config"])
            .arg(&config)
            .args(["launch", "1.21.1", "--dry-run"]);
        if explicit {
            command.args(["--account", id]);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        let result = json_output(&output);
        assert_ne!(result["error"]["code"], "account_error", "{result}");
        assert_ne!(result["error"]["code"], "configuration_error", "{result}");
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn instance_logs_can_read_legacy_runtime_log_for_instance() {
    let root = sandbox();
    let game = root.join("game");
    let instance = game.join(".ccl/instances/test");
    let isolated = instance.join("game");
    fs::create_dir_all(&isolated).unwrap();
    fs::write(
        instance.join("instance.json"),
        serde_json::json!({
            "id":"test", "minecraft":"1.21.1", "loader":null,
            "loader_version":null, "game_directory": fs::canonicalize(&isolated).unwrap(),
        })
        .to_string(),
    )
    .unwrap();
    fs::create_dir_all(isolated.join(".ccl/logs")).unwrap();
    fs::write(isolated.join(".ccl/logs/run1.log"), "first\nlast\n").unwrap();
    fs::write(
        isolated.join(".ccl/logs/run1.json"),
        serde_json::json!({
            "run_id":"run1", "version":"1.21.1",
            "pid":1, "exit_code":0, "cancelled":false,
            "log_path":isolated.join(".ccl/logs/run1.log"),
            "started_at_ms":1, "finished_at_ms":2,
        })
        .to_string(),
    )
    .unwrap();
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
fn root_profile_logs_only_show_runs_owned_by_selected_profile() {
    let root = sandbox();
    let game = root.join("game");
    write_instance(&game, "1.20.1", 17);
    let logs = game.join(".ccl/logs");
    fs::create_dir_all(&logs).unwrap();
    for (run_id, owner, body, started) in [
        ("run-own", "1.20.1", "this profile", 1),
        ("run-other", "another-profile", "other profile", 2),
    ] {
        fs::write(logs.join(format!("{run_id}.log")), body).unwrap();
        fs::write(
            logs.join(format!("{run_id}.json")),
            serde_json::json!({
                "run_id":run_id, "version":"1.20.1", "profile_id":owner,
                "pid":1, "exit_code":0, "cancelled":false,
                "log_path":logs.join(format!("{run_id}.log")),
                "started_at_ms":started, "finished_at_ms":started,
            })
            .to_string(),
        )
        .unwrap();
    }
    let latest = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["logs", "--instance", "version:1.20.1"])
        .output()
        .unwrap();
    assert!(latest.status.success());
    assert_eq!(json_output(&latest)["run_id"], "run-own");
    assert_eq!(json_output(&latest)["log"], "this profile");

    let rejected = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["logs", "--instance", "version:1.20.1", "--run", "run-other"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert_eq!(
        json_output(&rejected)["error"]["code"],
        "configuration_error"
    );
    assert!(String::from_utf8_lossy(&rejected.stdout).contains("does not belong to this profile"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn diagnose_rejects_a_run_owned_by_another_root_profile() {
    let root = sandbox();
    let game = root.join("game");
    write_instance(&game, "1.20.1", 17);
    let logs = game.join(".ccl/logs");
    fs::create_dir_all(&logs).unwrap();
    fs::write(logs.join("run-other.log"), "other profile log").unwrap();
    fs::write(
        logs.join("run-other.json"),
        serde_json::json!({
            "run_id":"run-other", "version":"1.20.1", "profile_id":"other-profile",
            "pid":1, "exit_code":1, "cancelled":false,
            "log_path":logs.join("run-other.log"),
            "started_at_ms":1, "finished_at_ms":2,
        })
        .to_string(),
    )
    .unwrap();

    let output = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args([
            "diagnose",
            "--instance",
            "version:1.20.1",
            "--run",
            "run-other",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(json_output(&output)["error"]["code"], "configuration_error");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn online_pack_create_new_rejects_existing_root_and_managed_targets_before_network() {
    let root = sandbox();
    let game = root.join("game");
    write_instance(&game, "1.20.1", 17);
    write_managed_instance(&game, "managed-existing");

    for target in ["version:1.20.1", "instance:managed-existing"] {
        let output = command(&root)
            .args(["--json", "--network-offline", "--game-dir"])
            .arg(&game)
            .args(["online", "pack-install", target, "modrinth", "example-pack"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(json_output(&output)["error"]["code"], "instance_conflict");
    }
    assert!(game.join("versions/1.20.1/1.20.1.json").is_file());
    assert!(
        game.join(".ccl/instances/managed-existing/instance.json")
            .is_file()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_launch_resolves_versions_from_the_managed_resource_root() {
    let root = sandbox();
    let game = root.join("game");
    let managed_game = write_managed_instance(&game, "managed");
    let version_dir = managed_game.join("versions/1.21.1");
    fs::create_dir_all(&version_dir).unwrap();
    fs::write(
        version_dir.join("1.21.1.json"),
        r#"{"id":"1.21.1","type":"release","javaVersion":{"component":"java-runtime-gamma","majorVersion":17},"mainClass":"Main","downloads":{"client":{"url":"https://fixture.invalid/client.jar","sha1":"da39a3ee5e6b4b0d3255bfef95601890afd80709","size":0}},"arguments":{"game":[]}}"#,
    )
    .unwrap();
    let java = java_fixture(&root);

    let output = command(&root)
        .args(["--json", "--network-offline", "--game-dir"])
        .arg(&game)
        .args([
            "launch",
            "--instance",
            "instance:managed",
            "--offline",
            "--dry-run",
            "--no-java-download",
            "--java",
        ])
        .arg(java)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let message = json_output(&output)["error"]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(message.contains(".ccl/instances/managed/game"), "{message}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_memory_setting_uses_the_same_profile_key_as_launch() {
    let root = sandbox();
    let game = root.join("game");
    let managed_game = write_managed_instance(&game, "managed");
    let config = root.join("config.json");
    fs::write(&config, "{}").unwrap();
    let mut digest = Sha256::new();
    digest.update(
        fs::canonicalize(&managed_game)
            .unwrap()
            .to_string_lossy()
            .as_bytes(),
    );
    digest.update([0]);
    digest.update(b"managed");
    let memory_key = format!("memory:{:x}", digest.finalize());

    let output = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["--config"])
        .arg(&config)
        .args([
            "settings",
            "memory",
            "--instance",
            "instance:managed",
            "--set",
            "1024",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value = json_output(&output);
    assert_eq!(value["override"], serde_json::json!(1024));
    let persisted: serde_json::Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    assert_eq!(
        persisted["launch"]["instance_memory"][memory_key],
        serde_json::json!(1024)
    );
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
    let root = sandbox();
    let java = java_fixture(&root);
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
        .args(["account", "login", "--no-browser"])
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

#[test]
fn instance_lifecycle_cli_requires_delete_confirmation_and_supports_preview_cleanup() {
    let root = sandbox();
    let game = write_managed_instance(&root, "source");
    fs::write(game.join("options.txt"), "music:0").unwrap();
    for args in [
        vec!["instance", "copy", "source", "copy"],
        vec!["instance", "rename", "copy", "renamed"],
        vec!["instance", "history", "renamed"],
        vec!["instance", "disk-usage", "renamed"],
        vec!["instance", "clean", "renamed", "--keep", "0"],
    ] {
        let output = command(&root)
            .arg("--json")
            .arg("--game-dir")
            .arg(&root)
            .args(&args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["ok"], true);
    }
    let refused = command(&root)
        .arg("--game-dir")
        .arg(&root)
        .args(["instance", "delete", "renamed"])
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(2));
    assert!(
        root.join(".ccl/instances/renamed/game/options.txt")
            .is_file()
    );
    let removed = command(&root)
        .arg("--json")
        .arg("--game-dir")
        .arg(&root)
        .args(["instance", "delete", "renamed", "--yes"])
        .output()
        .unwrap();
    assert!(removed.status.success());
    assert!(!root.join(".ccl/instances/renamed").exists());
    assert!(game.join("options.txt").is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn root_profiles_and_managed_instances_use_explicit_selectors_when_ids_overlap() {
    let root = sandbox();
    let game = root.join("game");
    write_instance(&game, "same-id", 17);
    write_managed_instance(&game, "same-id");

    let ambiguous = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["instance", "show", "same-id"])
        .output()
        .unwrap();
    assert!(!ambiguous.status.success());
    assert_eq!(
        json_output(&ambiguous)["error"]["code"],
        "configuration_error"
    );

    for (selector, kind) in [
        ("version:same-id", "root_profile"),
        ("instance:same-id", "managed_instance"),
    ] {
        let output = command(&root)
            .args(["--json", "--game-dir"])
            .arg(&game)
            .args(["instance", "show", selector])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(json_output(&output)["kind"], kind);
    }

    let listing = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["instance", "list"])
        .output()
        .unwrap();
    assert!(listing.status.success());
    let instances = &json_output(&listing)["instances"];
    assert_eq!(instances.as_array().unwrap().len(), 2);
    assert!(
        instances
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["selector"] == "version:same-id")
    );
    assert!(
        instances
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["selector"] == "instance:same-id")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn root_profile_content_edit_publishes_without_workspace_paths() {
    let root = sandbox();
    let game = root.join("game");
    write_instance(&game, "vanilla", 17);
    let source = root.join("fixture.jar");
    fs::write(&source, b"fixture mod bytes").unwrap();

    let added = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["content", "add", "version:vanilla", "mods"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stdout)
    );
    let added_json = json_output(&added);
    assert_eq!(added_json["entry"]["path"], "fixture.jar");
    assert!(
        !String::from_utf8_lossy(&added.stdout).contains(
            &game
                .join(".ccl/profile-workspaces")
                .to_string_lossy()
                .to_string()
        )
    );
    assert_eq!(
        fs::read(game.join("mods/fixture.jar")).unwrap(),
        b"fixture mod bytes"
    );

    let history = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["instance", "history", "version:vanilla"])
        .output()
        .unwrap();
    assert!(
        history.status.success(),
        "{}",
        String::from_utf8_lossy(&history.stdout)
    );
    let revision = json_output(&history)["history"][0]["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    let restored = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["instance", "restore", "version:vanilla", &revision])
        .output()
        .unwrap();
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stdout)
    );
    assert!(!game.join("mods/fixture.jar").exists());

    let rolled_back = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["instance", "rollback", "version:vanilla"])
        .output()
        .unwrap();
    assert!(
        rolled_back.status.success(),
        "{}",
        String::from_utf8_lossy(&rolled_back.stdout)
    );
    assert!(game.join("mods/fixture.jar").is_file());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn root_profile_copy_and_delete_preserve_the_independent_copy() {
    let root = sandbox();
    let game = root.join("game");
    write_instance(&game, "1.20.1", 17);
    fs::create_dir_all(game.join("mods")).unwrap();
    fs::write(game.join("mods/fixture.jar"), b"fixture mod bytes").unwrap();

    let copied = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["instance", "copy", "version:1.20.1", "vanilla-copy"])
        .output()
        .unwrap();
    assert!(
        copied.status.success(),
        "{}",
        String::from_utf8_lossy(&copied.stdout)
    );
    let instance = json_output(&copied)["instance"].clone();
    assert_eq!(instance["id"], "vanilla-copy");
    let copied_game = PathBuf::from(instance["game_directory"].as_str().unwrap());
    let launch_version = instance["loader_version"]
        .as_str()
        .unwrap_or_else(|| instance["minecraft"].as_str().unwrap());
    assert!(
        copied_game
            .join("versions")
            .join(launch_version)
            .join(format!("{launch_version}.json"))
            .is_file()
    );
    assert!(
        game.join(".ccl/instances/vanilla-copy/game/mods/fixture.jar")
            .is_file()
    );

    let deleted = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["instance", "delete", "version:1.20.1", "--yes"])
        .output()
        .unwrap();
    assert!(
        deleted.status.success(),
        "{}",
        String::from_utf8_lossy(&deleted.stdout)
    );
    assert!(!game.join("versions/1.20.1").exists());
    ccl_core::launch::load_installed_metadata(&copied_game, launch_version).unwrap();
    assert!(
        game.join(".ccl/instances/vanilla-copy/game/mods/fixture.jar")
            .is_file()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn root_profile_pack_export_uses_workspace_without_exposing_workspace_path() {
    let root = sandbox();
    let game = root.join("game");
    write_instance(&game, "1.20.1", 17);
    let archive = root.join("root-profile.mrpack");

    let output = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["pack", "export", "version:1.20.1"])
        .arg(&archive)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(archive.is_file());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("profile-workspaces"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn root_profile_pack_show_uses_workspace_and_explicit_missing_targets_do_not_create() {
    let root = sandbox();
    let game = root.join("game");
    write_instance(&game, "1.20.1", 17);

    let shown = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["pack", "show", "version:1.20.1"])
        .output()
        .unwrap();
    assert!(!shown.status.success());
    assert_eq!(json_output(&shown)["error"]["code"], "pack_conflict");
    assert!(!String::from_utf8_lossy(&shown.stdout).contains("profile-workspaces"));

    let missing = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args([
            "pack",
            "import",
            "version:missing-profile",
            "does-not-exist.mrpack",
        ])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert_eq!(missing.status.code(), Some(12));
    assert_eq!(json_output(&missing)["error"]["code"], "instance_error");
    assert!(
        json_output(&missing)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("missing-profile")
    );
    assert!(!game.join(".ccl/instances/missing-profile").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn root_profile_workspace_and_copy_keep_custom_hmcl_game_directory_separate() {
    let root = sandbox();
    let game = root.join("game");
    write_instance(&game, "1.20.1", 17);
    let version_dir = game.join("versions/1.20.1");
    fs::write(
        version_dir.join("hmclversion.cfg"),
        r#"{"usesGlobal":false,"gameDirType":"CUSTOM","gameDir":"custom-game"}"#,
    )
    .unwrap();
    fs::create_dir_all(game.join("custom-game")).unwrap();
    fs::write(game.join("custom-game/options.txt"), "before").unwrap();
    let source = root.join("custom-mod.jar");
    fs::write(&source, b"custom runtime mod").unwrap();

    let added = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["content", "add", "version:1.20.1", "mods"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stdout)
    );
    assert!(game.join("custom-game/mods/custom-mod.jar").is_file());
    assert!(!game.join("mods/custom-mod.jar").exists());

    let copied = command(&root)
        .args(["--json", "--game-dir"])
        .arg(&game)
        .args(["instance", "copy", "version:1.20.1", "custom-copy"])
        .output()
        .unwrap();
    assert!(
        copied.status.success(),
        "{}",
        String::from_utf8_lossy(&copied.stdout)
    );
    let metadata = &json_output(&copied)["instance"];
    let runtime = game.join(".ccl/instances/custom-copy/game");
    assert_eq!(
        metadata["game_directory"].as_str(),
        Some(fs::canonicalize(&runtime).unwrap().to_str().unwrap())
    );
    assert!(runtime.join("mods/custom-mod.jar").is_file());
    let selected_version = metadata["loader_version"]
        .as_str()
        .or_else(|| metadata["minecraft"].as_str())
        .unwrap();
    assert!(
        runtime
            .join("versions")
            .join(selected_version)
            .join(format!("{selected_version}.json"))
            .is_file(),
        "managed copy does not contain its selected launch version"
    );
    assert!(!String::from_utf8_lossy(&copied.stdout).contains("profile-workspaces"));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn network_offline_is_distinct_from_player_identity_and_blocks_online_catalog() {
    let root = sandbox();
    let output = command(&root)
        .arg("--json")
        .arg("--game-dir")
        .arg(&root)
        .args([
            "--network-offline",
            "online",
            "search",
            "modrinth",
            "example",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(15));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("offline")
    );
    assert!(!root.join("versions").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn memory_overrides_are_shared_persistent_and_default_to_auto() {
    let root = sandbox();
    let game = root.join("games");
    fs::create_dir(&game).unwrap();
    let config = root.join("config.json");
    fs::write(&config, "{}").unwrap();
    let id = "测试实例 with spaces";
    write_instance(&game, id, 21);
    let run = |args: &[&str]| {
        command(&root)
            .arg("--json")
            .arg("--config")
            .arg(&config)
            .arg("--game-dir")
            .arg(&game)
            .args(args)
            .output()
            .unwrap()
    };
    let output = run(&["settings", "show"]);
    assert_eq!(
        json_output(&output)["settings"]["launch"]["auto_memory"],
        true
    );
    for (value, expected) in [
        ("4096", serde_json::json!(4096)),
        ("auto", serde_json::json!("auto")),
        ("inherit", serde_json::Value::Null),
    ] {
        let output = run(&["settings", "memory", id, "--set", value]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(json_output(&output)["override"], expected);
    }
    assert_eq!(
        json_output(&run(&["settings", "memory", id]))["effective"],
        "auto"
    );
    let human = command(&root)
        .arg("--config")
        .arg(&config)
        .arg("--game-dir")
        .arg(&game)
        .args(["settings", "memory", id])
        .output()
        .unwrap();
    assert!(human.status.success());
    assert!(
        String::from_utf8_lossy(&human.stdout).contains("automatic (instance override: inherit)")
    );
    let output = run(&["launch", id, "--offline", "--memory", "255", "--dry-run"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("memory"));
    let output = run(&["launch", "--help"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("auto"));
    fs::remove_dir_all(root).unwrap();
}
