use super::*;
use crate::task::CancellationToken;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::Path,
    sync::Arc,
    thread,
};

fn version(project: &str, id: &str, filename: &str, bytes: &[u8]) -> VersionHit {
    VersionHit {
        provider: Provider::Modrinth,
        id: id.into(),
        project_id: project.into(),
        name: id.into(),
        version_number: id.into(),
        game_versions: vec!["1.21.1".into()],
        loaders: vec!["fabric".into()],
        files: vec![VersionFile {
            filename: filename.into(),
            url: format!("https://cdn.modrinth.com/{filename}"),
            size: bytes.len() as u64,
            sha1: None,
            sha512: Some(format!("{:x}", Sha512::digest(bytes))),
            primary: true,
        }],
        dependencies: vec![],
        optional_dependencies: vec![],
        incompatible_dependencies: vec![],
        pinned_dependencies: Default::default(),
        dependency_requirements: vec![],
    }
}
struct Fixture {
    root: PathBuf,
    game: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ccl-online-test-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let game = root.join(".ccl/instances/test/game");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.parent().unwrap().join("instance.json"),serde_json::json!({"id":"test","minecraft":"1.21.1","loader":null,"loader_version":null,"game_directory":game}).to_string()).unwrap();
        Self { root, game }
    }
    fn apply(&self, versions: &[VersionHit], bodies: &[&[u8]]) -> Result<(), OnlineError> {
        self.apply_with_context(versions, bodies, None)
    }
    fn apply_with_context(
        &self,
        versions: &[VersionHit],
        bodies: &[&[u8]],
        context: Option<&TaskContext>,
    ) -> Result<(), OnlineError> {
        let mut records = Vec::new();
        let mut downloads = Vec::new();
        for (v, body) in versions.iter().zip(bodies) {
            let file = &v.files[0];
            let path = self.root.join(format!("download-{}", downloads.len()));
            fs::write(&path, body).unwrap();
            let hash = file_sha512(&path).unwrap();
            records.push(OnlineInstalledVersion {
                provider: v.provider,
                project_id: v.project_id.clone(),
                version_id: v.id.clone(),
                version_number: v.version_number.clone(),
                files: std::collections::BTreeMap::from([(file.filename.clone(), hash.clone())]),
                metadata: Some(v.clone()),
            });
            downloads.push((file.filename.clone(), path, hash));
        }
        apply_downloads(
            self.root.clone(),
            &DownloadConfig::default(),
            "test",
            records,
            &downloads,
            context,
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn installing_another_project_preserves_existing_mods_and_update_removes_only_owned_old_version() {
    let f = Fixture::new();
    let a = version("a", "a1", "a1.jar", b"a1");
    let b = version("b", "b1", "b.jar", b"b1");
    f.apply(&[a], &[b"a1"]).unwrap();
    f.apply(&[b], &[b"b1"]).unwrap();
    assert_eq!(fs::read(f.game.join("mods/a1.jar")).unwrap(), b"a1");
    assert_eq!(fs::read(f.game.join("mods/b.jar")).unwrap(), b"b1");
    let next = version("a", "a2", "a2.jar", b"a2");
    f.apply(&[next], &[b"a2"]).unwrap();
    assert!(!f.game.join("mods/a1.jar").exists());
    assert_eq!(fs::read(f.game.join("mods/a2.jar")).unwrap(), b"a2");
    assert!(f.game.join("mods/b.jar").is_file());
    let receipt: OnlineReceipt =
        serde_json::from_slice(&fs::read(f.game.join(ONLINE_RECEIPT)).unwrap()).unwrap();
    assert_eq!(receipt.versions.len(), 2);
}
#[test]
fn dependency_set_has_distinct_downloads_and_one_rollback_snapshot() {
    let f = Fixture::new();
    let mut a = version("a", "a1", "a.jar", b"a");
    a.dependencies.push("dep".into());
    let dep = version("dep", "d1", "dep.jar", b"dep");
    f.apply(&[a, dep], &[b"a", b"dep"]).unwrap();
    assert_eq!(fs::read(f.game.join("mods/a.jar")).unwrap(), b"a");
    assert_eq!(fs::read(f.game.join("mods/dep.jar")).unwrap(), b"dep");
    let service = InstanceService::new(f.root.clone(), DownloadConfig::default()).unwrap();
    service.restore_revision("test", "previous").unwrap();
    assert!(!f.game.join("mods").exists());
}
#[test]
fn modified_owned_files_or_unmanaged_collisions_abort_without_partial_changes() {
    let f = Fixture::new();
    f.apply(&[version("a", "a1", "a.jar", b"original")], &[b"original"])
        .unwrap();
    fs::write(f.game.join("mods/a.jar"), b"local edit").unwrap();
    let before = fs::read(f.game.join(ONLINE_RECEIPT)).unwrap();
    assert!(
        f.apply(&[version("a", "a2", "new.jar", b"new")], &[b"new"])
            .is_err()
    );
    assert_eq!(fs::read(f.game.join("mods/a.jar")).unwrap(), b"local edit");
    assert!(!f.game.join("mods/new.jar").exists());
    assert_eq!(fs::read(f.game.join(ONLINE_RECEIPT)).unwrap(), before);
    fs::write(f.game.join("mods/local.jar"), b"private").unwrap();
    assert!(
        f.apply(
            &[version("b", "b1", "local.jar", b"download")],
            &[b"download"]
        )
        .is_err()
    );
    assert_eq!(fs::read(f.game.join("mods/local.jar")).unwrap(), b"private");
}

#[test]
fn cancellation_during_staged_online_apply_preserves_active_files_and_receipt() {
    let f = Fixture::new();
    let previous = version("a", "a1", "a.jar", b"previous-content");
    f.apply(&[previous], &[b"previous-content"]).unwrap();
    let receipt_path = f.game.join(ONLINE_RECEIPT);
    let previous_receipt = fs::read(&receipt_path).unwrap();
    let token = CancellationToken::default();
    let cancel_on_apply = token.clone();
    let context = TaskContext::new(
        token.clone(),
        Arc::new(move |event| {
            if matches!(event, TaskEvent::Progress { task, .. } if task == "online_content_apply") {
                cancel_on_apply.cancel();
            }
        }),
    );

    let result = f.apply_with_context(
        &[version("a", "a2", "a2.jar", b"new-content")],
        &[b"new-content"],
        Some(&context),
    );

    assert!(matches!(
        result,
        Err(OnlineError::Download(DownloadError::Cancelled))
    ));
    assert_eq!(
        fs::read(f.game.join("mods/a.jar")).unwrap(),
        b"previous-content"
    );
    assert!(!f.game.join("mods/a2.jar").exists());
    assert_eq!(fs::read(receipt_path).unwrap(), previous_receipt);
}

#[tokio::test]
async fn cancellation_on_download_progress_stops_transfer_and_keeps_existing_destination() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0; 1024];
        let _ = socket.read(&mut request);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\na")
            .unwrap();
        thread::sleep(std::time::Duration::from_millis(300));
    });
    let temporary = std::env::temp_dir().join(format!(
        "ccl-online-cancel-download-{}-{}",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&temporary).unwrap();
    let destination = temporary.join("content.jar");
    fs::write(&destination, b"keep-existing").unwrap();
    let token = CancellationToken::default();
    let cancellation = token.clone();
    let observed_progress = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = observed_progress.clone();
    let context = TaskContext::new(
        token,
        Arc::new(move |event| {
            if matches!(event, TaskEvent::Progress { .. }) {
                observed.store(true, Ordering::Release);
                cancellation.cancel();
            }
        }),
    );
    let manager = DownloadManager::new(&DownloadConfig::default()).unwrap();
    let result = manager
        .download(
            DownloadRequest::new(vec![format!("http://{address}/content.jar")], &destination)
                .with_size(100)
                .with_task_context(context),
        )
        .await;

    assert!(matches!(result, Err(DownloadError::Cancelled)));
    assert!(observed_progress.load(Ordering::Acquire));
    assert_eq!(fs::read(&destination).unwrap(), b"keep-existing");
    server.join().unwrap();
    fs::remove_dir_all(temporary).unwrap();
}
#[test]
fn dependency_types_pins_and_incompatible_versions_are_not_flattened() {
    let parsed:MrVersion=serde_json::from_value(serde_json::json!({"id":"a1","project_id":"a","name":"a","version_number":"1","game_versions":["1.21.1"],"loaders":["fabric"],"files":[],"dependencies":[{"project_id":"dep","version_id":"d1","dependency_type":"required"},{"project_id":"optional","dependency_type":"optional"},{"project_id":"bad","dependency_type":"incompatible"}]})).unwrap();
    let a = from_mr_version(parsed);
    assert_eq!(a.dependencies, vec!["dep"]);
    assert_eq!(a.optional_dependencies, vec!["optional"]);
    assert_eq!(a.pinned_dependencies["dep"], "d1");
    let dep = version("dep", "d2", "dep.jar", b"d2");
    let bad = version("bad", "bad1", "bad.jar", b"bad");
    let conflicts = OnlineContentService::analyze_conflicts(&[a, dep, bad]);
    assert!(conflicts.iter().any(|c| c.path == "pinned-version:dep"));
    assert!(conflicts.iter().any(|c| c.path == "incompatible:bad"));
}

