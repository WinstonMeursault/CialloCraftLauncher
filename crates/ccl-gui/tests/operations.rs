#[cfg(unix)]
use ccl_core::instances::InstanceError;
use ccl_core::{
    CancellationToken, TaskContext, config::DownloadConfig, install::InstallService,
    loaders::LoaderKind,
};
use ccl_gui::operations::{
    GuiOperations, InstallRequest, LaunchIdentity, LaunchIdentityKind, LaunchRequest,
};
#[cfg(unix)]
use ccl_gui::{PreferencesStore, bootstrap};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use zip::{ZipWriter, write::SimpleFileOptions};

fn context() -> TaskContext {
    TaskContext::new(CancellationToken::default(), Arc::new(|_| {}))
}

struct Fixture {
    url: String,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let client = b"GUI operations fixture client";
        let asset_index = serde_json::to_vec(&json!({"objects":{}})).unwrap();
        let metadata = serde_json::to_vec(&json!({
            "id":"fixture-1",
            "type":"release",
            "mainClass":"example.Main",
            "javaVersion":{"majorVersion":21,"component":"jre-legacy"},
            "downloads":{"client":{"url":format!("{base}/client.jar"),"sha1":sha1(client),"size":client.len()}},
            "assetIndex":{"id":"fixture-assets","url":format!("{base}/assets/index.json"),"sha1":sha1(&asset_index),"size":asset_index.len()},
            "minecraftArguments":"--username ${auth_player_name} --gameDir ${game_directory}"
        }))
        .unwrap();
        let manifest = serde_json::to_vec(&json!({
            "versions":[{"id":"fixture-1","type":"release","url":format!("{base}/version.json"),"sha1":sha1(&metadata)}]
        }))
        .unwrap();
        let routes = Arc::new(HashMap::from([
            ("/manifest.json".to_owned(), manifest),
            ("/version.json".to_owned(), metadata),
            ("/assets/index.json".to_owned(), asset_index),
            ("/client.jar".to_owned(), client.to_vec()),
        ]));
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = stop.clone();
        let worker = thread::spawn(move || {
            while !server_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let routes = routes.clone();
                        thread::spawn(move || serve(stream, &routes));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            url: format!("{base}/manifest.json"),
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

fn serve(mut stream: TcpStream, routes: &HashMap<String, Vec<u8>>) {
    // Windows accepted sockets inherit the nonblocking listener mode.
    // This worker serves a complete request synchronously on every platform.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        let Ok(count) = stream.read(&mut buffer) else {
            return;
        };
        if count == 0 {
            return;
        }
        request.extend_from_slice(&buffer[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let path = String::from_utf8_lossy(&request)
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_owned();
    let (status, body) = routes
        .get(&path)
        .map_or(("404 Not Found", &[][..]), |body| {
            ("200 OK", body.as_slice())
        });
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn sha1(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}

fn empty_mrpack(path: &std::path::Path) {
    let file = fs::File::create(path).unwrap();
    let mut archive = ZipWriter::new(file);
    archive
        .start_file("modrinth.index.json", SimpleFileOptions::default())
        .unwrap();
    archive
        .write_all(
            br#"{"formatVersion":1,"game":"minecraft","versionId":"fixture","name":"Fixture pack","files":[],"dependencies":{"minecraft":"fixture-1"}}"#,
        )
        .unwrap();
    archive.finish().unwrap();
}

#[cfg(unix)]
fn fixture_java(path: &std::path::Path, sleep: bool) {
    use std::os::unix::fs::PermissionsExt;
    let arch = std::env::consts::ARCH;
    let script = if sleep {
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"-XshowSettings:properties\" ]; then echo '    java.version = 21.0.1' >&2; echo '    os.arch = {arch}' >&2; exit 0; fi\necho fixture-launch-ready\nexec sleep 30\n"
        )
    } else {
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"-XshowSettings:properties\" ]; then echo '    java.version = 21.0.1' >&2; echo '    os.arch = {arch}' >&2; exit 0; fi\necho fixture-launch-ready\nexit 0\n"
        )
    };
    fs::write(path, script).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn frontend_launch_request_shape_deserializes_as_owned_dto() {
    let request: LaunchRequest = serde_json::from_value(json!({
        "key": "managed:my-world",
        "identity": {"kind":"offline", "username":"Player_1", "account_id":null},
        "java": null,
        "memory_mb": 4096,
        "auto_install_java": true,
        "dry_run": true
    }))
    .unwrap();
    assert_eq!(request.key, "managed:my-world");
    assert!(matches!(request.identity.kind, LaunchIdentityKind::Offline));
    assert_eq!(request.identity.username.as_deref(), Some("Player_1"));
    assert_eq!(request.memory_mb, Some(4096));
}

#[tokio::test]
async fn install_rejects_inconsistent_shared_loader_request_without_touching_game_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    let operations = GuiOperations::new(root.clone(), temp.path().join("config.json")).unwrap();
    let error = operations
        .install(
            InstallRequest {
                id: "test".into(),
                minecraft: "1.21.1".into(),
                loader: Some(LoaderKind::Fabric),
                loader_version: Some("0.16.10".into()),
                shared: true,
            },
            context(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid_install");
    assert!(!root.exists());
}

#[tokio::test]
async fn managed_loader_creation_conflicts_without_replacing_existing_save() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    let save = root.join(".ccl/instances/existing/game/saves/world/marker");
    fs::create_dir_all(save.parent().unwrap()).unwrap();
    fs::write(&save, "save data").unwrap();
    let operations = GuiOperations::new(root, temp.path().join("config.json")).unwrap();
    let error = operations
        .install(
            InstallRequest {
                id: "existing".into(),
                minecraft: "1.21.1".into(),
                loader: Some(LoaderKind::Fabric),
                loader_version: Some("0.16.10".into()),
                shared: false,
            },
            context(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "instance_conflict");
    assert_eq!(fs::read_to_string(save).unwrap(), "save data");
}

#[tokio::test]
async fn import_new_conflicts_under_instance_lock_and_preserves_existing_save() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    let save = root.join(".ccl/instances/existing/game/saves/world/marker");
    fs::create_dir_all(save.parent().unwrap()).unwrap();
    fs::write(&save, "precious save").unwrap();
    let archive = temp.path().join("fixture.mrpack");
    empty_mrpack(&archive);
    let operations = GuiOperations::new(root, temp.path().join("config.json")).unwrap();
    let error = operations
        .import(
            ccl_gui::operations::ImportRequest {
                update_existing: false,
                id: "existing".into(),
                archive,
                include_optional: false,
                replace_modified: false,
                allowed_hosts: vec![],
            },
            context(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "instance_conflict");
    assert_eq!(fs::read_to_string(save).unwrap(), "precious save");
}

#[tokio::test]
async fn launch_resolves_only_library_keys_and_log_history_is_bounded_shape() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    fs::create_dir_all(&root).unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let operations = GuiOperations::new(root.clone(), temp.path().join("config.json")).unwrap();
    let error = operations
        .launch(
            LaunchRequest {
                key: format!("vanilla:{}", outside.display()),
                identity: LaunchIdentity {
                    kind: LaunchIdentityKind::Offline,
                    username: Some("Player".into()),
                    account_id: None,
                },
                java: None,
                memory_mb: None,
                auto_install_java: false,
                dry_run: true,
            },
            context(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid_launch_key");
    assert_eq!(operations.log_runs("vanilla:demo").unwrap(), json!([]));
    assert_eq!(
        operations
            .read_run_log("vanilla:demo", None, 20)
            .unwrap_err()
            .code,
        "log_read"
    );
}

#[test]
fn saved_log_reads_bound_file_size_lines_and_line_length() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    let log_dir = root.join(".ccl/logs");
    fs::create_dir_all(&log_dir).unwrap();
    let log = log_dir.join("123-run.log");
    let mut content = vec![b'a'; 1_100_000];
    content.extend_from_slice(b"\nshort-line\n");
    content.extend(std::iter::repeat_n(b'b', 9000));
    content.extend_from_slice(b"\nlast-tail\n");
    fs::write(log, content).unwrap();
    let operations = GuiOperations::new(root, temp.path().join("config.json")).unwrap();
    let result = operations.read_run_log("vanilla:demo", None, 3).unwrap();
    assert_eq!(result["run_id"], "123-run");
    assert!(result["truncated"].as_bool().unwrap());
    let text = result["text"].as_str().unwrap();
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "short-line");
    assert_eq!(lines[1].len(), 8192);
    assert_eq!(lines[2], "last-tail");
}

#[cfg(unix)]
#[test]
fn saved_log_rejects_symlinked_log_directory() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    let ccl = root.join(".ccl");
    fs::create_dir_all(&ccl).unwrap();
    let external = temp.path().join("external-logs");
    fs::create_dir_all(&external).unwrap();
    symlink(external, ccl.join("logs")).unwrap();
    let operations = GuiOperations::new(root, temp.path().join("config.json")).unwrap();
    let error = operations
        .read_run_log("vanilla:demo", None, 20)
        .unwrap_err();
    assert_eq!(error.code, "log_read");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_install_launch_logs_cancellation_and_launch_lock_use_real_core_services() {
    use ccl_core::{TaskEvent, instances::InstanceService};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    let config_path = temp.path().join("config.json");
    let fixture = Fixture::new();
    let installer = InstallService::new(DownloadConfig::default())
        .unwrap()
        .with_manifest_url(&fixture.url);
    let operations = GuiOperations::new(root.clone(), config_path.clone())
        .unwrap()
        .with_install_service(installer);
    let installed = operations
        .install(
            InstallRequest {
                id: "fixture-world".into(),
                minecraft: "fixture-1".into(),
                loader: None,
                loader_version: None,
                shared: false,
            },
            context(),
        )
        .await
        .unwrap();
    assert_eq!(installed["instance"]["loader"], Value::Null);
    assert!(
        root.join(".ccl/instances/fixture-world/game/versions/fixture-1/fixture-1.jar")
            .is_file()
    );

    let java = temp.path().join("fixture-java");
    fixture_java(&java, false);
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let launch_context = TaskContext::new(
        CancellationToken::default(),
        Arc::new(move |event| {
            let _ = events.send(event);
        }),
    );
    let worker_ops = operations.clone();
    let worker = tokio::spawn(async move {
        worker_ops
            .launch(
                LaunchRequest {
                    key: "managed:fixture-world".into(),
                    identity: LaunchIdentity {
                        kind: LaunchIdentityKind::Offline,
                        username: Some("FixturePlayer".into()),
                        account_id: None,
                    },
                    java: Some(java),
                    memory_mb: Some(1024),
                    auto_install_java: false,
                    dry_run: false,
                },
                launch_context,
            )
            .await
    });
    loop {
        let event = tokio::time::timeout(Duration::from_secs(15), received.recv())
            .await
            .expect("launch emits a process start event");
        match event {
            Some(TaskEvent::ProcessStarted { .. }) => break,
            Some(_) => continue,
            None => panic!("launch completed before starting a process"),
        }
    }
    let instances = InstanceService::new(root.clone(), DownloadConfig::default()).unwrap();
    assert!(matches!(
        instances.acquire_launch("fixture-world"),
        Err(InstanceError::Conflict(_))
    ));
    let result = worker.await.unwrap().unwrap();
    assert_eq!(result["launch"]["exit_code"], 0);
    assert_eq!(result["ok"], true);
    let run_id = result["launch"]["run_id"].as_str().unwrap();
    let log = operations
        .read_run_log("managed:fixture-world", Some(run_id), 20)
        .unwrap();
    assert!(
        log["text"]
            .as_str()
            .unwrap()
            .contains("fixture-launch-ready")
    );
    assert_eq!(
        operations
            .log_runs("managed:fixture-world")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(instances.acquire_launch("fixture-world").is_ok());

    let java = temp.path().join("fixture-java-sleeper");
    fixture_java(&java, true);
    let token = CancellationToken::default();
    let event_token = token.clone();
    let cancellation_context = TaskContext::new(
        token.clone(),
        Arc::new(move |event| {
            if matches!(event, TaskEvent::Log { line, .. } if line == "fixture-launch-ready") {
                event_token.cancel();
            }
        }),
    );
    let cancelled = operations
        .launch(
            LaunchRequest {
                key: "managed:fixture-world".into(),
                identity: LaunchIdentity {
                    kind: LaunchIdentityKind::Offline,
                    username: Some("FixturePlayer".into()),
                    account_id: None,
                },
                java: Some(java.clone()),
                memory_mb: Some(1024),
                auto_install_java: false,
                dry_run: false,
            },
            cancellation_context,
        )
        .await
        .unwrap();
    assert_eq!(cancelled["launch"]["cancelled"], true);
    assert!(token.is_cancelled());
    assert_eq!(
        operations
            .log_runs("managed:fixture-world")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn managed_vanilla_install_uses_verified_downloads_on_every_platform() {
    let temp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new();
    let installer = InstallService::new(DownloadConfig::default())
        .unwrap()
        .with_manifest_url(&fixture.url);
    let root = temp.path().join("game");
    let service = GuiOperations::new(root.clone(), temp.path().join("config.json"))
        .unwrap()
        .with_install_service(installer);
    let request = InstallRequest {
        id: "portable-fixture".into(),
        minecraft: "fixture-1".into(),
        loader: None,
        loader_version: None,
        shared: false,
    };
    let installed = service.install(request.clone(), context()).await.unwrap();
    assert_eq!(installed["instance"]["loader"], Value::Null);
    let client = root.join(".ccl/instances/portable-fixture/game/versions/fixture-1/fixture-1.jar");
    assert_eq!(fs::read(&client).unwrap(), b"GUI operations fixture client");
    assert_eq!(
        service.install(request, context()).await.unwrap_err().code,
        "instance_conflict"
    );
    assert_eq!(fs::read(client).unwrap(), b"GUI operations fixture client");
}

#[cfg(unix)]
#[tokio::test]
async fn unicode_instance_name_roundtrips_through_library_and_dry_run_launch() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    fs::create_dir_all(&root).unwrap();
    let config_path = temp.path().join("config.json");
    let fixture = Fixture::new();
    let installer = InstallService::new(DownloadConfig::default())
        .unwrap()
        .with_manifest_url(&fixture.url);
    let operations = GuiOperations::new(root.clone(), config_path.clone())
        .unwrap()
        .with_install_service(installer);
    let display_id = "我的整合包 日本語! (1)";
    operations
        .install(
            InstallRequest {
                id: display_id.into(),
                minecraft: "fixture-1".into(),
                loader: None,
                loader_version: None,
                shared: false,
            },
            context(),
        )
        .await
        .unwrap();
    operations
        .install(
            InstallRequest {
                id: "ignored-for-shared-version".into(),
                minecraft: "fixture-1".into(),
                loader: None,
                loader_version: None,
                shared: true,
            },
            context(),
        )
        .await
        .unwrap();

    let preferences = PreferencesStore::new(temp.path().join("preferences"));
    preferences.select_root(root.clone()).unwrap();
    let library = bootstrap(&preferences, &config_path, None).unwrap();
    let entry = library
        .entries
        .iter()
        .find(|entry| entry.id == display_id)
        .expect("managed instance is discovered in the library");
    assert_eq!(entry.key, format!("managed:{display_id}"));
    assert_eq!(
        entry.game_directory,
        fs::canonicalize(root.join(".ccl/instances").join(display_id).join("game")).unwrap()
    );
    let shared_entry = library
        .entries
        .iter()
        .find(|entry| entry.key == "vanilla:fixture-1")
        .expect("existing shared version remains in the library");
    assert_eq!(
        shared_entry.game_directory,
        fs::canonicalize(&root).unwrap()
    );

    // Model an already-installed HMCL/shared profile with a Unicode and spaced ID.
    let external_id = "HMCL 中文 1.20.1";
    let source_version = root.join("versions/fixture-1");
    let external_version = root.join("versions").join(external_id);
    fs::create_dir_all(&external_version).unwrap();
    let mut external_metadata: Value =
        serde_json::from_slice(&fs::read(source_version.join("fixture-1.json")).unwrap()).unwrap();
    external_metadata["id"] = json!(external_id);
    fs::write(
        external_version.join(format!("{external_id}.json")),
        serde_json::to_vec(&external_metadata).unwrap(),
    )
    .unwrap();
    fs::copy(
        source_version.join("fixture-1.jar"),
        external_version.join(format!("{external_id}.jar")),
    )
    .unwrap();
    let library_with_external = bootstrap(&preferences, &config_path, None).unwrap();
    let external_entry = library_with_external
        .entries
        .iter()
        .find(|entry| entry.key == format!("vanilla:{external_id}"))
        .expect("Unicode shared profile remains discoverable");
    assert_eq!(
        external_entry.game_directory,
        fs::canonicalize(&root).unwrap()
    );

    let java = temp.path().join("fixture-java");
    fixture_java(&java, false);
    let launch = operations
        .launch(
            LaunchRequest {
                key: entry.key.clone(),
                identity: LaunchIdentity {
                    kind: LaunchIdentityKind::Offline,
                    username: Some("FixturePlayer".into()),
                    account_id: None,
                },
                java: Some(java.clone()),
                memory_mb: Some(1024),
                auto_install_java: false,
                dry_run: true,
            },
            context(),
        )
        .await
        .unwrap();
    assert_eq!(launch["ok"], true);

    let external_launch = operations
        .launch(
            LaunchRequest {
                key: external_entry.key.clone(),
                identity: LaunchIdentity {
                    kind: LaunchIdentityKind::Offline,
                    username: Some("FixturePlayer".into()),
                    account_id: None,
                },
                java: Some(java),
                memory_mb: Some(1024),
                auto_install_java: false,
                dry_run: true,
            },
            context(),
        )
        .await
        .unwrap();
    assert_eq!(external_launch["ok"], true);
}

#[tokio::test]
async fn advanced_instance_content_lifecycle_uses_actual_locked_core_transactions() {
    use ccl_core::{content::ContentKind, instances::InstanceService};
    use ccl_gui::instance_service::{InstanceOperations, InstanceRequest as R};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    let fixture = Fixture::new();
    let installer = InstallService::new(DownloadConfig::default())
        .unwrap()
        .with_manifest_url(&fixture.url);
    GuiOperations::new(root.clone(), temp.path().join("config.json"))
        .unwrap()
        .with_install_service(installer)
        .install(
            InstallRequest {
                id: "world".into(),
                minecraft: "fixture-1".into(),
                loader: None,
                loader_version: None,
                shared: false,
            },
            context(),
        )
        .await
        .unwrap();
    let service = InstanceOperations::new(root.clone(), DownloadConfig::default());
    let core = InstanceService::new(root.clone(), DownloadConfig::default()).unwrap();
    let source = temp.path().join("test.jar");
    fs::write(&source, b"local fixture mod").unwrap();
    service
        .execute(
            R::ContentAdd {
                id: "world".into(),
                kind: ContentKind::Mod,
                source,
                name: None,
            },
            Some(context()),
        )
        .await
        .unwrap();
    let game = core.game_directory("world").unwrap();
    assert!(game.join("mods/test.jar").is_file());
    service
        .execute(
            R::ContentDisable {
                id: "world".into(),
                name: "test.jar".into(),
            },
            Some(context()),
        )
        .await
        .unwrap();
    assert!(game.join("mods/test.jar.disabled").is_file());
    service
        .execute(
            R::ContentEnable {
                id: "world".into(),
                name: "test.jar.disabled".into(),
            },
            Some(context()),
        )
        .await
        .unwrap();
    let backup = temp.path().join("userdata.zip");
    service
        .execute(
            R::ContentBackup {
                id: "world".into(),
                output: backup.clone(),
            },
            Some(context()),
        )
        .await
        .unwrap();
    service
        .execute(
            R::ContentRemove {
                id: "world".into(),
                kind: ContentKind::Mod,
                name: "test.jar".into(),
            },
            Some(context()),
        )
        .await
        .unwrap();
    assert!(!game.join("mods/test.jar").exists());
    service
        .execute(
            R::ContentRestore {
                id: "world".into(),
                archive: backup,
            },
            Some(context()),
        )
        .await
        .unwrap();
    assert!(game.join("mods/test.jar").is_file());
    let guard = core.acquire_launch("world").unwrap();
    assert!(
        service
            .execute(R::Delete { id: "world".into() }, Some(context()))
            .await
            .is_err()
    );
    drop(guard);
    let history = service
        .execute(R::History { id: "world".into() }, None)
        .await
        .unwrap();
    assert!(!history["history"].as_array().unwrap().is_empty());
    let before = fs::read(game.join("mods/test.jar")).unwrap();
    service
        .execute(
            R::Clean {
                id: "world".into(),
                keep: 0,
                include_previous: true,
                apply: false,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(fs::read(game.join("mods/test.jar")).unwrap(), before);
    service
        .execute(
            R::Copy {
                id: "world".into(),
                destination: "copy".into(),
            },
            Some(context()),
        )
        .await
        .unwrap();
    let copy_game = core.game_directory("copy").unwrap();
    fs::write(copy_game.join("mods/test.jar"), b"independent").unwrap();
    assert_eq!(fs::read(game.join("mods/test.jar")).unwrap(), before);
    service
        .execute(
            R::Rename {
                id: "copy".into(),
                destination: "renamed".into(),
            },
            Some(context()),
        )
        .await
        .unwrap();
    assert!(core.metadata("copy").is_err());
    assert!(core.metadata("renamed").is_ok());
    service
        .execute(
            R::Delete {
                id: "renamed".into(),
            },
            Some(context()),
        )
        .await
        .unwrap();
    assert!(core.metadata("renamed").is_err());
    let token = CancellationToken::default();
    token.cancel();
    let err = service
        .execute(
            R::Delete { id: "world".into() },
            Some(TaskContext::new(token, Arc::new(|_| {}))),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "cancelled");
    assert!(core.metadata("world").is_ok());
}
