use crate::catalog::{CatalogRequest, LibraryCatalog};
use crate::{GuiError, GuiResult, Preferences, PreferencesStore};
use ccl_core::{
    compatibility::inspect_compatibility,
    config::LauncherConfig,
    directory::DirectoryInputs,
    instances::InstanceService,
    launch::{list_installed_versions, load_installed_metadata},
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
    pub memory_override: Option<ccl_core::memory::MemoryPolicy>,
}
#[derive(Serialize)]
pub struct LaunchDefaults {
    pub java: Option<PathBuf>,
    pub max_memory_mb: u32,
    pub auto_memory: bool,
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
            auto_memory: true,
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
        auto_memory: config.launch.auto_memory,
        auto_install_java: config.launch.auto_install_java,
        network_offline: config.downloads.offline,
    };
    if !result.root_exists {
        return Ok(result);
    }
    let catalog = LibraryCatalog::new(result.root.clone());
    match discover_entries_state(&result.root, &config.downloads) {
        Ok((mut entries, busy)) => {
            for entry in &mut entries {
                let key = ccl_core::memory::profile_key(&entry.game_directory, &entry.id)
                    .map_err(|error| GuiError::new("configuration_error", error.to_string()))?;
                entry.memory_override = config.launch.instance_memory.get(&key).copied();
            }
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
        let (minecraft, loader) = root_profile_identity(&root, &id);
        entries.push(LibraryEntry {
            key,
            id: id.clone(),
            display_name,
            has_revision_storage: false,
            minecraft,
            loader,
            game_directory: directory,
            launch_version,
            memory_override: None,
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
            memory_override: None,
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

/// Derive the descriptive identity from version metadata while leaving `id` as the launch target.
/// A custom profile name is never treated as a Minecraft version unless it is a version-like ID.
fn root_profile_identity(root: &Path, id: &str) -> (String, Option<ccl_core::loaders::LoaderKind>) {
    let Ok(metadata) = load_installed_metadata(root, id) else {
        return ("Unknown".into(), None);
    };
    let raw_path = root.join("versions").join(id).join(format!("{id}.json"));
    let raw: serde_json::Value = std::fs::read(&raw_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();

    let libraries = metadata
        .libraries
        .iter()
        .map(|library| library.name.as_str())
        .collect::<Vec<_>>();
    let loader = if libraries.iter().any(|name| {
        name.starts_with("net.minecraftforge:fmlloader:")
            || name.starts_with("net.minecraftforge:forge:")
    }) {
        Some(ccl_core::loaders::LoaderKind::Forge)
    } else if libraries
        .iter()
        .any(|name| name.starts_with("net.neoforged:neoforge:"))
    {
        Some(ccl_core::loaders::LoaderKind::NeoForge)
    } else if libraries
        .iter()
        .any(|name| name.starts_with("net.fabricmc:fabric-loader:"))
    {
        Some(ccl_core::loaders::LoaderKind::Fabric)
    } else if libraries
        .iter()
        .any(|name| name.starts_with("org.quiltmc:quilt-loader:"))
    {
        Some(ccl_core::loaders::LoaderKind::Quilt)
    } else if libraries
        .iter()
        .any(|name| name.starts_with("net.legacyfabric:legacy-fabric-loader:"))
    {
        Some(ccl_core::loaders::LoaderKind::LegacyFabric)
    } else if libraries
        .iter()
        .any(|name| name.starts_with("optifine:OptiFine:"))
    {
        Some(ccl_core::loaders::LoaderKind::OptiFine)
    } else {
        None
    };

    let inherited = raw
        .get("inheritsFrom")
        .and_then(serde_json::Value::as_str)
        .or(metadata.inherits_from.as_deref());
    let minecraft = inherited
        .and_then(version_like)
        .or_else(|| {
            raw.get("minecraftVersion")
                .or_else(|| raw.get("clientVersion"))
                .and_then(serde_json::Value::as_str)
                .and_then(version_like)
        })
        .or_else(|| loader.and_then(|kind| loader_minecraft_version(kind, &libraries)))
        .or_else(|| version_like(id))
        .unwrap_or_else(|| "Unknown".into());
    (minecraft, loader)
}

fn version_like(value: &str) -> Option<String> {
    let candidate = value.strip_prefix("minecraft:").unwrap_or(value);
    let bytes = candidate.as_bytes();
    if bytes.len() == 6
        && bytes[2] == b'w'
        && bytes[5].is_ascii_lowercase()
        && [0, 1, 3, 4].iter().all(|&i| bytes[i].is_ascii_digit())
    {
        return Some(candidate.to_owned());
    }
    let numeric = if let Some((base, suffix)) = candidate.split_once('-') {
        let number = suffix
            .strip_prefix("pre")
            .or_else(|| suffix.strip_prefix("rc"))?;
        if number.is_empty() || !number.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        base
    } else {
        candidate
    };
    let mut parts = numeric.split('.');
    let first = parts.next()?;
    if first.is_empty() || !first.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let rest = parts.collect::<Vec<_>>();
    if rest.is_empty()
        || rest
            .iter()
            .any(|part| part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some(candidate.to_owned())
}

fn loader_minecraft_version(
    kind: ccl_core::loaders::LoaderKind,
    libraries: &[&str],
) -> Option<String> {
    let prefixes: &[&str] = match kind {
        ccl_core::loaders::LoaderKind::Forge => {
            &["net.minecraftforge:fmlloader:", "net.minecraftforge:forge:"]
        }
        ccl_core::loaders::LoaderKind::NeoForge => &["net.neoforged:neoforge:"],
        _ => return None,
    };
    libraries.iter().find_map(|name| {
        prefixes.iter().find_map(|prefix| {
            name.strip_prefix(prefix)
                .and_then(|version| version_like(version.split_once('-')?.0))
        })
    })
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
        CatalogRequest::Memory {
            key,
            memory,
            expected,
        } => {
            let entry = current
                .entries
                .iter()
                .find(|entry| entry.key == key)
                .ok_or_else(|| {
                    GuiError::new(
                        "library_entry_missing",
                        "This library entry is no longer available.",
                    )
                })?;
            let key = ccl_core::memory::profile_key(&entry.game_directory, &entry.id)
                .map_err(|error| GuiError::new("configuration_error", error.to_string()))?;
            ccl_core::config::ConfigStore::new(config_path)
                .update(|config| {
                    if config.launch.instance_memory.get(&key).copied() != expected {
                        return Err(ccl_core::Error::InvalidConfig(
                            "Instance memory changed in another process; reload before saving"
                                .into(),
                        ));
                    }
                    if let Some(policy) = memory {
                        config.launch.instance_memory.insert(key, policy);
                    } else {
                        config.launch.instance_memory.remove(&key);
                    }
                    Ok(())
                })
                .map_err(|error| GuiError::new("configuration_error", error.to_string()))?;
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

    fn write_root_version(root: &Path, id: &str, metadata: serde_json::Value) {
        let directory = root.join("versions").join(id);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("{id}.json")),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn instance_memory_persists_is_shared_and_rejects_stale_edits() {
        use ccl_core::{config::LauncherConfig, memory::MemoryPolicy};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("games");
        fs::create_dir_all(&root).unwrap();
        write_root_version(
            &root,
            "中文 with spaces",
            json!({"id":"中文 with spaces", "mainClass":"Main"}),
        );
        let original =
            fs::read(root.join("versions/中文 with spaces/中文 with spaces.json")).unwrap();
        let config = temp.path().join("config.json");
        let store = PreferencesStore::new(temp.path().join("preferences"));
        store.select_root(root.clone()).unwrap();
        let initial = bootstrap(&store, &config, None).unwrap();
        assert!(initial.launch_defaults.auto_memory);
        let key = initial.entries[0].key.clone();
        assert_eq!(initial.entries[0].memory_override, None);
        let saved = update_library(
            &store,
            &config,
            None,
            CatalogRequest::Memory {
                key: key.clone(),
                memory: Some(MemoryPolicy::Manual(6144)),
                expected: None,
            },
        )
        .unwrap();
        assert_eq!(
            saved.entries[0].memory_override,
            Some(MemoryPolicy::Manual(6144))
        );
        let loaded = LauncherConfig::read(&config).unwrap();
        assert_eq!(
            loaded
                .launch
                .memory_policy(&root, "中文 with spaces", None)
                .unwrap(),
            MemoryPolicy::Manual(6144)
        );
        assert!(
            update_library(
                &store,
                &config,
                None,
                CatalogRequest::Memory {
                    key: key.clone(),
                    memory: Some(MemoryPolicy::AUTO),
                    expected: None
                }
            )
            .is_err()
        );
        update_library(
            &store,
            &config,
            None,
            CatalogRequest::Rename {
                key: key.clone(),
                name: "New display name".into(),
            },
        )
        .unwrap();
        let reopened = bootstrap(
            &PreferencesStore::new(temp.path().join("preferences")),
            &config,
            None,
        )
        .unwrap();
        assert_eq!(
            reopened.entries[0].memory_override,
            Some(MemoryPolicy::Manual(6144))
        );
        let inherited = update_library(
            &store,
            &config,
            None,
            CatalogRequest::Memory {
                key,
                memory: None,
                expected: Some(MemoryPolicy::Manual(6144)),
            },
        )
        .unwrap();
        assert_eq!(inherited.entries[0].memory_override, None);
        assert_eq!(
            fs::read(root.join("versions/中文 with spaces/中文 with spaces.json")).unwrap(),
            original
        );
    }

    #[test]
    fn root_profile_identity_uses_inheritance_and_loader_metadata() {
        assert_eq!(version_like("24w14a").as_deref(), Some("24w14a"));
        assert_eq!(version_like("1.21-pre1").as_deref(), Some("1.21-pre1"));
        assert!(version_like("1.20.1-custom").is_none());
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write_root_version(
            root,
            "1.20.1",
            json!({"id":"1.20.1", "type":"release", "mainClass":"net.minecraft.client.main.Main", "libraries":[]}),
        );
        write_root_version(
            root,
            "BMC4 + Maid Integrations",
            json!({
                "id":"BMC4 + Maid Integrations", "mainClass":"cpw.mods.bootstraplauncher.BootstrapLauncher",
                "libraries":[{"name":"net.minecraftforge:fmlloader:1.20.1-47.4.20"}]
            }),
        );
        write_root_version(
            root,
            "Closing Song 1.6.6.1",
            json!({
                "id":"Closing Song 1.6.6.1", "inheritsFrom":"1.20.1",
                "mainClass":"cpw.mods.bootstraplauncher.BootstrapLauncher", "libraries":[]
            }),
        );
        write_root_version(
            root,
            "Plain Custom Profile",
            json!({
                "id":"Plain Custom Profile", "mainClass":"net.minecraft.client.main.Main", "libraries":[]
            }),
        );

        assert_eq!(
            root_profile_identity(root, "BMC4 + Maid Integrations"),
            ("1.20.1".into(), Some(ccl_core::loaders::LoaderKind::Forge))
        );
        assert_eq!(
            root_profile_identity(root, "Closing Song 1.6.6.1"),
            ("1.20.1".into(), None)
        );
        assert_eq!(
            root_profile_identity(root, "1.20.1"),
            ("1.20.1".into(), None)
        );
        assert_eq!(
            root_profile_identity(root, "Plain Custom Profile"),
            ("Unknown".into(), None)
        );
    }

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