#[test]
fn conflict_analysis_keeps_provider_dependency_namespaces_separate() {
    let mut modrinth = version("same-id", "m1", "shared.jar", b"modrinth");
    modrinth.dependencies.push("needed-only-on-modrinth".into());
    modrinth
        .incompatible_dependencies
        .push("blocked-only-on-modrinth".into());
    let curseforge = VersionHit {
        provider: Provider::CurseForge,
        ..version("same-id", "m1", "shared.jar", b"curseforge")
    };
    let other = VersionHit {
        provider: Provider::CurseForge,
        ..version("needed-only-on-modrinth", "c1", "dep.jar", b"dependency")
    };
    let blocked = VersionHit {
        provider: Provider::CurseForge,
        ..version("blocked-only-on-modrinth", "c2", "blocked.jar", b"blocked")
    };
    let conflicts =
        OnlineContentService::analyze_conflicts(&[modrinth, curseforge, other, blocked]);
    assert!(
        conflicts
            .iter()
            .any(|c| c.path == "missing-required:needed-only-on-modrinth")
    );
    assert!(
        !conflicts
            .iter()
            .any(|c| c.path.starts_with("incompatible:"))
    );
    assert!(
        !conflicts
            .iter()
            .any(|c| c.path.starts_with("multiple-versions:"))
    );
}
#[tokio::test]
async fn offline_online_operations_fail_before_api_requests() {
    let client = OnlineContentService::new(OnlineConfig {
        offline: true,
        ..Default::default()
    })
    .unwrap()
    .with_endpoints("http://127.0.0.1:1", "http://127.0.0.1:1");
    assert!(matches!(
        client.versions(Provider::Modrinth, "p", None, None).await,
        Err(OnlineError::Offline)
    ));
    assert!(matches!(
        client
            .search(
                Provider::CurseForge,
                &SearchQuery {
                    query: "test".into(),
                    ..Default::default()
                }
            )
            .await,
        Err(OnlineError::Offline)
    ));
}
#[test]
fn filename_validation_rejects_cross_platform_and_path_escapes() {
    for name in [
        "../x.jar", "a/b.jar", "CON.jar", "a:b.jar", "a.jar.", "a.jar ", "x\\y.jar",
    ] {
        assert!(validate_mod_name(name).is_err(), "{name}");
    }
    assert!(validate_mod_name("ordinary-1.0.jar").is_ok());
    assert_eq!(Path::new("mods/a.jar").file_name().unwrap(), "a.jar");
}

