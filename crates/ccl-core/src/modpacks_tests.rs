use super::*;

use crate::{
    CancellationToken, TaskContext,
    config::DownloadConfig,
    instances::InstanceService,
    modpack_format::{Environment, EnvironmentValue, PackArchive, PackFile},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "ccl-modpack-{label}-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn service(root: &Path) -> ModpackService {
    ModpackService {
        instances: InstanceService::new(root.to_path_buf(), DownloadConfig::default()).unwrap(),
        config: DownloadConfig::default(),
        runtime_subdirectory: None,
        prepared: true,
    }
}

fn pack(
    root: &Path,
    version: &str,
    overrides: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> PathBuf {
    let path = root.join(format!("{version}.mrpack"));
    let manifest = Manifest {
        format_version: 1,
        game: "minecraft".into(),
        version_id: version.into(),
        name: "Test pack".into(),
        summary: Some("prepared fixture".into()),
        files: Vec::new(),
        dependencies: [("minecraft".into(), "1.21.1".into())].into(),
    };
    modpack_format::write_pack(&path, &manifest, &overrides.into_iter().collect()).unwrap();
    path
}

fn imports() -> ImportOptions {
    ImportOptions::default()
}

#[tokio::test]
async fn import_existing_rejects_missing_id_while_import_keeps_upsert_behavior() {
    let temp = TempDir::new("strict-update");
    let service = service(temp.path());
    let archive = pack(temp.path(), "strict-update", []);

    let error = service
        .import_existing("missing", &archive, &imports(), None)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ModpackError::Instance(InstanceError::NotFound(_))
    ));
    assert!(service.instances.metadata("missing").is_err());

    let imported = service
        .import("missing", &archive, &imports(), None)
        .await
        .unwrap();
    assert_eq!(imported.id, "missing");
}

