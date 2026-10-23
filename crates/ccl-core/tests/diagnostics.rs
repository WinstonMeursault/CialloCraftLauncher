use ccl_core::diagnostics::{DiagnosticsService, DoctorOptions, ExportOptions};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_SANDBOX: AtomicU64 = AtomicU64::new(0);
fn sandbox() -> PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    loop {
        let id = NEXT_SANDBOX.fetch_add(1, Ordering::Relaxed);
        let p =
            std::env::temp_dir().join(format!("ccl-diagnostics-{}-{n}-{id}", std::process::id()));
        match fs::create_dir(&p) {
            Ok(()) => return p,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("cannot create diagnostics test sandbox: {error}"),
        }
    }
}

#[test]
fn run_diagnosis_uses_saved_evidence_and_rejects_unsafe_ids() {
    let root = sandbox();
    let logs = root.join(".ccl/logs");
    fs::create_dir_all(&logs).unwrap();
    fs::write(logs.join("run-1.json"), r#"{"run_id":"run-1","version":"1.20.4","pid":7,"exit_code":1,"cancelled":false,"log_path":"ignored","started_at_ms":1,"finished_at_ms":2}"#).unwrap();
    fs::write(
        logs.join("run-1.log"),
        "java.lang.OutOfMemoryError: Java heap space\n",
    )
    .unwrap();
    let service = DiagnosticsService::new(&root, root.join("launcher"));
    let diagnosis = service.diagnose_run("run-1").unwrap();
    assert_eq!(diagnosis.category, "memory_exhaustion");
    assert!(diagnosis.confidence > 0.9);
    assert!(service.diagnose_run("../run-1").is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn diagnosis_reads_log_tail_and_correlates_crash_reports_by_run_window() {
    let root = sandbox();
    let logs = root.join(".ccl/logs");
    let reports = root.join("crash-reports");
    fs::create_dir_all(&logs).unwrap();
    fs::create_dir_all(&reports).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    fs::write(logs.join("tail.json"), format!(r#"{{"run_id":"tail","exit_code":1,"cancelled":false,"started_at_ms":{},"finished_at_ms":{}}}"#, now - 10_000, now + 10_000)).unwrap();
    fs::write(
        logs.join("tail.log"),
        format!(
            "{}\njava.lang.OutOfMemoryError",
            "ordinary output ".repeat(20_000)
        ),
    )
    .unwrap();
    fs::write(logs.join("crash.json"), format!(r#"{{"run_id":"crash","exit_code":1,"cancelled":false,"started_at_ms":{},"finished_at_ms":{}}}"#, now - 10_000, now + 10_000)).unwrap();
    fs::write(logs.join("crash.log"), "process exited").unwrap();
    fs::write(
        reports.join("crash-2026-10-22_test.txt"),
        "org.lwjgl.opengl initialization failed",
    )
    .unwrap();
    let service = DiagnosticsService::new(&root, root.join("launcher"));
    let tail = service.diagnose_run("tail").unwrap();
    assert_eq!(tail.category, "memory_exhaustion");
    let crash = service.diagnose_run("crash").unwrap();
    assert_eq!(crash.category, "graphics_initialization");
    assert_eq!(crash.correlated_crash_reports, 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn export_is_allowlisted_and_raw_log_is_explicit_and_bounded() {
    let root = sandbox();
    let logs = root.join(".ccl/logs");
    fs::create_dir_all(&logs).unwrap();
    fs::write(
        logs.join("x.json"),
        r#"{"run_id":"x","exit_code":2,"cancelled":false}"#,
    )
    .unwrap();
    fs::write(logs.join("x.log"), format!(
        "{} accessToken=TOP_SECRET --username Alice alice@example.test https://example.test/path?auth=QUERY_SECRET http://proxy-user:PROXY_SECRET@example.test/ /home/alice/.minecraft",
        "x".repeat(30_000)
    )).unwrap();
    let service = DiagnosticsService::new(&root, root.join("launcher"));
    let structured = service
        .export(&ExportOptions {
            run_id: Some("x".into()),
            include_raw_log_preview: false,
        })
        .unwrap();
    assert!(structured.raw_log_preview.is_none());
    assert!(!structured.structured_json.contains(root.to_str().unwrap()));
    let raw = service
        .export(&ExportOptions {
            run_id: Some("x".into()),
            include_raw_log_preview: true,
        })
        .unwrap();
    let preview = raw.raw_log_preview.unwrap();
    assert!(preview.len() <= 16 * 1024);
    for secret in [
        "TOP_SECRET",
        "QUERY_SECRET",
        "PROXY_SECRET",
        "alice@example.test",
        "/home/alice",
    ] {
        assert!(!preview.contains(secret), "preview leaked {secret}");
    }
    assert!(raw.raw_log_preview_notice.is_some());
    assert!(!raw.structured_json.contains("\"run_id\""));
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn run_diagnosis_will_not_follow_log_directory_symlinks() {
    use std::os::unix::fs::symlink;
    let root = sandbox();
    let outside = root.join("outside");
    fs::create_dir_all(outside.join("logs")).unwrap();
    fs::write(
        outside.join("logs/run.json"),
        r#"{"run_id":"run","exit_code":0,"cancelled":false}"#,
    )
    .unwrap();
    symlink(outside.join("logs"), root.join(".ccl")).unwrap();
    let service = DiagnosticsService::new(&root, root.join("data"));
    assert!(service.diagnose_run("run").is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn doctor_is_read_only_by_default_and_probes_are_opt_in() {
    let root = sandbox();
    let game = root.join("game");
    let data = root.join("data");
    fs::create_dir_all(&game).unwrap();
    fs::create_dir_all(&data).unwrap();
    let service = DiagnosticsService::new(&game, &data);
    let report = service.doctor(&DoctorOptions::default());
    assert!(report.network.is_none());
    assert!(report.write_probe.is_none());
    assert!(fs::read_dir(&data).unwrap().next().is_none());
    let report = service.doctor(&DoctorOptions {
        write_probe: true,
        ..DoctorOptions::default()
    });
    assert!(report.write_probe.unwrap().success);
    assert!(fs::read_dir(&data).unwrap().next().is_none());
    let new_data = root.join("new-data");
    let new_service = DiagnosticsService::new(&game, &new_data);
    assert!(
        !new_service
            .doctor(&DoctorOptions::default())
            .launcher_data_directory
            .exists
    );
    let report = new_service.doctor(&DoctorOptions {
        write_probe: true,
        ..DoctorOptions::default()
    });
    assert!(report.write_probe.unwrap().success);
    assert!(report.launcher_data_directory.exists);
    assert_eq!(report.launcher_data_directory.writable, Some(true));
    assert!(fs::read_dir(&new_data).unwrap().next().is_none());
    fs::remove_dir_all(root).unwrap();
}