#[test]
fn modrinth_search_and_version_fixtures_preserve_dependency_kinds_and_pins() {
    let search:MrSearch=serde_json::from_value(serde_json::json!({"hits":[{"project_id":"p-id","slug":"fabric-api","title":"Fabric API","description":"fixture","downloads":12,"icon_url":null}]})).unwrap();
    let projects = project_hits_from_modrinth(search, Provider::Modrinth);
    assert_eq!(projects[0].slug, "fabric-api");
    assert_eq!(projects[0].downloads, 12);
    let fixture:MrVersion=serde_json::from_value(serde_json::json!({"id":"root-v2","project_id":"root","name":"Root","version_number":"2.0","game_versions":["1.21.1"],"loaders":["fabric"],"files":[{"filename":"root.jar","url":"https://cdn.modrinth.com/root.jar","size":5,"hashes":{"sha1":"abc","sha512":"def"},"primary":true}],"dependencies":[{"project_id":"dep","version_id":"dep-v1","dependency_type":"required"},{"project_id":"addon","version_id":null,"dependency_type":"optional"},{"project_id":"broken","version_id":null,"dependency_type":"incompatible"},{"project_id":"embedded","dependency_type":"embedded"}]})).unwrap();
    let version = from_mr_version(fixture);
    assert_eq!(version.dependencies, vec!["dep"]);
    assert_eq!(version.optional_dependencies, vec!["addon"]);
    assert_eq!(version.incompatible_dependencies, vec!["broken"]);
    assert_eq!(version.pinned_dependencies["dep"], "dep-v1");
    assert_eq!(version.files[0].sha512.as_deref(), Some("def"));
}

