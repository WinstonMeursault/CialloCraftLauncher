use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, mpsc},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use ccl_core::{
    config::DownloadConfig,
    install::{InstallError, InstallOptions, InstallService},
    instances::{InstanceError, InstanceService},
    task::{CancellationToken, TaskContext},
};
use serde_json::json;
use sha1::{Digest, Sha1};

fn sha1(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}

fn instance_service(root: PathBuf, fixture: &Fixture) -> InstanceService {
    let installer = InstallService::new(DownloadConfig::default())
        .unwrap()
        .with_manifest_url(fixture.url("/manifest.json"));
    InstanceService::new(root, DownloadConfig::default())
        .unwrap()
        .with_install_service(installer)
}

struct Fixture {
    root: String,
    gate: Option<Arc<Gate>>,
    _thread: thread::JoinHandle<()>,
}

struct Gate {
    path: String,
    entered: mpsc::Sender<()>,
    entered_rx: Mutex<mpsc::Receiver<()>>,
    released: Mutex<bool>,
    released_cv: Condvar,
}

impl Fixture {
    fn new(routes: HashMap<String, Vec<u8>>) -> Self {
        Self::build(routes, None)
    }

    fn held(routes: HashMap<String, Vec<u8>>, path: &str) -> Self {
        let (entered, entered_rx) = mpsc::channel();
        let gate = Arc::new(Gate {
            path: path.into(),
            entered,
            entered_rx: Mutex::new(entered_rx),
            released: Mutex::new(false),
            released_cv: Condvar::new(),
        });
        Self::build(routes, Some(gate))
    }

    fn build(routes: HashMap<String, Vec<u8>>, gate: Option<Arc<Gate>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://{address}");
        let mut routes: HashMap<String, Vec<u8>> = routes
            .into_iter()
            .map(|(path, body)| {
                let body = String::from_utf8_lossy(&body)
                    .replace("http://fixture.invalid", &base)
                    .into_bytes();
                (path, body)
            })
            .collect();
        if let (Some(manifest), Some(version)) = (
            routes.get("/manifest.json").cloned(),
            routes.get("/version.json"),
        ) && let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&manifest)
            && let Some(entry) = value
                .get_mut("versions")
                .and_then(serde_json::Value::as_array_mut)
                .and_then(|entries| entries.first_mut())
        {
            entry["sha1"] = json!(sha1(version));
            routes.insert("/manifest.json".into(), serde_json::to_vec(&value).unwrap());
        }
        let routes = Arc::new(routes);
        let server_gate = gate.clone();
        let worker = thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let routes = routes.clone();
                let gate = server_gate.clone();
                thread::spawn(move || serve(stream, &routes, gate.as_deref()));
            }
        });
        Self {
            root: base,
            gate,
            _thread: worker,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.root)
    }

    fn wait_until_held(&self) {
        self.gate
            .as_ref()
            .expect("fixture has a gate")
            .entered_rx
            .lock()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("request reached held fixture endpoint");
    }

    fn release(&self) {
        let gate = self.gate.as_ref().expect("fixture has a gate");
        *gate.released.lock().unwrap() = true;
        gate.released_cv.notify_all();
    }
}

