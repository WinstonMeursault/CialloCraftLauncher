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
