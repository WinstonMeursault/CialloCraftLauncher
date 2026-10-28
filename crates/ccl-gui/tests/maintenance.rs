use ccl_core::config::{ConfigStore, LauncherConfig};
use ccl_gui::maintenance::{Maintenance, MaintenanceRequest};
use serde_json::json;
#[test]
fn settings_compare_under_lock_and_preserve_redacted_proxy_secret() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config.json");
    let store = ConfigStore::new(&config);
    store
        .set(
            "downloads.http_proxy",
            json!("http://user:secret@localhost:8080"),
        )
        .unwrap();
    let service = Maintenance {
        root: tmp.path().join("game"),
        config: config.clone(),
    };
    let initial = service.execute(MaintenanceRequest::Settings).unwrap()["settings"].clone();
    assert!(!initial.to_string().contains("secret"));
    service
        .execute(MaintenanceRequest::Set {
            key: "launch.max_memory_mb".into(),
            value: json!(4096),
            expected: initial.clone(),
        })
        .unwrap();
    assert_eq!(
        LauncherConfig::read(&config)
            .unwrap()
            .downloads
            .http_proxy
            .as_deref(),
        Some("http://user:secret@localhost:8080")
    );
    let err = service
        .execute(MaintenanceRequest::Set {
            key: "launch.max_memory_mb".into(),
            value: json!(8192),
            expected: initial,
        })
        .unwrap_err();
    assert!(err.message.contains("another process"));
    assert_eq!(
        LauncherConfig::read(&config).unwrap().launch.max_memory_mb,
        4096
    );
}
#[test]
fn whole_reset_recovers_corrupt_config_and_invalid_values_preserve_file() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config.json");
    std::fs::write(&config, "broken").unwrap();
    let service = Maintenance {
        root: tmp.path().join("game"),
        config: config.clone(),
    };
    service
        .execute(MaintenanceRequest::Reset {
            key: None,
            expected: json!(null),
        })
        .unwrap();
    let bytes = std::fs::read(&config).unwrap();
    let current = service.execute(MaintenanceRequest::Settings).unwrap()["settings"].clone();
    assert!(
        service
            .execute(MaintenanceRequest::Set {
                key: "launch.max_memory_mb".into(),
                value: json!(0),
                expected: current
            })
            .is_err()
    );
    assert_eq!(std::fs::read(&config).unwrap(), bytes);
}
#[test]
fn offline_doctor_cannot_probe_network_and_export_does_not_overwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config.json");
    ConfigStore::new(&config)
        .set("downloads.offline", json!(true))
        .unwrap();
    let service = Maintenance {
        root: tmp.path().join("game"),
        config,
    };
    assert_eq!(
        service
            .execute(MaintenanceRequest::Doctor {
                network_probe: true,
                write_probe: false
            })
            .unwrap_err()
            .code,
        "offline"
    );
    let report = service
        .execute(MaintenanceRequest::Doctor {
            network_probe: false,
            write_probe: false,
        })
        .unwrap();
    assert!(report["network"].is_null());
    assert!(report["write_probe"].is_null());
    assert!(!service.root.exists());
    let output = tmp.path().join("existing.json");
    std::fs::write(&output, "keep").unwrap();
    assert!(
        service
            .execute(MaintenanceRequest::ExportDiagnostics {
                key: None,
                run: None,
                output: output.clone(),
                include_log_preview: false
            })
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(output).unwrap(), "keep");
}

#[test]
fn profile_diagnostics_use_resource_logs_and_refuse_another_profiles_run() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    let runtime = temp.path().join("custom-runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    let runtime = std::fs::canonicalize(runtime).unwrap();
    for id in ["one", "two"] {
        let profile = root.join("versions").join(id);
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(
            profile.join(format!("{id}.json")),
            json!({"id":id,"mainClass":"Main"}).to_string(),
        )
        .unwrap();
        std::fs::write(
            profile.join("hmclversion.cfg"),
            json!({"usesGlobal":false,"gameDirType":2,"gameDir":runtime}).to_string(),
        )
        .unwrap();
    }
    let logs = root.join(".ccl/logs");
    std::fs::create_dir_all(&logs).unwrap();
    for id in ["one", "two"] {
        std::fs::write(logs.join(format!("{id}-run.json")), json!({"run_id":format!("{id}-run"),"profile_id":id,"version":id,"exit_code":0,"cancelled":false}).to_string()).unwrap();
        std::fs::write(logs.join(format!("{id}-run.log")), "fixture").unwrap();
    }
    let entry = ccl_gui::discover_entries(&root, &Default::default())
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == "one")
        .unwrap();
    let service = Maintenance {
        root,
        config: temp.path().join("config.json"),
    };
    let diagnosis = service
        .execute(MaintenanceRequest::Diagnose {
            key: Some(entry.key.clone()),
            run: Some("one-run".into()),
        })
        .unwrap();
    assert_eq!(diagnosis["run_id"], "one-run");
    assert!(
        service
            .execute(MaintenanceRequest::Diagnose {
                key: Some(entry.key),
                run: Some("two-run".into())
            })
            .is_err()
    );
    assert!(!runtime.join(".ccl/logs").exists());
}