fn serve(mut stream: TcpStream, routes: &HashMap<String, Vec<u8>>, gate: Option<&Gate>) {
    let mut request = Vec::new();
    let mut buf = [0; 1024];
    loop {
        let Ok(n) = stream.read(&mut buf) else { return };
        if n == 0 {
            return;
        }
        request.extend_from_slice(&buf[..n]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let first_line = String::from_utf8_lossy(&request);
    let path = first_line.split_whitespace().nth(1).unwrap_or("/");
    if let Some(gate) = gate.filter(|gate| gate.path == path) {
        let _ = gate.entered.send(());
        let mut released = gate.released.lock().unwrap();
        while !*released {
            released = gate.released_cv.wait(released).unwrap();
        }
    }
    let (status, body) = routes.get(path).map_or(("404 Not Found", &[][..]), |body| {
        ("200 OK", body.as_slice())
    });
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "ccl-install-{label}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn write_route(routes: &mut HashMap<String, Vec<u8>>, path: &str, body: &[u8]) {
    routes.insert(path.into(), body.to_vec());
}

fn install_fixture_routes() -> (HashMap<String, Vec<u8>>, Vec<u8>) {
    let client = b"restartable client jar";
    let library = b"fixture library jar";
    let object = b"fixture asset object".to_vec();
    // Keep this helper self-contained: asset objects normally use global Mojang
    // endpoints, while this fixture only serves its own loopback routes.
    let index = serde_json::to_vec(&json!({"objects":{}})).unwrap();
    let metadata = serde_json::to_vec(&json!({
        "id":"fixture-1",
        "downloads":{"client":{"url":"http://fixture.invalid/client.jar","sha1":sha1(client),"size":client.len()}},
        "libraries":[{"name":"example:fixture:1.0","downloads":{"artifact":{"path":"example/fixture/1.0/fixture-1.0.jar","url":"http://fixture.invalid/library.jar","sha1":sha1(library),"size":library.len()}}}],
        "assetIndex":{"id":"fixture-assets","url":"http://fixture.invalid/assets/index.json","sha1":sha1(&index),"size":index.len()}
    }))
    .unwrap();
    let mut routes = HashMap::new();
    write_route(&mut routes, "/manifest.json", &serde_json::to_vec(&json!({"versions":[{"id":"fixture-1","url":"http://fixture.invalid/version.json","sha1":sha1(&metadata)}]})).unwrap());
    write_route(&mut routes, "/version.json", &metadata);
    write_route(&mut routes, "/client.jar", client);
    write_route(&mut routes, "/library.jar", library);
    write_route(&mut routes, "/assets/index.json", &index);
    (routes, object)
}

fn options_for(game_directory: PathBuf) -> InstallOptions {
    InstallOptions {
        game_directory,
        os: "linux".into(),
        arch: "x86_64".into(),
        os_version: String::new(),
    }
}

fn seed_asset(game: &Path, object: &[u8]) {
    let hash = sha1(object);
    let path = game.join("assets/objects").join(&hash[..2]).join(hash);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, object).unwrap();
}

#[tokio::test]
async fn full_install_reuses_valid_files_repairs_corruption_and_rejects_metadata_conflict() {
    let client = b"fixture client jar";
    let library = b"fixture library jar";
    let object = b"fixture asset object";
    let index = serde_json::to_vec(
        &json!({"objects":{"icons/test.png":{"hash":sha1(object),"size":object.len()}}}),
    )
    .unwrap();
    let index_hash = sha1(&index);
    let metadata = json!({
        "id":"fixture-1",
        "downloads":{"client":{"url":"http://fixture.invalid/client.jar","sha1":sha1(client),"size":client.len()}},
        "libraries":[{"name":"example:fixture:1.0","downloads":{"artifact":{"path":"example/fixture/1.0/fixture-1.0.jar","url":"http://fixture.invalid/library.jar","sha1":sha1(library),"size":library.len()}}}],
        "assetIndex":{"id":"fixture-assets","url":"http://fixture.invalid/assets/index.json","sha1":index_hash,"size":index.len()}
    });
    let metadata_bytes = serde_json::to_vec(&metadata).unwrap();
    let mut routes = HashMap::new();
    write_route(&mut routes, "/manifest.json", &serde_json::to_vec(&json!({"versions":[{"id":"fixture-1","url":"http://fixture.invalid/version.json","sha1":sha1(&metadata_bytes)}]})).unwrap());
    write_route(&mut routes, "/version.json", &metadata_bytes);
    write_route(&mut routes, "/client.jar", client);
    write_route(&mut routes, "/library.jar", library);
    write_route(&mut routes, "/assets/index.json", &index);
    let fixture = Fixture::new(routes);
    let game = temp_dir("full-flow");
    let asset_path = game
        .join("assets/objects")
        .join(&sha1(object)[..2])
        .join(sha1(object));
    fs::create_dir_all(asset_path.parent().unwrap()).unwrap();
    fs::write(&asset_path, object).unwrap();
    let service = InstallService::new(DownloadConfig::default())
        .unwrap()
        .with_manifest_url(fixture.url("/manifest.json"));
    let options = InstallOptions {
        game_directory: game.clone(),
        os: "linux".into(),
        arch: "x86_64".into(),
        os_version: String::new(),
    };

    let first = service.install("fixture-1", &options, None).await.unwrap();
    assert!(
        first.downloaded_files >= 4,
        "expected manifest metadata, client, library, and index downloads: {first:?}"
    );
    assert!(
        first.reused_files >= 1,
        "the seeded content-addressed asset should be reused: {first:?}"
    );
    assert_eq!(
        fs::read(game.join("versions/fixture-1/fixture-1.jar")).unwrap(),
        client
    );
    assert_eq!(
        fs::read(game.join("libraries/example/fixture/1.0/fixture-1.0.jar")).unwrap(),
        library
    );
    assert_eq!(
        fs::read(game.join("assets/indexes/fixture-assets.json")).unwrap(),
        index
    );

    let second = service.install("fixture-1", &options, None).await.unwrap();
    assert_eq!(
        second.downloaded_files, 0,
        "second install should fetch nothing: {second:?}"
    );
    assert_eq!(second.reused_files, second.total_files);

    let client_path = game.join("versions/fixture-1/fixture-1.jar");
    fs::write(&client_path, b"corrupt").unwrap();
    let repaired = service.install("fixture-1", &options, None).await.unwrap();
    assert!(repaired.downloaded_files >= 1);
    assert_eq!(fs::read(&client_path).unwrap(), client);

    let metadata_path = game.join("versions/fixture-1/fixture-1.json");
    fs::write(&metadata_path, b"{}").unwrap();
    assert!(matches!(
        service.install("fixture-1", &options, None).await,
        Err(InstallError::Conflict(_))
    ));
    fs::remove_dir_all(game).unwrap();
}

#[tokio::test]
async fn rejects_unsafe_version_before_network_access() {
    let mut routes = HashMap::new();
    routes.insert("/manifest.json".into(), b"{}".to_vec());
    let fixture = Fixture::new(routes);
    let service = InstallService::new(DownloadConfig::default())
        .unwrap()
        .with_manifest_url(fixture.url("/manifest.json"));
    let game = temp_dir("unsafe-version");
    let options = InstallOptions {
        game_directory: game.clone(),
        os: "linux".into(),
        arch: "x86_64".into(),
        os_version: String::new(),
    };
    assert!(matches!(
        service.install("../escape", &options, None).await,
        Err(InstallError::InvalidResource(_))
    ));
    assert!(!game.join("versions").exists());
    fs::remove_dir_all(game).unwrap();
}

#[tokio::test]
async fn rejects_traversal_in_library_path_without_writing_outside_game_directory() {
    let client = b"client";
    let meta = serde_json::to_vec(&json!({
        "id":"bad-path",
        "downloads":{"client":{"url":"http://fixture.invalid/client.jar","sha1":sha1(client),"size":client.len()}},
        "libraries":[{"name":"example:bad:1","downloads":{"artifact":{"path":"../../escape.jar","url":"http://fixture.invalid/library.jar","sha1":sha1(b"library")}}}],
        "assetIndex":{"id":"assets","url":"http://fixture.invalid/assets/index.json","sha1":sha1(b"{\"objects\":{}}"),"size":13}
    })).unwrap();
    let mut routes = HashMap::new();
    write_route(
        &mut routes,
        "/manifest.json",
        &serde_json::to_vec(
            &json!({"versions":[{"id":"bad-path","url":"http://fixture.invalid/version.json","sha1":sha1(&meta)}]}),
        )
        .unwrap(),
    );
    write_route(&mut routes, "/version.json", &meta);
    write_route(&mut routes, "/client.jar", client);
    write_route(&mut routes, "/assets/index.json", b"{\"objects\":{}}");
    let fixture = Fixture::new(routes);
    let game = temp_dir("unsafe-library");
    let service = InstallService::new(DownloadConfig::default())
        .unwrap()
        .with_manifest_url(fixture.url("/manifest.json"));
    let options = InstallOptions {
        game_directory: game.clone(),
        os: "linux".into(),
        arch: "x86_64".into(),
        os_version: String::new(),
    };
    assert!(matches!(
        service.install("bad-path", &options, None).await,
        Err(InstallError::InvalidResource(_))
    ));
    assert!(!game.parent().unwrap().join("escape.jar").exists());
    fs::remove_dir_all(game).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_install_can_restart_and_finish_from_partial_state() {
    let (routes, object) = install_fixture_routes();
    let fixture = Fixture::held(routes, "/client.jar");
    let game = temp_dir("cancel-restart");
    seed_asset(&game, &object);
    let service = InstallService::new(DownloadConfig::default())
        .unwrap()
        .with_manifest_url(fixture.url("/manifest.json"));
    let options = options_for(game.clone());
    let token = CancellationToken::default();
    let context = TaskContext::new(token.clone(), Arc::new(|_| {}));
    let first_service = service.clone();
    let first_options = options.clone();
    let interrupted = tokio::spawn(async move {
        first_service
            .install("fixture-1", &first_options, Some(context))
            .await
    });

    fixture.wait_until_held();
    token.cancel();
    fixture.release();
    assert!(matches!(
        interrupted.await.unwrap(),
        // Cancellation may be reported by the downloader while it is waiting
        // for the held response, or by the installer immediately after that
        // download task returns.
        Err(InstallError::Download(_)) | Err(InstallError::Cancelled)
    ));
    assert!(token.is_cancelled());

    let resumed = service.install("fixture-1", &options, None).await.unwrap();
    assert!(
        resumed.downloaded_files >= 1,
        "restart should finish the interrupted client download: {resumed:?}"
    );
    assert_eq!(
        fs::read(game.join("versions/fixture-1/fixture-1.jar")).unwrap(),
        b"restartable client jar"
    );
    fs::remove_dir_all(game).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_install_for_same_version_returns_instance_conflict() {
    let (routes, object) = install_fixture_routes();
    let fixture = Fixture::held(routes, "/client.jar");
    let game = temp_dir("same-instance");
    seed_asset(&game, &object);
    let service = InstallService::new(DownloadConfig::default())
        .unwrap()
        .with_manifest_url(fixture.url("/manifest.json"));
    let options = options_for(game.clone());
    let first_service = service.clone();
    let first_options = options.clone();
    let first = tokio::spawn(async move {
        first_service
            .install("fixture-1", &first_options, None)
            .await
    });

    fixture.wait_until_held();
    assert!(matches!(
        service.install("fixture-1", &options, None).await,
        Err(InstallError::Conflict(_))
    ));
    fixture.release();
    first.await.unwrap().unwrap();
    fs::remove_dir_all(game).unwrap();
}

#[tokio::test]
async fn warmed_install_works_with_network_offline_and_corrupt_files_fail_without_replacement() {
    let (routes, object) = install_fixture_routes();
    let fixture = Fixture::new(routes);
    let game = temp_dir("offline-warm");
    seed_asset(&game, &object);
    let options = options_for(game.clone());
    let service = InstallService::new(DownloadConfig::default())
        .unwrap()
        .with_manifest_url(fixture.url("/manifest.json"));
    service.install("fixture-1", &options, None).await.unwrap();
    let offline = InstallService::new(DownloadConfig {
        offline: true,
        ..Default::default()
    })
    .unwrap()
    .with_manifest_url("http://127.0.0.1:1/never");
    let outcome = offline.install("fixture-1", &options, None).await.unwrap();
    assert_eq!(outcome.downloaded_files, 0);
    assert_eq!(outcome.reused_files, outcome.total_files);
    let client = game.join("versions/fixture-1/fixture-1.jar");
    fs::write(&client, b"corrupt").unwrap();
    let error = offline
        .install("fixture-1", &options, None)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        InstallError::Download(ccl_core::download::DownloadError::OfflineCacheMiss(_))
    ));
    assert_eq!(fs::read(&client).unwrap(), b"corrupt");
    fs::remove_dir_all(game).unwrap();
}

#[tokio::test]
async fn instance_vanilla_install_commits_fixture_and_refuses_existing_data() {
    let (routes, _) = install_fixture_routes();
    let fixture = Fixture::new(routes);
    let root = temp_dir("instance-vanilla");
    let instances = instance_service(root.clone(), &fixture);

    let metadata = instances
        .install_vanilla("clean", "fixture-1", None)
        .await
        .unwrap();
    assert_eq!(metadata.minecraft, "fixture-1");
    assert!(metadata.loader.is_none());
    assert!(metadata.loader_version.is_none());
    let game = root.join(".ccl/instances/clean/game");
    assert_eq!(
        fs::read(game.join("versions/fixture-1/fixture-1.jar")).unwrap(),
        b"restartable client jar"
    );
    assert_eq!(instances.metadata("clean").unwrap(), metadata);

    let existing = root.join(".ccl/instances/keep/game/saves/world");
    fs::create_dir_all(&existing).unwrap();
    fs::write(existing.join("marker"), "save").unwrap();
    assert!(matches!(
        instances.install_vanilla("keep", "fixture-1", None).await,
        Err(InstanceError::Conflict(_))
    ));
    assert_eq!(fs::read_to_string(existing.join("marker")).unwrap(), "save");
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_or_cancelled_vanilla_install_never_activates_a_generation() {
    let root = temp_dir("instance-vanilla-failure");
    let failed_fixture = Fixture::new(HashMap::new());
    let failed = instance_service(root.clone(), &failed_fixture);
    assert!(
        failed
            .install_vanilla("failed", "fixture-1", None)
            .await
            .is_err()
    );
    assert!(matches!(
        failed.metadata("failed"),
        Err(InstanceError::NotFound(_))
    ));
    assert!(!root.join(".ccl/instances/failed").exists());

    let (routes, _) = install_fixture_routes();
    let fixture = Fixture::held(routes, "/client.jar");
    let service = instance_service(root.clone(), &fixture);
    let token = CancellationToken::default();
    let context = TaskContext::new(token.clone(), Arc::new(|_| {}));
    let worker_service = service.clone();
    let worker = tokio::spawn(async move {
        worker_service
            .install_vanilla("cancelled", "fixture-1", Some(context))
            .await
    });
    fixture.wait_until_held();
    assert!(matches!(
        service.acquire_launch("cancelled"),
        Err(InstanceError::Conflict(_))
    ));
    token.cancel();
    fixture.release();
    let error = worker.await.unwrap().unwrap_err();
    assert!(error.is_cancelled());
    assert!(matches!(
        service.metadata("cancelled"),
        Err(InstanceError::NotFound(_))
    ));
    assert!(!root.join(".ccl/instances/cancelled").exists());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn vanilla_install_validates_ids_before_touching_storage() {
    let (routes, _) = install_fixture_routes();
    let fixture = Fixture::new(routes);
    let root = temp_dir("instance-vanilla-lock");
    let instances = instance_service(root.clone(), &fixture);
    assert!(matches!(
        instances
            .install_vanilla("../escape", "fixture-1", None)
            .await,
        Err(InstanceError::InvalidId(_))
    ));
    assert!(matches!(
        instances.install_vanilla("safe", "../escape", None).await,
        Err(InstanceError::InvalidId(_))
    ));

    assert!(!root.join(".ccl/instances").exists());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn create_only_loader_install_rejects_existing_instance_before_network_access() {
    let root = temp_dir("instance-loader-create-only");
    let instance = root.join(".ccl/instances/existing");
    fs::create_dir_all(instance.join("game/saves/world")).unwrap();
    fs::write(instance.join("game/saves/world/marker"), "save").unwrap();
    let service = InstanceService::new(root.clone(), DownloadConfig::default()).unwrap();
    let error = service
        .install_new_loader(
            "existing",
            &ccl_core::loaders::LoaderRequest {
                kind: ccl_core::loaders::LoaderKind::Fabric,
                minecraft: "fixture-1".into(),
                version: "0.1.0".into(),
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, InstanceError::Conflict(_)));
    assert_eq!(
        fs::read_to_string(instance.join("game/saves/world/marker")).unwrap(),
        "save"
    );
    fs::remove_dir_all(root).unwrap();
}
