use ccl_core::online_content::Provider;
use ccl_core::task::{CancellationToken, TaskContext};
use ccl_gui::online_service::{OnlineMutation, OnlineQuery};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::Arc,
    thread,
    time::Duration,
};

fn fixture(body: &'static str, status: &'static str, delay: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = String::new();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let _ = reader.read_line(&mut request);
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                    break;
                }
                headers.push_str(&line);
            }
            let _ = headers;
            thread::sleep(delay);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    format!("http://{address}")
}

fn fixture_sequence(routes: Vec<(&'static str, &'static str)>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        for (expected_path, body) in routes {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            assert!(request.contains(expected_path), "{request}");
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (format!("http://{address}"), worker)
}

fn test_context(token: CancellationToken) -> TaskContext {
    TaskContext::new(token, Arc::new(|_| {}))
}

fn create_instance(root: &std::path::Path) -> std::path::PathBuf {
    let root = std::fs::canonicalize(root).unwrap();
    let game = root.join(".ccl/instances/test/game");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(
        game.parent().unwrap().join("instance.json"),
        serde_json::json!({
            "id":"test", "minecraft":"1.21.1", "loader":null,
            "loader_version":null, "game_directory":game
        })
        .to_string(),
    )
    .unwrap();
    root
}

#[test]
fn typed_online_queries_accept_cli_equivalent_search_filters() {
    let request: OnlineQuery = serde_json::from_value(serde_json::json!({
        "operation": "search",
        "provider": "modrinth",
        "query": "sodium",
        "minecraft": "1.21.1",
        "loader": ["fabric"],
        "kind": "mod",
        "limit": 40
    }))
    .unwrap();
    assert!(matches!(request, OnlineQuery::Search {
        provider: Provider::Modrinth,
        query,
        minecraft: Some(ref version),
        ref loader,
        ref kind,
        limit: 40,
    } if query == "sodium" && version == "1.21.1" && loader == &["fabric"] && kind == "mod"));
}

#[test]
fn online_mutations_are_typed_and_reject_unknown_fields() {
    let install: OnlineMutation = serde_json::from_value(serde_json::json!({
        "operation": "install",
        "id": "instance",
        "provider": "curseforge",
        "project": "12345",
        "version": "67890"
    }))
    .unwrap();
    assert!(matches!(install, OnlineMutation::Install {
        provider: Provider::CurseForge,
        ref project,
        ref version,
        ..
    } if project == "12345" && version.as_deref() == Some("67890")));
    assert!(
        serde_json::from_value::<OnlineMutation>(serde_json::json!({
            "operation": "install", "id": "i", "provider": "modrinth", "project": "p", "extra": true
        }))
        .is_err()
    );
}

#[test]
fn search_defaults_match_cli_defaults() {
    let request: OnlineQuery = serde_json::from_value(serde_json::json!({
        "operation": "search", "provider": "modrinth", "query": "fabric api"
    }))
    .unwrap();
    assert!(
        matches!(request, OnlineQuery::Search { limit: 20, kind, loader, .. }
        if kind == "mod" && loader.is_empty())
    );
}

#[tokio::test]
async fn search_uses_local_provider_fixture_and_returns_cli_result_shape() {
    let endpoint = fixture(
        r#"{"hits":[{"project_id":"sodium-id","slug":"sodium","title":"Sodium","description":"Fast rendering","downloads":10,"icon_url":null}]}"#,
        "200 OK",
        Duration::ZERO,
    );
    let temp = tempfile::tempdir().unwrap();
    let service = ccl_gui::online_service::OnlineService::new(
        temp.path().into(),
        temp.path().join("config.json"),
    )
    .unwrap()
    .with_endpoints(&endpoint, &endpoint);
    let value = service
        .query(OnlineQuery::Search {
            provider: Provider::Modrinth,
            query: "sodium".into(),
            minecraft: Some("1.21.1".into()),
            loader: vec!["fabric".into()],
            kind: "mod".into(),
            limit: 20,
        })
        .await
        .unwrap();
    assert_eq!(value["projects"][0]["id"], "sodium-id");
    assert_eq!(value["projects"][0]["title"], "Sodium");
}

#[tokio::test]
async fn resolve_uses_local_fixture_and_reports_required_graph_and_conflicts() {
    let endpoint = fixture(
        r#"[{"id":"root-v1","project_id":"root","name":"Root","version_number":"1","game_versions":["1.21.1"],"loaders":["fabric"],"files":[],"dependencies":[]}]"#,
        "200 OK",
        Duration::ZERO,
    );
    let temp = tempfile::tempdir().unwrap();
    let service = ccl_gui::online_service::OnlineService::new(
        temp.path().into(),
        temp.path().join("config.json"),
    )
    .unwrap()
    .with_endpoints(&endpoint, &endpoint);
    let value = service
        .query(OnlineQuery::Resolve {
            provider: Provider::Modrinth,
            project: "root".into(),
            version: None,
            minecraft: "1.21.1".into(),
            loader: Some("fabric".into()),
        })
        .await
        .unwrap();
    assert_eq!(value["resolved"].as_array().unwrap().len(), 1);
    assert!(value["conflicts"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn resolve_plan_uses_the_selected_version_and_its_pinned_dependency() {
    let roots = r#"[
        {"id":"root-v2","project_id":"root","name":"Root 2","version_number":"2","game_versions":["1.21.1"],"loaders":["fabric"],"files":[],"dependencies":[{"project_id":"dep","version_id":"dep-v2","dependency_type":"required"}]},
        {"id":"root-v1","project_id":"root","name":"Root 1","version_number":"1","game_versions":["1.21.1"],"loaders":["fabric"],"files":[],"dependencies":[{"project_id":"dep","version_id":"dep-v1","dependency_type":"required"}]}
    ]"#;
    let dependency = r#"{"id":"dep-v1","project_id":"dep","name":"Dependency 1","version_number":"1","game_versions":["1.21.1"],"loaders":["fabric"],"files":[],"dependencies":[]}"#;
    let (endpoint, worker) = fixture_sequence(vec![
        ("/project/root/version", roots),
        ("/version/dep-v1", dependency),
    ]);
    let temp = tempfile::tempdir().unwrap();
    let service = ccl_gui::online_service::OnlineService::new(
        temp.path().into(),
        temp.path().join("config.json"),
    )
    .unwrap()
    .with_endpoints(&endpoint, &endpoint);
    let plan = service
        .query(OnlineQuery::Resolve {
            provider: Provider::Modrinth,
            project: "root".into(),
            version: Some("root-v1".into()),
            minecraft: "1.21.1".into(),
            loader: Some("fabric".into()),
        })
        .await
        .unwrap();
    let versions = plan["resolved"].as_array().unwrap();
    assert_eq!(versions[0]["id"], "root-v1");
    assert_eq!(versions[1]["id"], "dep-v1");
    worker.join().unwrap();
}

#[tokio::test]
async fn install_reports_unavailable_pinned_version_from_provider_fixture() {
    let endpoint = fixture("[]", "200 OK", Duration::ZERO);
    let temp = tempfile::tempdir().unwrap();
    let root = create_instance(temp.path());
    let service =
        ccl_gui::online_service::OnlineService::new(root, temp.path().join("config.json"))
            .unwrap()
            .with_endpoints(&endpoint, &endpoint);
    let error = service
        .mutate(
            OnlineMutation::Install {
                id: "test".into(),
                provider: Provider::Modrinth,
                project: "missing".into(),
                version: Some("missing-v1".into()),
            },
            test_context(CancellationToken::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid_online_request", "{error:?}");
    assert!(error.message.contains("compatible"));
}

#[tokio::test]
async fn install_cancellation_interrupts_a_stalled_provider_request() {
    let endpoint = fixture("[]", "200 OK", Duration::from_secs(2));
    let temp = tempfile::tempdir().unwrap();
    let root = create_instance(temp.path());
    let service =
        ccl_gui::online_service::OnlineService::new(root, temp.path().join("config.json"))
            .unwrap()
            .with_endpoints(&endpoint, &endpoint);
    let token = CancellationToken::default();
    let cancellation = token.clone();
    let work = tokio::spawn(async move {
        service
            .mutate(
                OnlineMutation::Install {
                    id: "test".into(),
                    provider: Provider::Modrinth,
                    project: "project".into(),
                    version: None,
                },
                test_context(token),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(80)).await;
    cancellation.cancel();
    let error = tokio::time::timeout(Duration::from_millis(700), work)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "cancelled", "{error:?}");
}
