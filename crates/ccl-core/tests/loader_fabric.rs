use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use ccl_core::{
    config::{DownloadConfig, SourceMode},
    install::InstallOptions,
    loaders::{LoaderError, LoaderKind, LoaderRequest, LoaderService},
};
use sha1::{Digest, Sha1};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn sha1(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}

async fn fixture_server(
    bad_library_hash: bool,
) -> (String, tokio::task::JoinHandle<()>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base = format!("http://{address}");
    let routes = fixture_routes(&base, bad_library_hash);
    let profile_requests = Arc::new(AtomicUsize::new(0));
    let server_profile_requests = profile_requests.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let routes = routes.clone();
            let profile_requests = server_profile_requests.clone();
            tokio::spawn(async move {
                let mut request = vec![0; 4096];
                let n = stream.read(&mut request).await.unwrap_or(0);
                let first = String::from_utf8_lossy(&request[..n])
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_owned();
                let path = first.split_whitespace().nth(1).unwrap_or("/");
                let mut body = routes.get(path).cloned();
                if path.ends_with("/profile/json")
                    && let Some(bytes) = &mut body
                {
                    let request = profile_requests.fetch_add(1, Ordering::Relaxed) + 1;
                    let mut profile: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                    let timestamp = format!("2026-10-22T00:00:{request:02}+0000");
                    profile["time"] = timestamp.clone().into();
                    profile["releaseTime"] = timestamp.into();
                    *bytes = serde_json::to_vec(&profile).unwrap();
                }
                let (status, body) = body
                    .map(|b| ("200 OK", b))
                    .unwrap_or_else(|| ("404 Not Found", b"missing".to_vec()));
                let headers = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(headers.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            });
        }
    });
    (base, task, profile_requests)
}

fn fixture_routes(base: &str, bad_library_hash: bool) -> HashMap<String, Vec<u8>> {
    let client = b"vanilla-client-fixture";
    let library = b"fabric-loader-library-fixture";
    let index = br#"{"objects":{}}"#;
    let library_hash = if bad_library_hash {
        "0000000000000000000000000000000000000000".to_owned()
    } else {
        sha1(library)
    };
    let vanilla = serde_json::json!({
        "id":"1.21.4",
        "downloads":{"client":{"url":format!("{base}/client.jar"),"sha1":sha1(client),"size":client.len()}},
        "assetIndex":{"id":"fixture","url":format!("{base}/assets.json"),"sha1":sha1(index),"size":index.len()}
    });
    let vanilla_bytes = serde_json::to_vec(&vanilla).unwrap();
    let manifest = serde_json::json!({"latest":{"release":"1.21.4"},"versions":[{
        "id":"1.21.4","type":"release","url":format!("{base}/version.json"),"sha1":sha1(&vanilla_bytes)
    }]});
    let library_entry = if bad_library_hash {
        serde_json::json!({"name":"net.fabricmc:fabric-loader:0.16.10","url":format!("{base}/maven/"),"sha1":library_hash,"size":library.len()})
    } else {
        serde_json::json!({"name":"net.fabricmc:fabric-loader:0.16.10","url":format!("{base}/maven/"),"size":library.len()})
    };
    let profile = serde_json::json!({
        "id":"fabric-loader-0.16.10-1.21.4",
        "inheritsFrom":"1.21.4",
        "type":"release",
        "mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient",
        "arguments":{"game":[]},
        "libraries":[library_entry]
    });
    let mut routes = HashMap::new();
    routes.insert(
        "/mc-version-manifest.json".into(),
        serde_json::to_vec(&manifest).unwrap(),
    );
    routes.insert("/version.json".into(), vanilla_bytes);
    routes.insert("/client.jar".into(), client.to_vec());
    routes.insert("/assets.json".into(), index.to_vec());
    routes.insert(
        "/fabric/v2/versions/loader/1.21.4".into(),
        serde_json::to_vec(&serde_json::json!([{"loader":{"version":"0.16.10","stable":true}}]))
            .unwrap(),
    );
    routes.insert(
        "/fabric/v2/versions/loader/1.21.4/0.16.10/profile/json".into(),
        serde_json::to_vec(&profile).unwrap(),
    );
    routes.insert(
        "/maven/net/fabricmc/fabric-loader/0.16.10/fabric-loader-0.16.10.jar".into(),
        library.to_vec(),
    );
    routes.insert(
        "/maven/net/fabricmc/fabric-loader/0.16.10/fabric-loader-0.16.10.jar.sha1".into(),
        sha1(library).into_bytes(),
    );
    routes
}

