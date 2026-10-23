use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use ccl_core::{
    config::DownloadConfig,
    content::{ContentKind, ContentService},
    instances::{InstanceMetadata, InstanceService},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "ccl-content-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn setup(root: &Temp) -> ContentService {
    let instance = root.0.join(".ccl/instances/demo");
    let game = instance.join("game");
    fs::create_dir_all(&game).unwrap();
    let game = fs::canonicalize(game).unwrap();
    let metadata = InstanceMetadata {
        id: "demo".into(),
        minecraft: "1.21".into(),
        loader: None,
        loader_version: None,
        game_directory: game,
    };
    fs::write(
        instance.join("instance.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    ContentService::new(root.0.clone(), DownloadConfig::default()).unwrap()
}

#[test]
fn manages_resource_packs_shader_packs_and_nested_saves() {
    use std::io::Write;
    let root = Temp::new();
    let svc = setup(&root);
    for (kind, name) in [
        (ContentKind::ResourcePack, "resources.zip"),
        (ContentKind::ShaderPack, "shaders.zip"),
    ] {
        let source = root.0.join(name);
        let mut zip = zip::ZipWriter::new(fs::File::create(&source).unwrap());
        zip.start_file("pack.mcmeta", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"{}").unwrap();
        zip.finish().unwrap();
        let entry = svc.add("demo", kind, &source, None).unwrap();
        assert_eq!(entry.path, name);
        assert_eq!(svc.list("demo", kind).unwrap(), vec![entry]);
        svc.remove("demo", kind, name).unwrap();
        assert!(svc.list("demo", kind).unwrap().is_empty());
        assert!(source.exists());
    }
    let world = root.0.join("world");
    fs::create_dir_all(world.join("region")).unwrap();
    fs::write(world.join("level.dat"), b"fixture level").unwrap();
    fs::write(world.join("region/r.0.0.mca"), b"fixture region").unwrap();
    svc.add("demo", ContentKind::Save, &world, Some("MyWorld"))
        .unwrap();
    assert_eq!(
        svc.list("demo", ContentKind::Save).unwrap()[0].path,
        "MyWorld"
    );
    let game = root.0.join(".ccl/instances/demo/game");
    assert_eq!(
        fs::read(game.join("saves/MyWorld/region/r.0.0.mca")).unwrap(),
        b"fixture region"
    );
    svc.remove("demo", ContentKind::Save, "MyWorld").unwrap();
    assert!(svc.list("demo", ContentKind::Save).unwrap().is_empty());
    assert!(world.join("level.dat").exists());
}

#[test]
fn manages_local_mods_and_enable_state_without_deleting_source() {
    let root = Temp::new();
    let svc = setup(&root);
    let source = root.0.join("local.jar");
    fs::write(&source, b"jar bytes").unwrap();
    let added = svc.add("demo", ContentKind::Mod, &source, None).unwrap();
    assert!(added.enabled);
    assert!(source.exists());
    let off = svc.set_enabled("demo", "local.jar", false).unwrap();
    assert!(!off.enabled);
    assert_eq!(off.path, "local.jar.disabled");
    assert_eq!(svc.list("demo", ContentKind::Mod).unwrap(), vec![off]);
    svc.remove("demo", ContentKind::Mod, "local.jar.disabled")
        .unwrap();
    assert!(svc.list("demo", ContentKind::Mod).unwrap().is_empty());
    assert!(source.exists());
}

#[test]
fn disable_then_enable_by_base_jar_name_and_accept_exact_disabled_name() {
    let root = Temp::new();
    let svc = setup(&root);
    let source = root.0.join("fabric-api-X.jar");
    fs::write(&source, b"fabric api").unwrap();
    svc.add("demo", ContentKind::Mod, &source, None).unwrap();

    let disabled = svc.set_enabled("demo", "fabric-api-X.jar", false).unwrap();
    assert_eq!(disabled.path, "fabric-api-X.jar.disabled");
    assert!(!disabled.enabled);

    let enabled = svc.set_enabled("demo", "fabric-api-X.jar", true).unwrap();
    assert_eq!(enabled.path, "fabric-api-X.jar");
    assert!(enabled.enabled);

    svc.set_enabled("demo", "fabric-api-X.jar", false).unwrap();
    let enabled = svc
        .set_enabled("demo", "fabric-api-X.jar.disabled", true)
        .unwrap();
    assert_eq!(enabled.path, "fabric-api-X.jar");
    assert!(enabled.enabled);
}

#[test]
fn enable_rejects_both_active_and_disabled_files_without_overwriting_either() {
    let root = Temp::new();
    let svc = setup(&root);
    let mods = root.0.join(".ccl/instances/demo/game/mods");
    fs::create_dir_all(&mods).unwrap();
    fs::write(mods.join("fabric-api-X.jar"), b"active bytes").unwrap();
    fs::write(mods.join("fabric-api-X.jar.disabled"), b"disabled bytes").unwrap();

    assert!(svc.set_enabled("demo", "fabric-api-X.jar", true).is_err());
    assert_eq!(
        fs::read(mods.join("fabric-api-X.jar")).unwrap(),
        b"active bytes"
    );
    assert_eq!(
        fs::read(mods.join("fabric-api-X.jar.disabled")).unwrap(),
        b"disabled bytes"
    );
}

#[test]
fn backup_restore_round_trip_covers_userdata_and_excludes_accounts() {
    let root = Temp::new();
    let svc = setup(&root);
    let game = root.0.join(".ccl/instances/demo/game");
    fs::create_dir_all(game.join("saves/world")).unwrap();
    fs::write(game.join("saves/world/level.dat"), b"save").unwrap();
    fs::create_dir_all(game.join("mods")).unwrap();
    fs::write(game.join("mods/a.jar"), b"mod").unwrap();
    fs::create_dir_all(game.join("config/empty/nested")).unwrap();
    fs::write(game.join("options.txt"), b"options").unwrap();
    fs::create_dir_all(game.join(".ccl")).unwrap();
    fs::write(game.join(".ccl/token"), b"secret").unwrap();
    let archive = root.0.join("backup.zip");
    svc.backup("demo", &archive).unwrap();
    fs::write(game.join("options.txt"), b"changed").unwrap();
    fs::remove_dir_all(game.join("saves")).unwrap();
    fs::remove_dir_all(game.join("config")).unwrap();
    fs::create_dir_all(game.join("shaderpacks/new")).unwrap();
    svc.restore("demo", &archive).unwrap();
    assert_eq!(fs::read(game.join("options.txt")).unwrap(), b"options");
    assert_eq!(
        fs::read(game.join("saves/world/level.dat")).unwrap(),
        b"save"
    );
    assert!(game.join("config/empty/nested").is_dir());
    assert!(!game.join("shaderpacks").exists());
    assert_eq!(fs::read(game.join(".ccl/token")).unwrap(), b"secret");
}

#[test]
fn rejects_overwriting_a_backup_destination() {
    let root = Temp::new();
    let svc = setup(&root);
    let archive = root.0.join("backup.zip");
    fs::write(&archive, b"keep").unwrap();
    assert!(svc.backup("demo", &archive).is_err());
    assert_eq!(fs::read(archive).unwrap(), b"keep");
}

#[test]
fn rejects_backup_destination_inside_game_without_creating_it() {
    let root = Temp::new();
    let svc = setup(&root);
    let destination = root
        .0
        .join(".ccl/instances/demo/game/new/nested/backup.zip");
    assert!(svc.backup("demo", &destination).is_err());
    assert!(!destination.parent().unwrap().exists());
}

#[test]
fn failed_edit_and_launch_lock_leave_active_content_unchanged() {
    let root = Temp::new();
    let svc = setup(&root);
    let source = root.0.join("local.jar");
    fs::write(&source, b"original").unwrap();
    svc.add("demo", ContentKind::Mod, &source, None).unwrap();

    fs::write(&source, b"replacement").unwrap();
    assert!(svc.add("demo", ContentKind::Mod, &source, None).is_err());
    let game = fs::canonicalize(root.0.join(".ccl/instances/demo/game")).unwrap();
    assert_eq!(fs::read(game.join("mods/local.jar")).unwrap(), b"original");

    let instances = InstanceService::new(root.0.clone(), DownloadConfig::default()).unwrap();
    let launch = instances.acquire_launch("demo").unwrap();
    assert!(
        svc.add("demo", ContentKind::Mod, &source, Some("other.jar"))
            .is_err()
    );
    drop(launch);
    assert!(!game.join("mods/other.jar").exists());
}

#[tokio::test]
async fn content_revision_rollback_can_undo_and_redo_a_user_change() {
    let root = Temp::new();
    let svc = setup(&root);
    let source = root.0.join("added.jar");
    fs::write(&source, b"mod").unwrap();
    svc.add("demo", ContentKind::Mod, &source, None).unwrap();
    let instances = InstanceService::new(root.0.clone(), DownloadConfig::default()).unwrap();

    instances.rollback("demo").await.unwrap();
    let game = fs::canonicalize(root.0.join(".ccl/instances/demo/game")).unwrap();
    assert!(!game.join("mods/added.jar").exists());

    instances.rollback("demo").await.unwrap();
    let game = fs::canonicalize(root.0.join(".ccl/instances/demo/game")).unwrap();
    assert_eq!(fs::read(game.join("mods/added.jar")).unwrap(), b"mod");
}
