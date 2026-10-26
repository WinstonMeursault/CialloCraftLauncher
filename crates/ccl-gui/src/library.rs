use crate::catalog::{CatalogRequest, LibraryCatalog};
use crate::{GuiError, GuiResult, Preferences, PreferencesStore};
use ccl_core::{
    compatibility::inspect_compatibility, config::LauncherConfig, directory::DirectoryInputs,
    instances::InstanceService, launch::list_installed_versions,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct LibraryEntry {
    pub key: String,
    pub id: String,
    pub display_name: String,
    pub has_revision_storage: bool,
    pub minecraft: String,
    pub loader: Option<ccl_core::loaders::LoaderKind>,
    pub game_directory: PathBuf,
    pub launch_version: String,
}
#[derive(Serialize)]
pub struct LaunchDefaults {
    pub java: Option<PathBuf>,
    pub max_memory_mb: u32,
    pub auto_install_java: bool,
    pub network_offline: bool,
}
#[derive(Serialize)]
pub struct Bootstrap {
    pub root: PathBuf,
    pub root_exists: bool,
    pub setup_required: bool,
    pub launch_defaults: LaunchDefaults,
    pub config_path: PathBuf,
    pub preferences: Preferences,
    pub entries: Vec<LibraryEntry>,
    pub hidden_entries: usize,
    pub warnings: Vec<String>,
}
pub fn bootstrap(
    store: &PreferencesStore,
    config_path: &Path,
    explicit: Option<PathBuf>,
) -> GuiResult<Bootstrap> {
    let preferences = store.load()?;
    let selected = explicit.or(store.root()?);
    let confirmed = selected.is_some();
    let mut inputs = DirectoryInputs::from_environment(selected)
        .map_err(|e| GuiError::new("directory_error", e.to_string()))?;
    // Desktop cwd can be a bundle or an arbitrary installation directory.
    inputs.working_directory = inputs.home_directory.clone();
    let resolution = inputs
        .resolve()
        .map_err(|e| GuiError::new("directory_error", e.to_string()))?;
    let root = if resolution.exists {
        std::fs::canonicalize(&resolution.path).map_err(|_| {
            GuiError::new(
                "directory_error",
                "Cannot resolve the selected game directory",
            )
        })?
    } else {
        resolution.path
    };
    let mut result = Bootstrap {
        root,
        root_exists: resolution.exists,
        setup_required: !confirmed || !resolution.exists,
        launch_defaults: LaunchDefaults {
            java: None,
            max_memory_mb: 2048,
            auto_install_java: false,
            network_offline: false,
        },
        config_path: config_path.into(),
        preferences,
        entries: vec![],
        hidden_entries: 0,
        warnings: vec![],
    };
    let compatibility = inspect_compatibility(config_path, &result.root);
    if !compatibility.compatible {
        result.warnings.push("Incompatible or unreadable metadata: library discovery is blocked without rewriting files".into());
        return Ok(result);
    }
    let config = LauncherConfig::read_or_default(config_path)
        .map_err(|e| GuiError::new("configuration_error", e.to_string()))?;
    result.launch_defaults = LaunchDefaults {
        java: config.launch.java.clone(),
        max_memory_mb: config.launch.max_memory_mb,
        auto_install_java: config.launch.auto_install_java,
        network_offline: config.downloads.offline,
    };
    if !result.root_exists {
        return Ok(result);
    }
    let catalog = LibraryCatalog::new(result.root.clone());
    match discover_entries_state(&result.root, &config.downloads) {
        Ok((entries, busy)) => {
            result.entries = entries;
            if busy {
                result.warnings.push(
                    "Instance revision metadata is busy; retry after other operations finish"
                        .into(),
                );
            }
        }
        Err(error) => result.warnings.push(error.message),
    }
    match catalog.records() {
        Ok(records) => {
            result.hidden_entries = records.values().filter(|record| record.hidden).count()
        }
        Err(error) => result.warnings.push(error.message),
    }
    Ok(result)
}

/// Discover root versions and revision-backed instances as one library shape.
/// No game directory is created, moved, or copied by discovery.
pub fn discover_entries(
    root: &Path,
    config: &ccl_core::config::DownloadConfig,
) -> GuiResult<Vec<LibraryEntry>> {
    discover_entries_state(root, config).map(|(entries, _)| entries)
}

fn discover_entries_state(
    root: &Path,
    config: &ccl_core::config::DownloadConfig,
) -> GuiResult<(Vec<LibraryEntry>, bool)> {
    let root = std::fs::canonicalize(root)
        .map_err(|error| GuiError::new("directory_error", error.to_string()))?;
    let catalog = LibraryCatalog::new(root.clone());
    let records = catalog.records()?;
    let mut entries = Vec::new();
    let versions = list_installed_versions(&root)
        .map_err(|error| GuiError::new("library_unavailable", error.to_string()))?;
    for version in versions {
        let id = version.id;
        let launch_version = id.clone();
        let directory = root.clone();
        let key = descriptor_key(&directory, &id);
        if records.get(&key).is_some_and(|record| record.hidden) {
            continue;
        }
        let display_name = records
            .get(&key)
            .map(|record| record.display_name.clone())
            .unwrap_or_else(|| id.clone());
        entries.push(LibraryEntry {
            key,
            id: id.clone(),
            display_name,
            has_revision_storage: false,
            minecraft: id,
            loader: None,
            game_directory: directory,
            launch_version,
        });
    }
    let (instances, busy) = match InstanceService::new(root.clone(), config.clone())
        .and_then(|service| service.list())
    {
        Ok(instances) => (instances, false),
        Err(ccl_core::instances::InstanceError::Conflict(_)) => (Vec::new(), true),
        Err(error) => return Err(GuiError::new("library_unavailable", error.to_string())),
    };
    for instance in instances {
        let launch_version = instance
            .loader_version
            .clone()
            .unwrap_or_else(|| instance.minecraft.clone());
        let directory = std::fs::canonicalize(&instance.game_directory)
            .map_err(|error| GuiError::new("library_unavailable", error.to_string()))?;
        let key = descriptor_key(&directory, &instance.id);
        if records.get(&key).is_some_and(|record| record.hidden) {
            continue;
        }
        let display_name = records
            .get(&key)
            .map(|record| record.display_name.clone())
            .unwrap_or_else(|| instance.id.clone());
        entries.push(LibraryEntry {
            key,
            id: instance.id,
            display_name,
            has_revision_storage: true,
            minecraft: instance.minecraft,
            loader: instance.loader,
            game_directory: directory,
            launch_version,
        });
    }
    entries.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
            .then_with(|| left.key.cmp(&right.key))
    });
    Ok((entries, busy))
}

