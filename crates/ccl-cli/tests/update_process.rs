//! End-to-end recovery tests drive the real, built CCL helper subprocess.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Sandbox(PathBuf);
static NEXT_SANDBOX: AtomicU64 = AtomicU64::new(0);
impl Sandbox {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        loop {
            let id = NEXT_SANDBOX.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "ccl-update-process-{}-{stamp}-{id}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create update test sandbox: {error}"),
            }
        }
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    _root: Sandbox,
    target: PathBuf,
    helper: PathBuf,
    directory: PathBuf,
    old_bytes: Vec<u8>,
    old_hash: String,
    nonce: String,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn built_ccl() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_ccl"))
}
fn identity() -> Value {
    let output = Command::new(built_ccl())
        .arg("__build-identity")
        .output()
        .unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}
fn candidate() -> (Vec<u8>, Value) {
    static CANDIDATE: OnceLock<(Vec<u8>, Value)> = OnceLock::new();
    CANDIDATE.get_or_init(|| {
        let root = Sandbox::new();
        let current = identity();
        let version = "0.1.1";
        let commit = "a".repeat(40);
        let channel = current["channel"].as_str().unwrap();
        let target = current["target"].as_str().unwrap();
        let source = r#"
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("__update-health") {
        println!("{{\"nonce\":\"{}\",\"compatible\":true,\"build\":{{\"version\":\"__VERSION__\",\"commit\":\"__COMMIT__\",\"channel\":\"__CHANNEL__\",\"target\":\"__TARGET__\"}}}}", args[2]);
    } else { std::process::exit(1); }
}
"#
            .replace("__VERSION__", version)
            .replace("__COMMIT__", &commit)
            .replace("__CHANNEL__", channel)
            .replace("__TARGET__", target);
        let source_path = root.0.join("candidate.rs");
        let executable_path = root.0.join(if cfg!(windows) { "candidate.exe" } else { "candidate" });
        fs::write(&source_path, source).unwrap();
        let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let output = Command::new(compiler).arg(&source_path).arg("-o").arg(&executable_path).output().unwrap();
        assert!(output.status.success(), "rustc candidate build failed: {}", String::from_utf8_lossy(&output.stderr));
        let build = json!({"version": version, "commit": commit, "channel": channel, "target": target});
        (fs::read(executable_path).unwrap(), build)
    }).clone()
}
fn fixture(candidate: &[u8], new_identity: Value) -> Fixture {
    let root = Sandbox::new();
    let target = root.0.join(if cfg!(windows) { "ccl.exe" } else { "ccl" });
    let old_bytes = fs::read(built_ccl()).unwrap();
    fs::write(&target, &old_bytes).unwrap();
    executable(&target);
    let old_hash = digest(&old_bytes);
    let directory = root.0.join(".ccl-update");
    fs::create_dir_all(&directory).unwrap();
    let helper = directory.join(format!(
        "helper-{old_hash}{}",
        if cfg!(windows) { ".exe" } else { "" }
    ));
    fs::copy(built_ccl(), &helper).unwrap();
    executable(&helper);
    fs::write(directory.join("candidate"), candidate).unwrap();
    executable(&directory.join("candidate"));
    let canonical_target = fs::canonicalize(&target).unwrap();
    let nonce = "1a".repeat(32);
    let current_identity = identity();
    let journal = json!({
        "schema": 1,
        "state": "staged",
        "nonce": nonce,
        "target": canonical_target,
        "old_sha256": old_hash,
        "old_identity": current_identity,
        "new_sha256": digest(candidate),
        "new_identity": new_identity,
        "config": root.0.join("config.json"),
        "game_root": root.0.join("game"),
        "error": null
    });
    fs::write(
        directory.join("transaction.json"),
        serde_json::to_vec_pretty(&journal).unwrap(),
    )
    .unwrap();
    Fixture {
        _root: root,
        target,
        helper,
        directory,
        old_bytes,
        old_hash,
        nonce,
    }
}
#[cfg(unix)]
fn executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
#[cfg(not(unix))]
fn executable(_: &Path) {}

fn start_helper(f: &Fixture, nonce: &str) -> Child {
    Command::new(&f.helper)
        .arg("__update-helper")
        .arg(&f.target)
        .arg(nonce)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}
