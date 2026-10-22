use ccl_core::compatibility::{CompatibilityStatus, inspect_compatibility};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn temp(label: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "ccl-upgrade-compat-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&p).unwrap();
    p
}
fn write_instance_metadata(instance: &Path) {
    let value = serde_json::json!({
        "id":"fixture", "minecraft":"1.21.1", "loader":null,
        "loader_version":null, "game_directory":instance.join("game")
    });
    fs::write(
        instance.join("instance.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}

// config.json and accounts.json below are literal outputs from the old 2edbf0e CLI and
// AccountStore in `.local/phases/p7/baseline`; the reproducible generator uses a separate
// git-archived source tree and a no-op in-memory secret store. The instance path is sanitized.
// The other receipt shapes exercise the old reader/writer-compatible fields. No secrets exist.
#[test]
fn baseline_2edbf0e_data_is_readable_without_migration() {
    let root = temp("legacy");
    let cfg = root.join("config.json");
    fs::write(
        &cfg,
        r#"{
  "downloads": {
    "offline": true,
    "version_list_source": "auto",
    "file_source": "auto",
    "mirror_root": "https://bmclapi2.bangbang93.com",
    "http_proxy": null,
    "max_parallel": 8
  },
  "auth": { "microsoft_client_id": null },
  "launch": { "java": null, "max_memory_mb": 2048, "auto_install_java": true }
}"#,
    )
    .unwrap();
    fs::write(
        root.join("accounts.json"),
        r#"{
  "accounts": [{
    "id": "fixture", "username": "Fixture",
    "uuid": "00000000-0000-0000-0000-000000000000",
    "xuid": null, "account_type": "microsoft", "oauth_client_id": null,
    "last_used_unix": null
  }],
  "selected": null
}"#,
    )
    .unwrap();
    let game_root = root.join("game-root");
    let instance = game_root.join(".ccl/instances/fixture");
    fs::create_dir_all(instance.join("game/.ccl")).unwrap();
    fs::create_dir_all(instance.join("previous/.ccl")).unwrap();
    fs::create_dir_all(instance.join("history/0001/game/.ccl")).unwrap();
    write_instance_metadata(&instance);
    // These are the historical generation receipts emitted by the corresponding baseline writers.
    let pack = r#"{"schema":1,"manifest":{"formatVersion":1,"game":"minecraft","versionId":"fixture","name":"Fixture","files":[],"dependencies":{"minecraft":"1.21.1"}},"owned_files":{},"include_optional":false}"#;
    let online = r#"{"schema":1,"versions":[],"owned_files":{}}"#;
    for generation in [
        instance.join("game"),
        instance.join("previous"),
        instance.join("history/0001/game"),
    ] {
        fs::write(generation.join(".ccl/modpack.json"), pack).unwrap();
        fs::write(generation.join(".ccl/online-content.json"), online).unwrap();
    }
    let report = inspect_compatibility(&cfg, &game_root);
    assert!(report.compatible, "{report:#?}");
    assert!(
        report
            .items
            .iter()
            .any(|i| i.format == "account_metadata" && i.status == CompatibilityStatus::Compatible)
    );
    assert!(
        report
            .items
            .iter()
            .filter(|i| i.format.ends_with("receipt"))
            .count()
            >= 6
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn future_receipt_schema_refuses_compatibility_and_rollback() {
    let root = temp("future");
    let cfg = root.join("config.json");
    fs::write(&cfg, "{}").unwrap();
    let game_root = root.join("game-root");
    let generation = game_root.join(".ccl/instances/fixture/game/.ccl");
    fs::create_dir_all(&generation).unwrap();
    write_instance_metadata(&game_root.join(".ccl/instances/fixture"));
    fs::write(
        generation.join("online-content.json"),
        r#"{"schema":2,"versions":[],"owned_files":{}}"#,
    )
    .unwrap();
    let report = inspect_compatibility(&cfg, &game_root);
    assert!(!report.compatible);
    assert!(
        report
            .items
            .iter()
            .any(|i| i.format == "online_content_receipt"
                && i.status == CompatibilityStatus::Incompatible)
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn receipt_with_supported_number_but_invalid_shape_is_refused() {
    let root = temp("malformed-receipt");
    let cfg = root.join("config.json");
    fs::write(&cfg, "{}").unwrap();
    let game_root = root.join("game-root");
    let instance = game_root.join(".ccl/instances/fixture");
    let generation = instance.join("game/.ccl");
    fs::create_dir_all(&generation).unwrap();
    write_instance_metadata(&instance);
    fs::write(generation.join("modpack.json"), r#"{"schema":1}"#).unwrap();
    let report = inspect_compatibility(&cfg, &game_root);
    assert!(!report.compatible);
    assert!(report.items.iter().any(|i| i.format == "modpack_receipt" && i.status == CompatibilityStatus::Incompatible));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn unknown_future_config_fields_are_refused_without_rewriting() {
    let root = temp("unknown");
    let cfg = root.join("config.json");
    let original = br#"{"future_schema":2}"#;
    fs::write(&cfg, original).unwrap();
    let report = inspect_compatibility(&cfg, &root.join("game"));
    assert!(!report.compatible);
    assert_eq!(fs::read(&cfg).unwrap(), original);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn oversized_metadata_is_refused() {
    let root = temp("large");
    let cfg = root.join("config.json");
    fs::write(&cfg, vec![b' '; 2 * 1024 * 1024 + 1]).unwrap();
    let report = inspect_compatibility(&cfg, &root.join("game"));
    assert!(!report.compatible);
    assert!(report.items.iter().any(|i| i.format == "launcher_config" && i.status == CompatibilityStatus::Incompatible));
    let _ = fs::remove_dir_all(root);
}