fn descriptor_key(directory: &Path, id: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(directory.to_string_lossy().as_bytes());
    hash.update([0]);
    hash.update(id.as_bytes());
    format!("instance:{:x}", hash.finalize())
}

pub fn update_library(
    store: &PreferencesStore,
    config_path: &Path,
    explicit: Option<PathBuf>,
    request: CatalogRequest,
) -> GuiResult<Bootstrap> {
    let current = bootstrap(store, config_path, explicit.clone())?;
    if current.setup_required || !current.root_exists {
        return Err(GuiError::new(
            "library_unavailable",
            "Select an existing game directory first.",
        ));
    }
    let catalog = LibraryCatalog::new(current.root.clone());
    match request {
        CatalogRequest::Rename { key, name } => {
            discover_entries(
                &current.root,
                &LauncherConfig::read_or_default(config_path)
                    .map_err(|error| GuiError::new("configuration_error", error.to_string()))?
                    .downloads,
            )?
            .into_iter()
            .any(|entry| entry.key == key)
            .then_some(())
            .ok_or_else(|| {
                GuiError::new(
                    "library_entry_missing",
                    "This library entry is no longer available.",
                )
            })?;
            catalog.set_display_name(&key, &name)?;
        }
        CatalogRequest::Hide { key } => {
            let config = LauncherConfig::read_or_default(config_path)
                .map_err(|error| GuiError::new("configuration_error", error.to_string()))?;
            let entries = discover_entries(&current.root, &config.downloads)?;
            let entry = entries
                .into_iter()
                .find(|entry| entry.key == key)
                .ok_or_else(|| {
                    GuiError::new(
                        "library_entry_missing",
                        "This library entry is no longer available.",
                    )
                })?;
            catalog.hide(&key, &entry.display_name)?;
        }
        CatalogRequest::RestoreAll => catalog.restore_all()?,
    }
    bootstrap(store, config_path, explicit)
}

