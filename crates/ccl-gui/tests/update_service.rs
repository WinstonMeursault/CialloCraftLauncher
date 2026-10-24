#![cfg(unix)]

use ccl_core::{CancellationToken, config::DownloadConfig, update::BuildIdentity};
use ccl_gui::update_service::{UpdateGuiService, UpdateRequest, UpdateResponse};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

fn fixture_cli(root: &Path, body: &str) {
    fs::create_dir_all(root).unwrap();
    let executable = root.join("ccl");
    fs::write(
        &executable,
        format!("#!/bin/sh\nif [ \"$1\" = \"__build-identity\" ]; then\n  printf '%s' '{body}'\n  exit 0\nfi\nexit 64\n"),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
}

fn service() -> UpdateGuiService {
    UpdateGuiService::new(
        DownloadConfig::default(),
        Path::new("/tmp/ccl-gui-update-config.json").to_path_buf(),
        Path::new("/tmp/ccl-gui-update-game").to_path_buf(),
    )
}

#[tokio::test]
async fn status_reads_identity_from_the_selected_cli_executable() {
    let temp = tempfile::tempdir().unwrap();
    let expected = BuildIdentity::current();
    fixture_cli(temp.path(), &serde_json::to_string(&expected).unwrap());

    let response = service()
        .execute(
            UpdateRequest::Status {
                installation_root: temp.path().to_path_buf(),
            },
            CancellationToken::default(),
        )
        .await
        .unwrap();

    match response {
        UpdateResponse::Status {
            build,
            update_status,
        } => {
            assert_eq!(build, expected);
            assert!(update_status.is_none());
        }
        other => panic!("unexpected response: {other:?}"),
    }
}

#[tokio::test]
async fn status_rejects_an_executable_without_a_ccl_build_identity() {
    let temp = tempfile::tempdir().unwrap();
    fixture_cli(temp.path(), "not-json");

    let error = service()
        .execute(
            UpdateRequest::Status {
                installation_root: temp.path().to_path_buf(),
            },
            CancellationToken::default(),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, "update_error");
    assert!(error.message.contains("build identity unavailable"));
}

#[tokio::test]
async fn recovery_with_missing_target_reaches_the_core_journal_path() {
    let temp = tempfile::tempdir().unwrap();

    let error = service()
        .execute(
            UpdateRequest::Recover {
                installation_root: temp.path().to_path_buf(),
            },
            CancellationToken::default(),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, "update_error");
    assert!(!error.message.contains("build identity unavailable"));
}