fn service(base: &str) -> LoaderService {
    let config = DownloadConfig {
        version_list_source: SourceMode::Official,
        file_source: SourceMode::Official,
        ..DownloadConfig::default()
    };
    LoaderService::new(config).unwrap().with_fabric_endpoints(
        format!("{base}/fabric"),
        format!("{base}/mc-version-manifest.json"),
    )
}

fn temp_game_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ccl-loader-fabric-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

#[tokio::test]
async fn installs_vanilla_and_official_fabric_profile_with_verified_maven_library() {
    let (base, server, _) = fixture_server(false).await;
    let game = temp_game_dir("success");
    let options = InstallOptions::for_current_platform(&game);
    let request = LoaderRequest {
        kind: LoaderKind::Fabric,
        minecraft: "1.21.4".into(),
        version: "0.16.10".into(),
    };
    let result = service(&base)
        .install(&request, &options, None)
        .await
        .unwrap();
    assert_eq!(result, "fabric-loader-0.16.10-1.21.4");
    let lib = game.join("libraries/net/fabricmc/fabric-loader/0.16.10/fabric-loader-0.16.10.jar");
    assert_eq!(
        std::fs::read(lib).unwrap(),
        b"fabric-loader-library-fixture"
    );
    assert!(game.join("versions/1.21.4/1.21.4.json").is_file());
    let profile_path =
        game.join("versions/fabric-loader-0.16.10-1.21.4/fabric-loader-0.16.10-1.21.4.json");
    let profile: serde_json::Value =
        serde_json::from_slice(&std::fs::read(profile_path).unwrap()).unwrap();
    assert_eq!(
        profile["libraries"][0]["downloads"]["artifact"]["path"],
        "net/fabricmc/fabric-loader/0.16.10/fabric-loader-0.16.10.jar"
    );
    assert_eq!(
        profile["libraries"][0]["downloads"]["artifact"]["sha1"],
        sha1(b"fabric-loader-library-fixture")
    );
    let _ = std::fs::remove_dir_all(game);
    server.abort();
}

#[tokio::test]
async fn reinstalling_same_fabric_profile_ignores_refreshed_timestamps() {
    let (base, server, profile_requests) = fixture_server(false).await;
    let game = temp_game_dir("same-profile-reinstall");
    let options = InstallOptions::for_current_platform(&game);
    let request = LoaderRequest {
        kind: LoaderKind::Fabric,
        minecraft: "1.21.4".into(),
        version: "0.16.10".into(),
    };
    let loader = service(&base);

    assert_eq!(
        loader.install(&request, &options, None).await.unwrap(),
        "fabric-loader-0.16.10-1.21.4"
    );
    let profile_path =
        game.join("versions/fabric-loader-0.16.10-1.21.4/fabric-loader-0.16.10-1.21.4.json");
    let first: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&profile_path).unwrap()).unwrap();
    // Reformat and reorder object keys without changing the profile's meaning.
    std::fs::write(&profile_path, serde_json::to_vec(&first).unwrap()).unwrap();

    assert_eq!(
        loader.install(&request, &options, None).await.unwrap(),
        "fabric-loader-0.16.10-1.21.4"
    );
    let second: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&profile_path).unwrap()).unwrap();
    assert_eq!(profile_requests.load(Ordering::Relaxed), 2);
    // The second response has a refreshed timestamp, but an equivalent installed profile is
    // intentionally left in place rather than rewritten during an idempotent reinstall.
    assert_eq!(first["time"], second["time"]);
    assert_eq!(first["releaseTime"], second["releaseTime"]);
    assert_eq!(first["mainClass"], second["mainClass"]);

    let _ = std::fs::remove_dir_all(game);
    server.abort();
}