/// Explicit user confirmation is required before creating the proposed game root.
pub fn initialize_root(
    store: &PreferencesStore,
    config_path: &Path,
    explicit: Option<PathBuf>,
) -> GuiResult<Bootstrap> {
    let data = bootstrap(store, config_path, explicit.clone())?;
    if !data.warnings.is_empty() {
        return Err(GuiError::new(
            "library_unavailable",
            data.warnings.join("\n"),
        ));
    }
    std::fs::create_dir_all(&data.root)
        .map_err(|_| GuiError::new("directory_error", "Cannot create game directory"))?;
    store.select_root(data.root)?;
    bootstrap(store, config_path, explicit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PreferencesStore;
    use serde_json::json;
    use std::fs;

    #[test]
    fn rename_and_hide_are_catalog_only_for_an_existing_root_version() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("minecraft");
        let version_dir = root.join("versions/demo");
        fs::create_dir_all(&version_dir).unwrap();
        fs::write(
            version_dir.join("demo.json"),
            serde_json::to_vec(&json!({
                "id":"demo", "type":"release", "mainClass":"example.Main",
                "libraries":[], "javaVersion":{"majorVersion":21}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(version_dir.join("demo.jar"), b"existing game data").unwrap();
        let original = fs::read(version_dir.join("demo.jar")).unwrap();
        let config = temp.path().join("config.json");
        let store = PreferencesStore::new(temp.path().join("preferences"));
        store.select_root(root.clone()).unwrap();
        let entry = bootstrap(&store, &config, None).unwrap().entries.remove(0);

        update_library(
            &store,
            &config,
            None,
            CatalogRequest::Rename {
                key: entry.key.clone(),
                name: "My saved profile 日本語".into(),
            },
        )
        .unwrap();
        let hidden = update_library(
            &store,
            &config,
            None,
            CatalogRequest::Hide {
                key: entry.key.clone(),
            },
        )
        .unwrap();
        assert!(hidden.entries.is_empty());
        assert_eq!(hidden.hidden_entries, 1);
        assert_eq!(fs::read(version_dir.join("demo.jar")).unwrap(), original);

        let restored = update_library(&store, &config, None, CatalogRequest::RestoreAll).unwrap();
        assert_eq!(restored.entries[0].display_name, "My saved profile 日本語");
    }
    #[test]
    fn display_identity_survives_game_version_changes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("minecraft");
        let instance = root.join(".ccl/instances/demo");
        let game = instance.join("game");
        fs::create_dir_all(&game).unwrap();
        let game = fs::canonicalize(game).unwrap();
        let write_metadata = |version: &str| {
            fs::write(
                instance.join("instance.json"),
                serde_json::to_vec(&json!({
                    "id":"demo", "minecraft":version, "loader":null, "loader_version":null,
                    "game_directory":game
                }))
                .unwrap(),
            )
            .unwrap();
        };
        write_metadata("1.21.1");
        let config = ccl_core::config::DownloadConfig::default();
        let first = discover_entries(&root, &config).unwrap().remove(0);
        LibraryCatalog::new(&root)
            .set_display_name(&first.key, "我的世界 🌸")
            .unwrap();
        write_metadata("1.21.2");
        let second = discover_entries(&root, &config).unwrap().remove(0);
        assert_eq!(first.key, second.key);
        assert_eq!(second.display_name, "我的世界 🌸");
        assert_eq!(second.launch_version, "1.21.2");
    }
}