fn wait_bounded(child: Child) -> Output {
    wait_bounded_with_poll(child, Duration::from_millis(25))
}
fn wait_bounded_with_poll(mut child: Child, poll_interval: Duration) -> Output {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("update helper exceeded 60-second test bound");
        }
        thread::sleep(poll_interval);
    }
}
fn transaction_state(f: &Fixture) -> String {
    serde_json::from_slice::<Value>(&fs::read(f.directory.join("transaction.json")).unwrap())
        .unwrap()["state"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn helper_replaces_and_commits_using_its_own_verified_image() {
    let (new_bytes, new_id) = candidate();
    let f = fixture(&new_bytes, new_id);
    let output = wait_bounded(start_helper(&f, &f.nonce));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(transaction_state(&f), "committed");
    assert_ne!(digest(&fs::read(&f.target).unwrap()), f.old_hash);
}

#[test]
fn failed_candidate_health_restores_old_executable_and_records_rollback() {
    let (candidate_bytes, mut wrong_id) = candidate();
    wrong_id["commit"] = Value::String("b".repeat(40));
    let f = fixture(&candidate_bytes, wrong_id);
    let output = wait_bounded(start_helper(&f, &f.nonce));
    assert_eq!(
        output.status.code(),
        Some(17),
        "helper should report failed update after recovery"
    );
    assert_eq!(transaction_state(&f), "rolled_back");
    assert_eq!(fs::read(&f.target).unwrap(), f.old_bytes);
}

#[test]
fn invalid_nonce_cannot_start_transaction_or_change_target() {
    let (bytes, id) = candidate();
    let f = fixture(&bytes, id);
    let output = wait_bounded(start_helper(&f, &"2b".repeat(32)));
    assert_eq!(output.status.code(), Some(17));
    assert_eq!(transaction_state(&f), "staged");
    assert_eq!(fs::read(&f.target).unwrap(), f.old_bytes);
}

#[test]
fn staged_missing_target_recovers_from_verified_helper() {
    let (candidate_bytes, candidate_identity) = candidate();
    let f = fixture(&candidate_bytes, candidate_identity);
    assert_eq!(digest(&fs::read(&f.helper).unwrap()), f.old_hash);
    fs::remove_file(&f.target).unwrap();

    let output = wait_bounded(start_helper(&f, &f.nonce));
    assert_eq!(
        output.status.code(),
        Some(17),
        "failed update should report recovered rollback"
    );
    assert_eq!(transaction_state(&f), "rolled_back");
    assert_eq!(fs::read(&f.target).unwrap(), f.old_bytes);
    let health = Command::new(&f.target)
        .arg("__build-identity")
        .output()
        .unwrap();
    assert!(
        health.status.success(),
        "restored executable must start: {}",
        String::from_utf8_lossy(&health.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&health.stdout).unwrap(),
        identity()
    );
}

#[test]
#[ignore = "performance harness only"]
fn benchmark_update_transactions() {
    let (valid_candidate, valid_identity) = candidate();
    let mut rollback_identity = valid_identity.clone();
    rollback_identity["commit"] = Value::String("b".repeat(40));
    let mut commit_ms = Vec::new();
    let mut rollback_ms = Vec::new();
    for _ in 0..3 {
        let commit_fixture = fixture(&valid_candidate, valid_identity.clone());
        let start = Instant::now();
        let child = start_helper(&commit_fixture, &commit_fixture.nonce);
        let output = wait_bounded_with_poll(child, Duration::from_millis(1));
        let elapsed_ms = start.elapsed().as_secs_f64() * 1_000.0;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(transaction_state(&commit_fixture), "committed");
        commit_ms.push(elapsed_ms);

        let rollback_fixture = fixture(&valid_candidate, rollback_identity.clone());
        let start = Instant::now();
        let child = start_helper(&rollback_fixture, &rollback_fixture.nonce);
        let output = wait_bounded_with_poll(child, Duration::from_millis(1));
        let elapsed_ms = start.elapsed().as_secs_f64() * 1_000.0;
        assert_eq!(output.status.code(), Some(17));
        assert_eq!(transaction_state(&rollback_fixture), "rolled_back");
        assert_eq!(
            fs::read(&rollback_fixture.target).unwrap(),
            rollback_fixture.old_bytes,
            "health failure must restore the previous executable"
        );
        rollback_ms.push(elapsed_ms);
    }
    println!(
        "CCL_UPDATE_BENCHMARK={}",
        json!({
            "scenario": "helper_update_and_health_failure_rollback",
            "commit_ms": commit_ms,
            "rollback_ms": rollback_ms,
            "candidate_bytes": valid_candidate.len(),
            "samples_per_scenario": 3
        })
    );
}

#[cfg(windows)]
#[test]
fn helper_waits_for_a_locked_target_then_completes_replacement() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
    let (bytes, id) = candidate();
    let f = fixture(&bytes, id);
    // Omitting FILE_SHARE_DELETE simulates a process holding the image open.
    let handle = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&f.target)
        .unwrap();
    let child = start_helper(&f, &f.nonce);
    let deadline = Instant::now() + Duration::from_secs(15);
    while transaction_state(&f) != "replacing" && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(
        transaction_state(&f),
        "replacing",
        "helper never reached its replacement step"
    );
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        fs::read(&f.target).unwrap(),
        f.old_bytes,
        "target changed while its delete share was denied"
    );
    drop(handle);
    let output = wait_bounded(child);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(transaction_state(&f), "committed");
}