#[tokio::test]
async fn exports_an_ordinary_managed_instance_with_loader_dependencies() {
    let temp = TempDir::new("ordinary-export");
    let service = service(temp.path());
    let archive = pack(
        temp.path(),
        "seed",
        [("mods/local.jar".into(), b"local binary".to_vec())],
    );
    let mut metadata = service
        .import("ordinary", &archive, &imports(), None)
        .await
        .unwrap();
    fs::remove_file(metadata.game_directory.join(RECEIPT)).unwrap();
    metadata.loader = Some(LoaderKind::Fabric);
    metadata.loader_version = Some("fabric-loader-0.20.1-1.21.1".into());
    fs::write(
        instance_path(temp.path(), "ordinary").join("instance.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    let output = temp.path().join("ordinary.mrpack");
    assert!(matches!(
        service.export("ordinary", &output, false),
        Err(ModpackError::Conflict { .. })
    ));
    let manifest = service.export("ordinary", &output, true).unwrap();
    assert_eq!(manifest.dependencies["fabric-loader"], "0.20.1");
    let parsed = modpack_format::read_pack(&output).unwrap();
    assert_eq!(parsed.overrides["mods/local.jar"], b"local binary");
    assert!(parsed.manifest.files.is_empty());
    for (kind, profile, key, release) in [
        (LoaderKind::Forge, "1.21.1-forge-52.1.0", "forge", "52.1.0"),
        (
            LoaderKind::NeoForge,
            "neoforge-21.1.234",
            "neoforge",
            "21.1.234",
        ),
    ] {
        metadata.loader = Some(kind);
        metadata.loader_version = Some(profile.into());
        assert_eq!(
            manifest_for_instance(&metadata).unwrap().dependencies[key],
            release
        );
    }
}

#[tokio::test]
async fn export_embeds_edited_configuration_and_excludes_private_remote_paths() {
    let temp = TempDir::new("export-privacy");
    let service = service(temp.path());
    let archive = pack(
        temp.path(),
        "v1",
        [
            ("config/a.txt".into(), b"original".to_vec()),
            ("saves/world/level.dat".into(), b"private world".to_vec()),
            ("servers.dat".into(), b"private servers".to_vec()),
        ],
    );
    let instance = service
        .import("privacy", &archive, &imports(), None)
        .await
        .unwrap();
    let mut receipt = service.show("privacy").unwrap();
    for path in ["config/a.txt", "saves/world/level.dat", "servers.dat"] {
        receipt.manifest.files.push(PackFile {
            path: path.into(),
            hashes: [
                ("sha1".into(), "a".repeat(40)),
                ("sha512".into(), hash(b"original")),
            ]
            .into(),
            env: None,
            downloads: vec!["https://cdn.modrinth.com/test".into()],
            file_size: 8,
        });
    }
    fs::write(
        instance.game_directory.join(RECEIPT),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    fs::write(
        instance.game_directory.join("config/a.txt"),
        b"edited configuration",
    )
    .unwrap();
    let output = temp.path().join("export.mrpack");
    service.export("privacy", &output, false).unwrap();
    let exported = modpack_format::read_pack(&output).unwrap();
    assert!(exported.manifest.files.is_empty());
    assert_eq!(exported.overrides["config/a.txt"], b"edited configuration");
    assert!(
        exported
            .overrides
            .keys()
            .all(|path| !private_export_path(path))
    );
}

fn cancelled_context(token: CancellationToken) -> TaskContext {
    TaskContext::new(token, Arc::new(|_| {}))
}

fn instance_path(root: &Path, id: &str) -> PathBuf {
    root.join(".ccl/instances").join(id)
}

#[tokio::test]
async fn imports_overrides_receipt_and_instance_metadata_then_reimports_to_second_id() {
    let temp = TempDir::new("import");
    let service = service(temp.path());
    let archive = pack(
        temp.path(),
        "v1",
        [
            ("config/example.toml".into(), b"answer = 42\n".to_vec()),
            ("options.txt".into(), b"snooperEnabled:false\n".to_vec()),
        ],
    );

    let metadata = service
        .import("pack-one", &archive, &imports(), None)
        .await
        .unwrap();
    assert_eq!(metadata.id, "pack-one");
    assert_eq!(metadata.minecraft, "1.21.1");
    assert_eq!(metadata.loader, None);
    assert_eq!(
        fs::read(metadata.game_directory.join("config/example.toml")).unwrap(),
        b"answer = 42\n"
    );
    assert_eq!(
        fs::read(metadata.game_directory.join("options.txt")).unwrap(),
        b"snooperEnabled:false\n"
    );

    let receipt = service.show("pack-one").unwrap();
    assert_eq!(receipt.schema, 1);
    assert_eq!(receipt.manifest.version_id, "v1");
    assert_eq!(receipt.owned_files.len(), 2);
    assert!(receipt.owned_files.contains_key("config/example.toml"));
    assert!(!receipt.include_optional);

    let second = service
        .import("pack-two", &archive, &imports(), None)
        .await
        .unwrap();
    assert_eq!(second.id, "pack-two");
    assert_eq!(
        fs::read(second.game_directory.join("config/example.toml")).unwrap(),
        b"answer = 42\n"
    );
    assert_eq!(service.show("pack-two").unwrap().manifest.version_id, "v1");
}

#[tokio::test]
async fn update_refuses_modified_deleted_and_new_unmanaged_collision_without_touching_active_generation()
 {
    // Modified pack-owned data is preserved on failure.
    let modified_root = TempDir::new("update-modified");
    let modified = service(modified_root.path());
    let old = pack(
        modified_root.path(),
        "old",
        [("config/owned.txt".into(), b"old".to_vec())],
    );
    let middle = pack(
        modified_root.path(),
        "middle",
        [("config/owned.txt".into(), b"middle".to_vec())],
    );
    let new = pack(
        modified_root.path(),
        "new",
        [("config/owned.txt".into(), b"new".to_vec())],
    );
    let first = modified
        .import("modified", &old, &imports(), None)
        .await
        .unwrap();
    modified
        .import("modified", &middle, &imports(), None)
        .await
        .unwrap();
    fs::write(first.game_directory.join("config/owned.txt"), b"user edit").unwrap();
    let before_metadata =
        fs::read(instance_path(modified_root.path(), "modified").join("instance.json")).unwrap();
    let error = modified
        .import("modified", &new, &imports(), None)
        .await
        .unwrap_err();
    assert!(matches!(error, ModpackError::Conflict { .. }));
    assert_eq!(
        fs::read(first.game_directory.join("config/owned.txt")).unwrap(),
        b"user edit"
    );
    assert_eq!(
        fs::read(instance_path(modified_root.path(), "modified").join("instance.json")).unwrap(),
        before_metadata
    );
    assert_eq!(
        fs::read(instance_path(modified_root.path(), "modified").join("previous/config/owned.txt"))
            .unwrap(),
        b"old"
    );
    assert_eq!(
        modified.show("modified").unwrap().manifest.version_id,
        "middle"
    );

    // Deleted pack-owned data is also treated as a local modification.
    let deleted_root = TempDir::new("update-deleted");
    let deleted = service(deleted_root.path());
    let old = pack(
        deleted_root.path(),
        "old",
        [("config/owned.txt".into(), b"old".to_vec())],
    );
    let new = pack(
        deleted_root.path(),
        "new",
        [("config/owned.txt".into(), b"new".to_vec())],
    );
    let first = deleted
        .import("deleted", &old, &imports(), None)
        .await
        .unwrap();
    fs::remove_file(first.game_directory.join("config/owned.txt")).unwrap();
    let error = deleted
        .import("deleted", &new, &imports(), None)
        .await
        .unwrap_err();
    assert!(matches!(error, ModpackError::Conflict { .. }));
    assert!(!first.game_directory.join("config/owned.txt").exists());
    assert!(
        !instance_path(deleted_root.path(), "deleted")
            .join("previous")
            .exists()
    );
    assert_eq!(deleted.show("deleted").unwrap().manifest.version_id, "old");

    // A path newly claimed by the pack cannot overwrite pre-existing unowned data.
    let collision_root = TempDir::new("update-unmanaged");
    let collision = service(collision_root.path());
    let old = pack(
        collision_root.path(),
        "old",
        [("config/owned.txt".into(), b"old".to_vec())],
    );
    let new = pack(
        collision_root.path(),
        "new",
        [("config/local.txt".into(), b"from pack".to_vec())],
    );
    let first = collision
        .import("collision", &old, &imports(), None)
        .await
        .unwrap();
    fs::write(first.game_directory.join("config/local.txt"), b"unmanaged").unwrap();
    let error = collision
        .import("collision", &new, &imports(), None)
        .await
        .unwrap_err();
    assert!(matches!(error, ModpackError::Conflict { .. }));
    assert_eq!(
        fs::read(first.game_directory.join("config/local.txt")).unwrap(),
        b"unmanaged"
    );
    assert_eq!(
        collision.show("collision").unwrap().manifest.version_id,
        "old"
    );
}

#[tokio::test]
async fn explicit_replace_replaces_user_edit_and_preserves_it_in_previous_generation() {
    let temp = TempDir::new("replace");
    let service = service(temp.path());
    let archive = pack(
        temp.path(),
        "v1",
        [("config/owned.txt".into(), b"pack value".to_vec())],
    );
    let metadata = service
        .import("replace", &archive, &imports(), None)
        .await
        .unwrap();
    fs::write(
        metadata.game_directory.join("config/owned.txt"),
        b"user value",
    )
    .unwrap();

    let options = ImportOptions {
        replace_modified: true,
        ..ImportOptions::default()
    };
    service
        .import("replace", &archive, &options, None)
        .await
        .unwrap();
    assert_eq!(
        fs::read(metadata.game_directory.join("config/owned.txt")).unwrap(),
        b"pack value"
    );
    assert_eq!(
        fs::read(instance_path(temp.path(), "replace").join("previous/config/owned.txt")).unwrap(),
        b"user value"
    );
    assert_eq!(service.show("replace").unwrap().manifest.version_id, "v1");
}

#[tokio::test]
async fn update_removes_obsolete_owned_files_and_preserves_unowned_data() {
    let temp = TempDir::new("remove-obsolete");
    let service = service(temp.path());
    let old = pack(
        temp.path(),
        "old",
        [("config/old.txt".into(), b"old".to_vec())],
    );
    let new = pack(
        temp.path(),
        "new",
        [("config/new.txt".into(), b"new".to_vec())],
    );
    let metadata = service
        .import("cleanup", &old, &imports(), None)
        .await
        .unwrap();
    fs::write(metadata.game_directory.join("config/local.txt"), b"keep me").unwrap();

    service
        .import("cleanup", &new, &imports(), None)
        .await
        .unwrap();
    assert!(!metadata.game_directory.join("config/old.txt").exists());
    assert_eq!(
        fs::read(metadata.game_directory.join("config/new.txt")).unwrap(),
        b"new"
    );
    assert_eq!(
        fs::read(metadata.game_directory.join("config/local.txt")).unwrap(),
        b"keep me"
    );
}

#[tokio::test]
async fn instance_rollback_undoes_and_redoes_a_pack_update() {
    let temp = TempDir::new("rollback");
    let service = service(temp.path());
    let old = pack(
        temp.path(),
        "old",
        [("config/version.txt".into(), b"old".to_vec())],
    );
    let new = pack(
        temp.path(),
        "new",
        [("config/version.txt".into(), b"new".to_vec())],
    );
    let metadata = service
        .import("rollback", &old, &imports(), None)
        .await
        .unwrap();
    service
        .import("rollback", &new, &imports(), None)
        .await
        .unwrap();
    assert_eq!(
        fs::read(metadata.game_directory.join("config/version.txt")).unwrap(),
        b"new"
    );

    service.instances.rollback("rollback").await.unwrap();
    assert_eq!(service.show("rollback").unwrap().manifest.version_id, "old");
    assert_eq!(
        fs::read(metadata.game_directory.join("config/version.txt")).unwrap(),
        b"old"
    );

    service.instances.rollback("rollback").await.unwrap();
    assert_eq!(service.show("rollback").unwrap().manifest.version_id, "new");
    assert_eq!(
        fs::read(metadata.game_directory.join("config/version.txt")).unwrap(),
        b"new"
    );
}

#[tokio::test]
async fn cancellation_before_import_and_after_stage_keeps_active_generation_unchanged() {
    let before_root = TempDir::new("cancel-before");
    let before_service = service(before_root.path());
    let before_pack = pack(before_root.path(), "before", []);
    let token = CancellationToken::default();
    token.cancel();
    let error = before_service
        .import(
            "cancel-before",
            &before_pack,
            &imports(),
            Some(cancelled_context(token)),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ModpackError::Cancelled));
    assert!(!instance_path(before_root.path(), "cancel-before").exists());

    let temp = TempDir::new("cancel-stage");
    let service = service(temp.path());
    let archive = pack(
        temp.path(),
        "stable",
        [("config/owned.txt".into(), b"stable".to_vec())],
    );
    let metadata = service
        .import("cancel-stage", &archive, &imports(), None)
        .await
        .unwrap();
    // Keep an unowned file in the game tree to verify the active generation is
    // retained when the staged import is cancelled.
    fs::write(
        metadata.game_directory.join("options.txt"),
        b"preserve this user data",
    )
    .unwrap();
    let token = CancellationToken::default();
    let callback_token = token.clone();
    let stage_at_cancellation = instance_path(temp.path(), "cancel-stage").join(".stage");
    let context = TaskContext::new(
        token.clone(),
        Arc::new(move |_| {
            assert!(stage_at_cancellation.is_dir());
            callback_token.cancel();
        }),
    );
    let archive = temp.path().join("cancel-during-download.mrpack");
    let payload = b"new remote file";
    let manifest = Manifest {
        format_version: 1,
        game: "minecraft".into(),
        version_id: "cancel-stage-new".into(),
        name: "Cancellation fixture".into(),
        summary: None,
        files: vec![PackFile {
            path: "mods/new.jar".into(),
            hashes: [
                ("sha1".into(), format!("{:x}", sha1::Sha1::digest(payload))),
                ("sha512".into(), hash(payload)),
            ]
            .into(),
            env: None,
            downloads: vec!["https://fixture.invalid/new.jar".into()],
            file_size: payload.len() as u64,
        }],
        dependencies: [("minecraft".into(), "1.21.1".into())].into(),
    };
    modpack_format::write_pack(&archive, &manifest, &BTreeMap::new()).unwrap();
    let options = ImportOptions {
        allowed_hosts: vec!["fixture.invalid".into()],
        ..imports()
    };
    let error = service
        .import("cancel-stage", &archive, &options, Some(context))
        .await
        .unwrap_err();
    assert!(error.is_cancelled(), "expected cancellation, got {error:?}");
    assert!(token.is_cancelled());
    assert_eq!(
        service.show("cancel-stage").unwrap().manifest.version_id,
        "stable"
    );
    assert_eq!(
        fs::read(metadata.game_directory.join("config/owned.txt")).unwrap(),
        b"stable"
    );
    assert_eq!(
        fs::metadata(metadata.game_directory.join("options.txt"))
            .unwrap()
            .len(),
        b"preserve this user data".len() as u64
    );
}

#[tokio::test]
async fn export_roundtrips_and_requires_explicit_local_binary_redistribution() {
    let temp = TempDir::new("export");
    let service = service(temp.path());
    let archive = pack(
        temp.path(),
        "source",
        [("config/settings.txt".into(), b"value=7".to_vec())],
    );
    let metadata = service
        .import("export", &archive, &imports(), None)
        .await
        .unwrap();
    fs::create_dir_all(metadata.game_directory.join("mods")).unwrap();
    fs::write(
        metadata.game_directory.join("mods/local.jar"),
        b"local mod bytes",
    )
    .unwrap();

    let denied = temp.path().join("denied.mrpack");
    let error = service.export("export", &denied, false).unwrap_err();
    assert!(matches!(error, ModpackError::Conflict { .. }));
    assert!(error.to_string().contains("redistribution permission"));
    assert!(!denied.exists());

    let output = temp.path().join("exported.mrpack");
    let exported = service.export("export", &output, true).unwrap();
    let PackArchive {
        manifest,
        overrides,
    } = modpack_format::read_pack(&output).unwrap();
    assert_eq!(manifest.version_id, exported.version_id);
    assert_eq!(manifest.dependencies["minecraft"], "1.21.1");
    assert_eq!(overrides["config/settings.txt"], b"value=7");
    assert_eq!(overrides["mods/local.jar"], b"local mod bytes");
}

#[tokio::test]
async fn export_reports_a_missing_receipt_owned_file_with_its_path() {
    let temp = TempDir::new("export-missing");
    let service = service(temp.path());
    let archive = pack(temp.path(), "source", []);
    let metadata = service
        .import("missing", &archive, &imports(), None)
        .await
        .unwrap();
    let receipt_path = metadata.game_directory.join(RECEIPT);
    let mut receipt: PackReceipt =
        serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    let missing = "mods/missing.jar";
    receipt.manifest.files.push(PackFile {
        path: missing.into(),
        hashes: [
            ("sha1".into(), "a".repeat(40)),
            ("sha512".into(), "b".repeat(128)),
        ]
        .into(),
        env: Some(Environment {
            client: EnvironmentValue::Required,
            server: EnvironmentValue::Unsupported,
        }),
        downloads: vec!["https://cdn.modrinth.com/data/missing.jar".into()],
        file_size: 1,
    });
    receipt.owned_files.insert(missing.into(), "b".repeat(128));
    fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();

    let error = service
        .export("missing", &temp.path().join("missing-export.mrpack"), false)
        .unwrap_err();
    assert!(matches!(error, ModpackError::Io { .. }));
    match error {
        ModpackError::Io { path, source } => {
            assert!(path.ends_with(Path::new(missing)), "{}", path.display());
            assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
        }
        other => panic!("expected missing-file diagnostic, got {other}"),
    }
}

#[tokio::test]
async fn another_launch_lock_prevents_pack_import_from_mutating_the_instance() {
    let temp = TempDir::new("locked");
    let service = service(temp.path());
    let old = pack(
        temp.path(),
        "old",
        [("config/owned.txt".into(), b"old".to_vec())],
    );
    let new = pack(
        temp.path(),
        "new",
        [("config/owned.txt".into(), b"new".to_vec())],
    );
    let metadata = service
        .import("locked", &old, &imports(), None)
        .await
        .unwrap();
    let guard = service.instances.acquire_launch("locked").unwrap();

    let error = service
        .import("locked", &new, &imports(), None)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ModpackError::Instance(InstanceError::Conflict(_))
    ));
    assert_eq!(
        service.show("locked").unwrap_err().to_string(),
        "instance is busy or being used by a game process: locked"
    );
    drop(guard);
    assert_eq!(service.show("locked").unwrap().manifest.version_id, "old");
    assert_eq!(
        fs::read(metadata.game_directory.join("config/owned.txt")).unwrap(),
        b"old"
    );
}