#[tokio::test]
async fn reinstall_still_rejects_different_fabric_launch_profile() {
    let (base, server, _) = fixture_server(false).await;
    let game = temp_game_dir("different-profile-conflict");
    let options = InstallOptions::for_current_platform(&game);
    let request = LoaderRequest {
        kind: LoaderKind::Fabric,
        minecraft: "1.21.4".into(),
        version: "0.16.10".into(),
    };
    let loader = service(&base);
    loader.install(&request, &options, None).await.unwrap();
    let profile_path =
        game.join("versions/fabric-loader-0.16.10-1.21.4/fabric-loader-0.16.10-1.21.4.json");
    let mut existing: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&profile_path).unwrap()).unwrap();
    existing["mainClass"] = "changed.launcher.Main".into();
    std::fs::write(&profile_path, serde_json::to_vec_pretty(&existing).unwrap()).unwrap();

    let result = loader.install(&request, &options, None).await;
    assert!(matches!(result, Err(LoaderError::Conflict(_))));
    let after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&profile_path).unwrap()).unwrap();
    assert_eq!(after["mainClass"], "changed.launcher.Main");

    let _ = std::fs::remove_dir_all(game);
    server.abort();
}

#[tokio::test]
async fn rejects_incompatible_loader_before_creating_game_files() {
    let (base, server, _) = fixture_server(false).await;
    let game = temp_game_dir("incompatible");
    let request = LoaderRequest {
        kind: LoaderKind::Fabric,
        minecraft: "1.21.4".into(),
        version: "0.16.9".into(),
    };
    let result = service(&base)
        .install(&request, &InstallOptions::for_current_platform(&game), None)
        .await;
    assert!(matches!(result, Err(LoaderError::Unsupported(_))));
    assert!(
        !game.exists(),
        "incompatible versions must be rejected before vanilla files are staged"
    );
    server.abort();
}

#[tokio::test]
async fn rejects_a_library_with_a_wrong_sha1() {
    let (base, server, _) = fixture_server(true).await;
    let game = temp_game_dir("bad-sha1");
    let request = LoaderRequest {
        kind: LoaderKind::Fabric,
        minecraft: "1.21.4".into(),
        version: "0.16.10".into(),
    };
    let result = service(&base)
        .install(&request, &InstallOptions::for_current_platform(&game), None)
        .await;
    assert!(matches!(result, Err(LoaderError::Download(_))));
    assert!(
        !game
            .join("libraries/net/fabricmc/fabric-loader/0.16.10/fabric-loader-0.16.10.jar")
            .exists()
    );
    let _ = std::fs::remove_dir_all(game);
    server.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_a_symlink_as_the_staging_game_directory() {
    let target = temp_game_dir("symlink-target");
    let link = temp_game_dir("symlink");
    std::fs::create_dir_all(&target).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let request = LoaderRequest {
        kind: LoaderKind::Fabric,
        minecraft: "1.21.4".into(),
        version: "0.16.10".into(),
    };
    let result = service("http://127.0.0.1:1")
        .install(&request, &InstallOptions::for_current_platform(&link), None)
        .await;
    assert!(matches!(result, Err(LoaderError::Conflict(_))));
    assert!(std::fs::read_dir(&target).unwrap().next().is_none());
    std::fs::remove_file(&link).unwrap();
    let _ = std::fs::remove_dir_all(target);
}
