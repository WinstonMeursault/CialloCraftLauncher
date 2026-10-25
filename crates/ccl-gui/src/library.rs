use crate::{GuiError, GuiResult, Preferences, PreferencesStore};
use ccl_core::{
    compatibility::inspect_compatibility, config::LauncherConfig, directory::DirectoryInputs,
    instances::InstanceService, launch::list_installed_versions,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct LibraryEntry {
    pub key: String,
    pub id: String,
    pub kind: &'static str,
    pub minecraft: String,
    pub loader: Option<ccl_core::loaders::LoaderKind>,
    pub game_directory: PathBuf,
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
    match list_installed_versions(&result.root) {
        Ok(versions) => result
            .entries
            .extend(versions.into_iter().map(|v| LibraryEntry {
                key: format!("vanilla:{}", v.id),
                minecraft: v.id.clone(),
                id: v.id,
                kind: "vanilla",
                loader: None,
                game_directory: result.root.clone(),
            })),
        Err(_) => result
            .warnings
            .push("Some shared versions could not be read; no files were replaced".into()),
    }
    match InstanceService::new(result.root.clone(), config.downloads).and_then(|s| s.list()) {
        Ok(instances) => result
            .entries
            .extend(instances.into_iter().map(|i| LibraryEntry {
                key: format!("managed:{}", i.id),
                id: i.id,
                kind: "managed",
                minecraft: i.minecraft,
                loader: i.loader,
                game_directory: i.game_directory,
            })),
        Err(_) => result.warnings.push(
            "Managed instances are busy or unreadable; retry after other operations finish".into(),
        ),
    }
    Ok(result)
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
