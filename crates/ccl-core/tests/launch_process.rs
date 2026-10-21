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
    LaunchPlan {
        executable,
        arguments,
        working_directory,
    }
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
