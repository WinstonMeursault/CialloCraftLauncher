use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    thread,
};
use zip::{ZipWriter, write::SimpleFileOptions};
static ARCHIVE_ID: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "ccl-import-regression-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn archive(&self, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = self.0.join(format!(
            "input-{}.zip",
            ARCHIVE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let mut zip = ZipWriter::new(fs::File::create(&path).unwrap());
        for (name, bytes) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
const MMC:&[u8]=br#"{"components":[{"uid":"net.minecraft","version":"1.21.1"},{"uid":"net.fabricmc.fabric-loader","version":"0.20.1"}]}"#;
#[test]
fn nested_prism_export_preserves_worlds_options_and_configuration() {
    let f = Fixture::new();
    let input = f.archive(&[
        ("Example/mmc-pack.json", MMC),
        ("Example/.minecraft/saves/World/level.dat", b"world"),
        ("Example/.minecraft/options.txt", b"music:0"),
        ("Example/.minecraft/config/config.toml", b"value=1"),
        ("Example/instance.cfg", b"PreLaunchCommand=do-not-run"),
    ]);
    let output = f.0.join("converted.mrpack");
    convert_multimc(&input, &output).unwrap();
    let pack = modpack_format::read_pack(&output).unwrap();
    assert_eq!(pack.overrides["saves/World/level.dat"], b"world");
    assert_eq!(pack.overrides["options.txt"], b"music:0");
    assert_eq!(pack.overrides.len(), 3);
    assert_eq!(
        ImportFormatService::inspect(&input).unwrap()["format"],
        "multimc-prism"
    );
}
#[test]
fn unsupported_custom_component_is_not_silently_ignored() {
    let f = Fixture::new();
    let input=f.archive(&[("mmc-pack.json",br#"{"components":[{"uid":"net.minecraft","version":"1.21.1"},{"uid":"custom.patch","version":"1"}]}"#)]);
    let output = f.0.join("out.mrpack");
    assert!(convert_multimc(&input, &output).is_err());
    assert!(!output.exists());
}
#[test]
fn archive_preflight_rejects_traversal_case_alias_and_file_parent_collisions() {
    for extra in [
        vec![("minecraft/mods/../../escape.jar", &b"bad"[..])],
        vec![
            ("minecraft/mods/A.jar", &b"a"[..]),
            ("minecraft/mods/a.jar", &b"b"[..]),
        ],
        vec![
            ("minecraft/config/file", &b"a"[..]),
            ("minecraft/config/file/child", &b"b"[..]),
        ],
    ] {
        let f = Fixture::new();
        let mut entries = vec![("mmc-pack.json", MMC)];
        entries.extend(extra);
        let path = f.archive(&entries);
        assert!(checked_archive(&path).is_err());
    }
}
#[tokio::test]
async fn curseforge_overrides_only_pack_converts_without_key_and_optional_files_are_skipped() {
    let f = Fixture::new();
    let input=f.archive(&[("manifest.json",br#"{"minecraft":{"version":"1.21.1","modLoaders":[{"id":"neoforge-21.1.234","primary":true}]},"name":"CF fixture","version":"1","files":[{"projectID":42,"fileID":43,"required":false}],"overrides":"overrides"}"#),("overrides/config/settings.json",b"{}")]);
    let output = f.0.join("converted.mrpack");
    convert_curseforge(
        &input,
        &output,
        &DownloadConfig::default(),
        &ImportOptionsExt::default(),
        "http://127.0.0.1:1",
        None,
    )
    .await
    .unwrap();
    let pack = modpack_format::read_pack(&output).unwrap();
    assert_eq!(pack.manifest.dependencies["neoforge"], "21.1.234");
    assert_eq!(pack.overrides["config/settings.json"], b"{}");
}
#[tokio::test]
async fn curseforge_required_remote_files_need_owner_key_before_contacting_api() {
    let f = Fixture::new();
    let input=f.archive(&[("manifest.json",br#"{"minecraft":{"version":"1.21.1","modLoaders":[]},"files":[{"projectID":42,"fileID":43}],"overrides":"overrides"}"#)]);
    let output = f.0.join("converted.mrpack");
    let error = convert_curseforge(
        &input,
        &output,
        &DownloadConfig::default(),
        &ImportOptionsExt::default(),
        "http://127.0.0.1:1",
        None,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("CCL_CURSEFORGE_API_KEY"));
    assert!(!output.exists());
}

fn api_fixture(status: u16, body: &'static str) -> (String, thread::JoinHandle<String>) {
    api_fixture_delay(status, body, std::time::Duration::ZERO)
}
fn api_fixture_delay(
    status: u16,
    body: &'static str,
    delay: std::time::Duration,
) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line.is_empty() || line == "\r\n" {
                break;
            }
            request.push_str(&line)
        }
        thread::sleep(delay);
        let title = if status == 200 { "OK" } else { "Unauthorized" };
        let response = format!(
            "HTTP/1.1 {status} {title}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        request
    });
    (format!("http://{address}/v1"), worker)
}

fn cf_required_archive(f: &Fixture) -> PathBuf {
    f.archive(&[("manifest.json",br#"{"minecraft":{"version":"1.21.1","modLoaders":[]} ,"name":"Fixture","files":[{"projectID":42,"fileID":43}],"overrides":"overrides"}"#)])
}

#[tokio::test]
async fn curseforge_api_authorization_is_user_supplied_and_unauthorized_errors_do_not_echo_it() {
    let f = Fixture::new();
    let input = cf_required_archive(&f);
    let output = f.0.join("unauthorized.mrpack");
    let (base, worker) = api_fixture(401, "{}");
    let key = "fixture-owner-key-never-log";
    let options = ImportOptionsExt {
        curseforge_api_key: Some(key.into()),
        ..Default::default()
    };
    assert!(!format!("{options:?}").contains(key));
    let error = convert_curseforge(
        &input,
        &output,
        &DownloadConfig::default(),
        &options,
        &base,
        None,
    )
    .await
    .unwrap_err();
    let request = worker.join().unwrap();
    assert!(matches!(error, ImportFormatError::Http(401)));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("x-api-key: fixture-owner-key-never-log")
    );
    assert!(!error.to_string().contains(key));
    assert!(!output.exists());
}

#[tokio::test]
async fn curseforge_restricted_file_without_official_download_url_stops_before_file_request() {
    let f = Fixture::new();
    let input = cf_required_archive(&f);
    let output = f.0.join("restricted.mrpack");
    let (base, worker) = api_fixture(
        200,
        r#"{"data":{"downloadUrl":null,"fileName":"restricted.jar","fileLength":16,"hashes":[{"value":"0123456789012345678901234567890123456789","algo":1}]}}"#,
    );
    let error = convert_curseforge(
        &input,
        &output,
        &DownloadConfig::default(),
        &ImportOptionsExt {
            curseforge_api_key: Some("fixture-key".into()),
            ..Default::default()
        },
        &base,
        None,
    )
    .await
    .unwrap_err();
    let _request = worker.join().unwrap();
    assert!(error.to_string().contains("not directly downloadable"));
    assert!(!output.exists());
}

#[tokio::test]
async fn oversized_curseforge_metadata_body_is_rejected_from_content_length() {
    let f = Fixture::new();
    let input = cf_required_archive(&f);
    let output = f.0.join("oversize-response.mrpack");
    let (base, worker) = api_fixture_with_oversized_length();
    let error = convert_curseforge(
        &input,
        &output,
        &DownloadConfig::default(),
        &ImportOptionsExt {
            curseforge_api_key: Some("fixture-key".into()),
            ..Default::default()
        },
        &base,
        None,
    )
    .await
    .unwrap_err();
    let _request = worker.join().unwrap();
    assert!(error.to_string().contains("exceeds 2097152 bytes"));
    assert!(!output.exists());
}

fn api_fixture_with_oversized_length() -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line.is_empty() || line == "\r\n" {
                break;
            }
            request.push_str(&line)
        }
        let response = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2097153\r\nConnection: close\r\n\r\n{}";
        let _ = stream.write_all(response.as_bytes());
        request
    });
    (format!("http://{address}/v1"), worker)
}

#[tokio::test]
async fn pre_cancelled_curseforge_metadata_resolution_returns_typed_cancellation() {
    use crate::task::{CancellationToken, TaskContext};
    use std::sync::Arc;
    let f = Fixture::new();
    let input = cf_required_archive(&f);
    let output = f.0.join("cancelled.mrpack");
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let context = TaskContext::new(cancellation, Arc::new(|_| {}));
    let error = convert_curseforge(
        &input,
        &output,
        &DownloadConfig::default(),
        &ImportOptionsExt {
            curseforge_api_key: Some("fixture-key".into()),
            ..Default::default()
        },
        "http://127.0.0.1:1/v1",
        Some(context),
    )
    .await
    .unwrap_err();
    assert!(error.is_cancelled());
    assert!(!output.exists());
}

#[tokio::test]
async fn cancellation_interrupts_a_pending_curseforge_metadata_request() {
    use crate::task::{CancellationToken, TaskContext};
    use std::sync::Arc;
    let f = Fixture::new();
    let input = cf_required_archive(&f);
    let output = f.0.join("cancel-pending.mrpack");
    let (base, server) = api_fixture_delay(
        200,
        r#"{"data":{"downloadUrl":null,"fileName":"unused.jar","fileLength":0,"hashes":[]}}"#,
        std::time::Duration::from_millis(500),
    );
    let cancellation = CancellationToken::default();
    let context = TaskContext::new(cancellation.clone(), Arc::new(|_| {}));
    let input2 = input.clone();
    let output2 = output.clone();
    let options = ImportOptionsExt {
        curseforge_api_key: Some("fixture-key".into()),
        ..Default::default()
    };
    let task = tokio::spawn(async move {
        convert_curseforge(
            &input2,
            &output2,
            &DownloadConfig::default(),
            &options,
            &base,
            Some(context),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    cancellation.cancel();
    let error = task.await.unwrap().unwrap_err();
    let request = server.join().unwrap();
    assert!(error.is_cancelled());
    assert!(
        request
            .to_ascii_lowercase()
            .contains("x-api-key: fixture-key")
    );
    assert!(!output.exists());
}

#[tokio::test]
async fn converted_curseforge_and_prism_payloads_install_without_runtime_or_provider_key() {
    let f = Fixture::new();
    let cf=f.archive(&[("manifest.json",br#"{"minecraft":{"version":"1.21.1","modLoaders":[{"id":"neoforge-21.1.234","primary":true}]} ,"name":"Local CF","files":[],"overrides":"overrides"}"#),("overrides/config/settings.json",b"{}"),("overrides/options.txt",b"local" )]);
    let prism = f.archive(&[
        ("Pack/mmc-pack.json", MMC),
        ("Pack/.minecraft/saves/World/level.dat", b"world"),
        ("Pack/.minecraft/options.txt", b"music:0"),
        ("Pack/.minecraft/config/config.toml", b"value=1"),
    ]);
    let service = ImportFormatService::new_prepared(f.0.clone(), DownloadConfig::default());
    let cf_meta = service
        .import("cf", &cf, &ImportOptionsExt::default(), None)
        .await
        .unwrap();
    let prism_meta = service
        .import("prism", &prism, &ImportOptionsExt::default(), None)
        .await
        .unwrap();
    assert_eq!(
        fs::read(cf_meta.game_directory.join("config/settings.json")).unwrap(),
        b"{}"
    );
    assert_eq!(
        fs::read(cf_meta.game_directory.join("options.txt")).unwrap(),
        b"local"
    );
    assert_eq!(
        fs::read(prism_meta.game_directory.join("saves/World/level.dat")).unwrap(),
        b"world"
    );
    assert_eq!(
        fs::read(prism_meta.game_directory.join("config/config.toml")).unwrap(),
        b"value=1"
    );
}

#[test]
fn archive_preflight_bounds_declared_sizes_before_payload_reads_and_allows_dot_minecraft_paths() {
    assert!(validate_archive_name("Pack/.minecraft/mods/example.jar").is_ok());
    assert!(validate_archive_name("Pack/mods/../escape.jar").is_err());
    assert!(validate_archive_entry_count(100_000).is_ok());
    assert!(validate_archive_entry_count(100_001).is_err());
    let mut total = 0;
    assert!(add_archive_size(&mut total, 128 * 1024 * 1024, "a").is_ok());
    assert!(add_archive_size(&mut total, 128 * 1024 * 1024, "b").is_ok());
    assert!(add_archive_size(&mut total, 128 * 1024 * 1024, "c").is_ok());
    assert!(add_archive_size(&mut total, 128 * 1024 * 1024, "d").is_ok());
    assert!(add_archive_size(&mut total, 1, "e").is_err());
    let f = Fixture::new();
    let path = f.0.join("large.zip");
    let mut zip = ZipWriter::new(fs::File::create(&path).unwrap());
    zip.start_file("mmc-pack.json", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(MMC).unwrap();
    zip.start_file("large.bin", SimpleFileOptions::default())
        .unwrap();
    let chunk = vec![0u8; 64 * 1024];
    for _ in 0..2049 {
        zip.write_all(&chunk).unwrap();
    }
    zip.finish().unwrap();
    let error = match checked_archive(&path) {
        Ok(_) => panic!("oversized entry unexpectedly accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("128 MiB"));
}

#[test]
fn oversized_manifest_is_rejected_from_entry_metadata_before_json_allocation() {
    let f = Fixture::new();
    let path = f.0.join("oversized-manifest.zip");
    let mut writer = ZipWriter::new(fs::File::create(&path).unwrap());
    writer
        .start_file("manifest.json", SimpleFileOptions::default())
        .unwrap();
    let chunk = vec![b'x'; 64 * 1024];
    for _ in 0..33 {
        writer.write_all(&chunk).unwrap()
    }
    writer.finish().unwrap();
    let (mut archive, _) = checked_archive(&path).unwrap();
    let error = read_json_entry(&mut archive, "manifest.json").unwrap_err();
    assert!(error.to_string().contains("manifest exceeds 2 MiB"));
}
