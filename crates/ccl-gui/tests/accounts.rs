use ccl_core::{
    accounts::{MemorySecretStore, SecretStore},
    auth::AuthEndpoints,
    config::LauncherConfig,
    task::CancellationToken,
};
use ccl_gui::account_service::{AccountService, DeviceAuthorization};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
    thread,
    time::Duration,
};

const UUID: &str = "0123456789abcdef0123456789abcdef";

fn fixture(replies: Vec<&'static str>) -> (AuthEndpoints, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(false).unwrap();
    let worker = thread::spawn(move || {
        for body in replies {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            let header_end = loop {
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0, "request ended before headers");
                request.extend_from_slice(&buf[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            while request.len() < header_end + content_length {
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0, "request ended before body");
                request.extend_from_slice(&buf[..n]);
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    let endpoint = |path: &str| format!("http://{addr}/{path}").parse().unwrap();
    (
        AuthEndpoints {
            device_code: endpoint("device"),
            token: endpoint("token"),
            xbox_live: endpoint("xbox"),
            xsts: endpoint("xsts"),
            minecraft: endpoint("minecraft"),
            entitlements: endpoint("entitlements"),
            profile: endpoint("profile"),
        },
        worker,
    )
}

fn successful_login_fixture() -> (AuthEndpoints, thread::JoinHandle<()>) {
    fixture(vec![
        r#"{"device_code":"device-code-sentinel","user_code":"ABCD-EFGH","verification_uri":"https://microsoft.com/devicelogin","message":"Sign in","expires_in":120,"interval":1}"#,
        r#"{"access_token":"oauth-access-sentinel","refresh_token":"oauth-refresh-sentinel"}"#,
        r#"{"Token":"xbox-token","DisplayClaims":{"xui":[{"uhs":"12345"}]}}"#,
        r#"{"Token":"xsts-token","DisplayClaims":{"xui":[{"uhs":"12345","xid":"987654321"}]}}"#,
        r#"{"access_token":"minecraft-access-sentinel","expires_in":3600}"#,
        r#"{"items":[{"name":"product_minecraft"}]}"#,
        r#"{"id":"0123456789abcdef0123456789abcdef","name":"FixturePlayer"}"#,
    ])
}

fn config(offline: bool) -> LauncherConfig {
    let mut config = LauncherConfig::default();
    config.auth.microsoft_client_id = Some("fixture-client".into());
    config.downloads.offline = offline;
    config
}

#[tokio::test]
async fn mock_login_persists_only_metadata_and_supports_selection_logout_and_offline_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let secrets = Arc::new(MemorySecretStore::default());
    let (endpoints, worker) = successful_login_fixture();
    let service =
        AccountService::with_auth_endpoints(&path, secrets.clone(), config(false), endpoints);
    let cancel = CancellationToken::default();

    let (login, authorization) = service
        .begin_login(None, &cancel)
        .await
        .expect("device-code request succeeds");
    let auth_json = serde_json::to_string(&authorization).unwrap();
    for secret in [
        "device-code-sentinel",
        "access_token",
        "refresh_token",
        "oauth-access-sentinel",
    ] {
        assert!(!auth_json.contains(secret));
    }
    assert_eq!(authorization.user_code, "ABCD-EFGH");
    assert_eq!(authorization.expires_in, 120);

    let metadata = service.complete_login(login, &cancel).await.unwrap();
    assert_eq!(metadata.uuid, UUID);
    assert_eq!(metadata.username, "FixturePlayer");
    worker.join().unwrap();

    let index = std::fs::read_to_string(dir.path().join("accounts.json")).unwrap();
    for secret in [
        "device-code-sentinel",
        "oauth-access-sentinel",
        "oauth-refresh-sentinel",
        "minecraft-access-sentinel",
    ] {
        assert!(!index.contains(secret));
    }
    let stored = secrets.get(UUID).unwrap().unwrap();
    assert_eq!(stored.access_token(), "minecraft-access-sentinel");
    assert_eq!(stored.refresh_token(), "oauth-refresh-sentinel");

    service.select(UUID).unwrap();
    let listing = service.list().unwrap();
    assert_eq!(listing.selected.as_deref(), Some(UUID));
    assert_eq!(listing.accounts, vec![metadata]);

    let offline = AccountService::new(&path, secrets.clone(), config(true));
    let identity = offline.launch_identity(UUID, &cancel).await.unwrap();
    assert_eq!(identity.username, "FixturePlayer");
    assert_eq!(identity.uuid, UUID);

    offline.logout(UUID).unwrap();
    assert_eq!(offline.list().unwrap().selected, None);
    assert!(offline.list().unwrap().accounts.is_empty());
    assert!(secrets.get(UUID).unwrap().is_none());
}

#[tokio::test]
async fn cancellation_and_configuration_errors_are_mapped_without_network_access() {
    let dir = tempfile::tempdir().unwrap();
    let service = AccountService::new(
        dir.path().join("config.json"),
        Arc::new(MemorySecretStore::default()),
        LauncherConfig::default(),
    );
    let err = service
        .begin_login(None, &CancellationToken::default())
        .await
        .err()
        .unwrap();
    assert_eq!(err.code, "auth_config");

    let mut configured = config(false);
    let service = AccountService::new(
        dir.path().join("other.json"),
        Arc::new(MemorySecretStore::default()),
        configured.clone(),
    );
    let cancelled = CancellationToken::default();
    cancelled.cancel();
    let err = service.begin_login(None, &cancelled).await.err().unwrap();
    assert_eq!(err.code, "cancelled");

    configured.downloads.offline = true;
    let offline = AccountService::new(
        dir.path().join("offline.json"),
        Arc::new(MemorySecretStore::default()),
        configured,
    );
    let err = offline
        .begin_login(None, &CancellationToken::default())
        .await
        .err()
        .unwrap();
    assert_eq!(err.code, "offline");
}

#[test]
fn authorization_dto_contains_only_frontend_safe_fields() {
    let dto = DeviceAuthorization {
        user_code: "ABCD-EFGH".into(),
        verification_uri: "https://microsoft.com/devicelogin".into(),
        expires_in: 900,
    };
    let encoded = serde_json::to_string(&dto).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&encoded).unwrap(),
        serde_json::json!({
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://microsoft.com/devicelogin",
            "expires_in": 900
        })
    );
    for forbidden in [
        "device_code",
        "access_token",
        "refresh_token",
        "secret-sentinel",
    ] {
        assert!(!encoded.contains(forbidden));
    }
}
