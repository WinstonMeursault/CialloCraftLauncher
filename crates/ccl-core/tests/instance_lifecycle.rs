use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use ccl_core::{
    CancellationToken, TaskContext,
    config::DownloadConfig,
    instances::{InstanceError, InstanceMetadata, InstanceService},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ccl-instance-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn service(root: &Temp) -> InstanceService {
    InstanceService::new(root.0.clone(), DownloadConfig::default()).unwrap()
}
fn meta(id: &str, minecraft: &str, loader_version: Option<&str>) -> InstanceMetadata {
    InstanceMetadata {
        id: id.into(),
        minecraft: minecraft.into(),
        loader: loader_version.map(|_| ccl_core::loaders::LoaderKind::Fabric),
        loader_version: loader_version.map(str::to_owned),
        game_directory: PathBuf::new(),
    }
}

#[test]
fn rejects_traversal_and_reports_missing_instances() {
    let root = Temp::new();
    let service = service(&root);
    assert!(matches!(
        service.metadata("../outside"),
        Err(InstanceError::InvalidId(_))
    ));
    assert!(matches!(
        service.metadata("missing"),
        Err(InstanceError::NotFound(_))
    ));
    assert!(matches!(
        service.acquire_launch("missing"),
        Err(InstanceError::NotFound(_))
    ));
    assert!(service.list().unwrap().is_empty());
}

#[test]
fn constructing_or_listing_does_not_create_a_missing_game_root() {
    let base = Temp::new();
    let missing = base.0.join("not-created");
    let service = InstanceService::new(missing.clone(), DownloadConfig::default()).unwrap();
    assert!(service.list().unwrap().is_empty());
    assert!(!missing.exists());
}

#[test]
fn rejects_metadata_that_points_outside_instance_storage() {
    let root = Temp::new();
    let service = service(&root);
    let instance = root.0.join(".ccl/instances/test");
    fs::create_dir_all(instance.join("game")).unwrap();
    let mut metadata = meta("test", "1.21", None);
    metadata.game_directory = root.0.join("outside");
    fs::write(
        instance.join("instance.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        service.metadata("test"),
        Err(InstanceError::InvalidId(_))
    ));

    let mut metadata = meta("test", "1.21", Some("fabric-loader-1"));
    metadata.game_directory = instance.join("game");
    metadata.loader_version = Some("../../outside".into());
    fs::write(
        instance.join("instance.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        service.metadata("test"),
        Err(InstanceError::InvalidId(_))
    ));
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_instance_paths() {
    use std::os::unix::fs::symlink;
    let root = Temp::new();
    let service = service(&root);
    let external = root.0.join("external");
    fs::create_dir_all(&external).unwrap();
    fs::create_dir_all(root.0.join(".ccl/instances")).unwrap();
    symlink(&external, root.0.join(".ccl/instances/test")).unwrap();
    assert!(matches!(
        service.metadata("test"),
        Err(InstanceError::Conflict(_))
    ));
}

#[tokio::test]
async fn rollback_exchanges_whole_generation_and_preserves_user_files() {
    let root = Temp::new();
    let service = service(&root);
    let instance = root.0.join(".ccl/instances/test");
    fs::create_dir_all(instance.join("game/saves/world")).unwrap();
    fs::create_dir_all(instance.join("previous/saves/world")).unwrap();
    fs::write(instance.join("game/saves/world/marker"), "new").unwrap();
    fs::write(instance.join("previous/saves/world/marker"), "old").unwrap();
    fs::write(
        instance.join("instance.json"),
        serde_json::to_vec(&meta("test", "1.21", Some("fabric-loader-1"))).unwrap(),
    )
    .unwrap();
    fs::write(
        instance.join("previous.json"),
        serde_json::to_vec(&meta("test", "1.20.6", None)).unwrap(),
    )
    .unwrap();
    let restored = service.rollback("test").await.unwrap();
    assert_eq!(restored.minecraft, "1.20.6");
    assert_eq!(
        fs::read_to_string(instance.join("game/saves/world/marker")).unwrap(),
        "new"
    );
    assert_eq!(
        fs::read_to_string(instance.join("previous/saves/world/marker")).unwrap(),
        "new"
    );
    assert_eq!(service.metadata("test").unwrap().minecraft, "1.20.6");
}

#[tokio::test]
async fn uninstall_loader_removes_only_loader_version_and_keeps_user_data() {
    let root = Temp::new();
    let service = service(&root);
    let instance = root.0.join(".ccl/instances/test");
    fs::create_dir_all(instance.join("game/versions/fabric-loader-1")).unwrap();
    fs::create_dir_all(instance.join("game/versions/1.21")).unwrap();
    fs::create_dir_all(instance.join("game/mods")).unwrap();
    fs::create_dir_all(instance.join("game/config")).unwrap();
    fs::create_dir_all(instance.join("game/saves/world")).unwrap();
    fs::write(instance.join("game/mods/user.jar"), "user").unwrap();
    fs::write(instance.join("game/config/user.cfg"), "config").unwrap();
    fs::write(instance.join("game/saves/world/level.dat"), "save").unwrap();
    fs::write(instance.join("game/options.txt"), "options").unwrap();
    fs::write(
        instance.join("instance.json"),
        serde_json::to_vec(&meta("test", "1.21", Some("fabric-loader-1"))).unwrap(),
    )
    .unwrap();
    let uninstalled = service.uninstall_loader("test", None).await.unwrap();
    assert!(uninstalled.loader.is_none());
    assert!(!instance.join("game/versions/fabric-loader-1").exists());
    assert!(instance.join("game/versions/1.21").exists());
    assert_eq!(
        fs::read_to_string(instance.join("game/mods/user.jar")).unwrap(),
        "user"
    );
    assert_eq!(
        fs::read_to_string(instance.join("game/saves/world/level.dat")).unwrap(),
        "save"
    );
    assert_eq!(
        fs::read_to_string(instance.join("game/config/user.cfg")).unwrap(),
        "config"
    );
    assert_eq!(
        fs::read_to_string(instance.join("game/options.txt")).unwrap(),
        "options"
    );
    let rolled_back = service.rollback("test").await.unwrap();
    assert_eq!(
        rolled_back.loader,
        Some(ccl_core::loaders::LoaderKind::Fabric)
    );
    assert_eq!(
        rolled_back.loader_version.as_deref(),
        Some("fabric-loader-1")
    );
    assert!(instance.join("game/versions/fabric-loader-1").exists());
    assert_eq!(
        fs::read_to_string(instance.join("game/mods/user.jar")).unwrap(),
        "user"
    );
    assert_eq!(
        fs::read_to_string(instance.join("game/config/user.cfg")).unwrap(),
        "config"
    );
    assert_eq!(
        fs::read_to_string(instance.join("game/saves/world/level.dat")).unwrap(),
        "save"
    );
    assert_eq!(
        fs::read_to_string(instance.join("game/options.txt")).unwrap(),
        "options"
    );
}

#[tokio::test]
async fn cancelled_uninstall_leaves_instance_files_untouched() {
    let root = Temp::new();
    let service = service(&root);
    let instance = root.0.join(".ccl/instances/test");
    fs::create_dir_all(instance.join("game/versions/fabric-loader-1")).unwrap();
    fs::write(
        instance.join("instance.json"),
        serde_json::to_vec(&meta("test", "1.21", Some("fabric-loader-1"))).unwrap(),
    )
    .unwrap();
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let context = TaskContext::new(cancellation, std::sync::Arc::new(|_| {}));
    assert!(matches!(
        service.uninstall_loader("test", Some(context)).await,
        Err(InstanceError::Cancelled)
    ));
    assert!(instance.join("game/versions/fabric-loader-1").exists());
}

#[test]
fn launch_guard_blocks_mutating_operations() {
    let root = Temp::new();
    let service = service(&root);
    let instance = root.0.join(".ccl/instances/test");
    fs::create_dir_all(instance.join("game")).unwrap();
    fs::write(
        instance.join("instance.json"),
        serde_json::to_vec(&meta("test", "1.21", None)).unwrap(),
    )
    .unwrap();
    let guard = service.acquire_launch("test").unwrap();
    assert!(matches!(
        service.acquire_launch("test"),
        Err(InstanceError::Conflict(_))
    ));
    drop(guard);
    assert!(service.acquire_launch("test").is_ok());
}
