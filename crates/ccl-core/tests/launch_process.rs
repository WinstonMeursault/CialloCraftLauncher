use ccl_core::{
    launch::{LaunchError, LaunchPlan, run_process},
    task::{CancellationToken, TaskContext, TaskEvent},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "ccl launch {label} {} {}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn plan(executable: PathBuf, arguments: Vec<String>, working_directory: PathBuf) -> LaunchPlan {
    LaunchPlan::new(executable, arguments, working_directory)
}

fn fixture_args(name: &str) -> Vec<String> {
    vec![
        "--exact".into(),
        name.into(),
        "--ignored".into(),
        "--nocapture".into(),
    ]
}

fn fixture_executable(dir: &Path) -> PathBuf {
    let suffix = std::env::current_exe()
        .unwrap()
        .extension()
        .map(|x| format!(".{}", x.to_string_lossy()))
        .unwrap_or_default();
    let target = dir.join(format!("fixture executable{suffix}"));
    // An immutable symlink preserves the spaced executable path without opening
    // a writable copy while other parallel tests are spawning children (ETXTBSY on Linux).
    #[cfg(unix)]
    std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &target).unwrap();
    #[cfg(windows)]
    fs::copy(std::env::current_exe().unwrap(), &target).unwrap();
    target
}

#[tokio::test]
async fn launches_child_with_spaced_paths_and_preserves_stdout_stderr() {
    let root = temp_dir("paths with spaces");
    let exe = fixture_executable(&root);
    let cwd = root.join("working directory with spaces");
    fs::create_dir_all(&cwd).unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        run_process(
            &plan(exe, fixture_args("fixture_print_streams"), cwd),
            "fixture",
            &root.join("logs"),
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.exit_code, Some(0));
    let log = fs::read_to_string(result.log_path).unwrap();
    assert!(log.contains("[stdout] fixture stdout"), "{log}");
    assert!(log.contains("[stderr] fixture stderr"), "{log}");
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn reports_child_nonzero_exit() {
    let root = temp_dir("nonzero");
    let exe = fixture_executable(&root);
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        run_process(
            &plan(exe, fixture_args("fixture_exit_nonzero"), root.clone()),
            "fixture",
            &root.join("logs"),
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.exit_code, Some(17));
    assert!(!result.cancelled);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn cancels_child_after_output_marker() {
    let root = temp_dir("cancel");
    let exe = fixture_executable(&root);
    let token = CancellationToken::default();
    let callback_token = token.clone();
    let context = TaskContext::new(
        token,
        Arc::new(move |event| {
            if matches!(event, TaskEvent::Log { line, .. } if line == "cancel-ready") {
                callback_token.cancel();
            }
        }),
    );
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        run_process(
            &plan(exe, fixture_args("fixture_wait_forever"), root.clone()),
            "fixture",
            &root.join("logs"),
            Some(context),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(result.cancelled);
    assert!(
        fs::read_to_string(result.log_path)
            .unwrap()
            .contains("cancel-ready")
    );
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn reports_spawn_failure() {
    let root = temp_dir("missing executable");
    let missing = root.join("no such executable");
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        run_process(
            &plan(missing, vec![], root.clone()),
            "fixture",
            &root.join("logs"),
            None,
        ),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(LaunchError::Io { .. })));
    let _ = fs::remove_dir_all(root);
}

// Child-only fixtures are launched through this integration test binary. They
// are ignored in the parent run so ordinary `cargo test` never sleeps or exits.
#[test]
#[ignore]
fn fixture_print_streams() {
    println!("fixture stdout");
    eprintln!("fixture stderr");
}

#[test]
#[ignore]
fn fixture_exit_nonzero() {
    std::process::exit(17);
}

#[test]
#[ignore]
fn fixture_wait_forever() {
    println!("cancel-ready");
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[tokio::test]
async fn authenticated_launch_redacts_tokens_from_plans_logs_and_events() {
    use ccl_core::{
        launch_plan::{
            LaunchPlatform, MicrosoftIdentity, OfflineAccount, PlanOptions,
            build_authenticated_launch_plan,
        },
        metadata::VersionMetadata,
    };
    use std::sync::Mutex;
    let root = temp_dir("authenticated redaction");
    let executable = fixture_executable(&root);
    fs::create_dir_all(root.join("versions/test")).unwrap();
    fs::create_dir_all(root.join("natives")).unwrap();
    fs::write(root.join("versions/test/test.jar"), b"test").unwrap();
    let metadata = VersionMetadata::parse(r#"{"id":"test","mainClass":"Main","downloads":{"client":{}},"arguments":{"game":["--username","${auth_player_name}","--uuid","${auth_uuid}","--accessToken","${auth_access_token}","--session","${auth_session}","--userType","${user_type}","--clientId","${clientid}","--xuid","${auth_xuid}"]}}"#).unwrap();
    let secret = "CCL-P3-SECRET-SENTINEL";
    let identity = MicrosoftIdentity::new(
        "OnlinePlayer".into(),
        "0123456789abcdef0123456789abcdef".into(),
        "123456789".into(),
        "00000000-0000-0000-0000-000000000001".into(),
        secret.into(),
    )
    .unwrap();
    let mut plan = build_authenticated_launch_plan(
        &metadata,
        &PlanOptions {
            game_directory: root.clone(),
            version: "test".into(),
            natives_directory: root.join("natives"),
            java_executable: executable,
            max_memory_mb: 1024,
            account: OfflineAccount::new("OfflinePlayer").unwrap(),
            platform: LaunchPlatform::current(),
            logging_argument: None,
        },
        &identity,
    )
    .unwrap();
    assert!(
        plan.arguments
            .windows(2)
            .any(|a| a == ["--username", "OnlinePlayer"])
    );
    assert!(
        plan.arguments
            .windows(2)
            .any(|a| a == ["--accessToken", secret])
    );
    assert!(
        plan.arguments
            .windows(2)
            .any(|a| a == ["--userType", "msa"])
    );
    assert!(
        plan.arguments
            .iter()
            .any(|a| a == &format!("token:{secret}:0123456789abcdef0123456789abcdef"))
    );
    assert!(!serde_json::to_string(&plan).unwrap().contains(secret));
    assert!(!format!("{plan:?} {identity:?}").contains(secret));
    plan.arguments = fixture_args("fixture_print_secret");
    let events = Arc::new(Mutex::new(Vec::new()));
    let output = events.clone();
    let context = TaskContext::new(
        CancellationToken::default(),
        Arc::new(move |e| output.lock().unwrap().push(e)),
    );
    let result = run_process(&plan, "test", &root.join("logs"), Some(context))
        .await
        .unwrap();
    assert_eq!(result.exit_code, Some(0));
    let log = fs::read_to_string(result.log_path).unwrap();
    assert!(!log.contains(secret));
    assert!(log.contains("<redacted>"));
    assert!(
        !serde_json::to_string(&*events.lock().unwrap())
            .unwrap()
            .contains(secret)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore]
fn fixture_print_secret() {
    println!("token:CCL-P3-SECRET-SENTINEL:profile");
    eprintln!("accessToken=CCL-P3-SECRET-SENTINEL");
}