#[test]
fn curseforge_version_fixture_maps_hashes_relationships_and_loader_tags() {
    let fixture:CfEnvelope<Vec<CfVersion>>=serde_json::from_value(serde_json::json!({"data":[{"id":123,"displayName":"Build 7","fileName":"example.jar","fileLength":99,"downloadUrl":"https://mediafilez.forgecdn.net/files/example.jar","gameVersions":["1.20.1","Fabric"],"hashes":[{"value":"sha1-value","algo":1}],"dependencies":[{"modId":10,"relationType":3},{"modId":11,"relationType":2},{"modId":12,"relationType":5}]}]})).unwrap();
    let version = from_cf_version("42", fixture.data.into_iter().next().unwrap());
    assert_eq!(version.provider, Provider::CurseForge);
    assert_eq!(version.dependencies, vec!["10"]);
    assert_eq!(version.optional_dependencies, vec!["11"]);
    assert_eq!(version.incompatible_dependencies, vec!["12"]);
    assert_eq!(version.game_versions, vec!["1.20.1"]);
    assert_eq!(version.loaders, vec!["fabric"]);
    assert_eq!(version.files[0].sha1.as_deref(), Some("sha1-value"));
    let restricted: CfVersion = serde_json::from_value(serde_json::json!({"id":124,"displayName":"Restricted","fileName":"restricted.jar","fileLength":10,"downloadUrl":null,"gameVersions":["1.20.1"],"hashes":[{"value":"sha1","algo":1}]})).unwrap();
    assert!(from_cf_version("42", restricted).files.is_empty());
}

#[test]
fn curseforge_auth_header_is_caller_supplied_and_debug_output_redacts_it() {
    let key = "fixture-user-secret";
    let config = OnlineConfig {
        curseforge_api_key: Some(key.into()),
        ..Default::default()
    };
    assert!(!format!("{config:?}").contains(key));
    let service = OnlineContentService::new(config).unwrap();
    let request = service
        .authorize_curseforge(
            reqwest::Client::new().get("https://api.curseforge.com/v1/mods/search"),
        )
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(request.headers().get("x-api-key").unwrap(), key);
    let without = OnlineContentService::new(OnlineConfig::default()).unwrap();
    assert!(matches!(
        without.authorize_curseforge(
            reqwest::Client::new().get("https://api.curseforge.com/v1/mods/search")
        ),
        Err(OnlineError::MissingCurseForgeKey)
    ));
}

#[test]
fn update_selection_uses_the_newest_provider_result() {
    let latest = version("p", "v3", "mod.jar", b"latest");
    let older = version("p", "v2", "mod.jar", b"older");
    assert_eq!(
        latest_update(vec![latest.clone(), older], "v2").unwrap().id,
        "v3"
    );
    assert!(latest_update(vec![latest.clone()], "v3").is_none());
}

fn serve_fixture(routes: Vec<(&'static str, &'static str)>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        for (expected_path, body) in routes {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            assert!(request_line.contains(expected_path), "{request_line}");
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (format!("http://{address}"), worker)
}

#[tokio::test]
async fn modrinth_fixture_server_exercises_search_versions_and_recursive_pinned_resolution() {
    let (root_body, dep_body) = (
        r#"[{"id":"root-v2","project_id":"root","name":"Root","version_number":"2.0","game_versions":["1.21.1"],"loaders":["fabric"],"files":[],"dependencies":[{"project_id":"dep","version_id":"dep-v1","dependency_type":"required"}]}]"#,
        r#"{"id":"dep-v1","project_id":"dep","name":"Dependency","version_number":"1.0","game_versions":["1.21.1"],"loaders":["fabric"],"files":[],"dependencies":[]}"#,
    );
    let (base, worker) = serve_fixture(vec![
        (
            "/search",
            r#"{"hits":[{"project_id":"root","slug":"root","title":"Root","description":"fixture","downloads":1,"icon_url":null}]}"#,
        ),
        ("/project/root/version", root_body),
        ("/version/dep-v1", dep_body),
    ]);
    let service = OnlineContentService::new(OnlineConfig::default())
        .unwrap()
        .with_endpoints(&base, &base);
    let hits = service
        .search(
            Provider::Modrinth,
            &SearchQuery {
                query: "root".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(hits[0].id, "root");
    let graph = service
        .resolve_dependencies("root", "1.21.1", "fabric")
        .await
        .unwrap();
    assert_eq!(graph.len(), 2);
    assert_eq!(
        graph.iter().find(|v| v.project_id == "dep").unwrap().id,
        "dep-v1"
    );
    worker.join().unwrap();
}
