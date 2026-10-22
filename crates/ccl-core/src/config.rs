use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Preference for official or mirrored download endpoints.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMode {
    #[default]
    Auto,
    Official,
    Mirror,
}

/// Download settings shared by command-line and future graphical clients.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DownloadConfig {
    /// Disable all network access; valid local artifacts can still be reused.
    pub offline: bool,
    pub version_list_source: SourceMode,
    pub file_source: SourceMode,
    pub mirror_root: String,
    pub http_proxy: Option<String>,
    pub max_parallel: usize,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            offline: false,
            version_list_source: SourceMode::Auto,
            file_source: SourceMode::Auto,
            mirror_root: "https://bmclapi2.bangbang93.com".into(),
            http_proxy: None,
            max_parallel: 8,
        }
    }
}

impl std::fmt::Debug for DownloadConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadConfig")
            .field("version_list_source", &self.version_list_source)
            .field("offline", &self.offline)
            .field("file_source", &self.file_source)
            .field("mirror_root", &self.mirror_root)
            .field("http_proxy", &self.http_proxy.as_deref().map(redact_proxy))
            .field("max_parallel", &self.max_parallel)
            .finish()
    }
}

/// Microsoft OAuth application configuration. This is a public client ID, never a secret.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuthConfig {
    pub microsoft_client_id: Option<String>,
}

/// Defaults used when preparing a game launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LaunchConfig {
    pub java: Option<PathBuf>,
    pub max_memory_mb: u32,
    pub auto_install_java: bool,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            java: None,
            max_memory_mb: 2048,
            auto_install_java: true,
        }
    }
}

/// User configuration. Missing keys inherit defaults for backward compatibility.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LauncherConfig {
    pub downloads: DownloadConfig,
    pub auth: AuthConfig,
    pub launch: LaunchConfig,
}

impl LauncherConfig {
    pub fn read(path: &Path) -> Result<Self> {
        let contents = fs::read(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Self =
            serde_json::from_slice(&contents).map_err(|source| Error::ConfigJson {
                path: path.to_path_buf(),
                source,
            })?;
        config.validate()?;
        Ok(config)
    }

    pub fn read_or_default(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(contents) => {
                let config: Self =
                    serde_json::from_slice(&contents).map_err(|source| Error::ConfigJson {
                        path: path.to_path_buf(),
                        source,
                    })?;
                config.validate()?;
                Ok(config)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(Error::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.downloads.max_parallel == 0 || self.downloads.max_parallel > 64 {
            return Err(Error::InvalidConfig(
                "downloads.max_parallel must be in 1..=64".into(),
            ));
        }
        let mirror = reqwest::Url::parse(&self.downloads.mirror_root).map_err(|_| {
            Error::InvalidConfig("downloads.mirror_root must be a valid HTTPS URL".into())
        })?;
        if mirror.scheme() != "https"
            || mirror.host_str().is_none()
            || !mirror.username().is_empty()
            || mirror.password().is_some()
        {
            return Err(Error::InvalidConfig(
                "downloads.mirror_root must be an HTTPS URL without credentials".into(),
            ));
        }
        if let Some(proxy) = &self.downloads.http_proxy {
            reqwest::Proxy::all(proxy)
                .map_err(|_| Error::InvalidConfig("downloads.http_proxy is invalid".into()))?;
        }
        if self.launch.max_memory_mb < 512 || self.launch.max_memory_mb > 131_072 {
            return Err(Error::InvalidConfig(
                "launch.max_memory_mb must be in 512..=131072".into(),
            ));
        }
        if let Some(id) = &self.auth.microsoft_client_id
            && !is_public_client_id(id)
        {
            return Err(Error::InvalidConfig(
                "auth.microsoft_client_id must be a UUID-format public client ID".into(),
            ));
        }
        Ok(())
    }

    /// JSON for displaying settings, with any proxy user information and password hidden.
    pub fn display_json(&self) -> Result<serde_json::Value> {
        let mut value = serde_json::to_value(self).map_err(|source| Error::ConfigJson {
            path: PathBuf::from("<memory>"),
            source,
        })?;
        if let Some(proxy) = self.downloads.http_proxy.as_deref() {
            value["downloads"]["http_proxy"] = serde_json::Value::String(redact_proxy(proxy));
        }
        Ok(value)
    }
}

fn is_public_client_id(value: &str) -> bool {
    let parts: Vec<_> = value.split('-').collect();
    [8, 4, 4, 4, 12]
        .iter()
        .zip(parts.iter())
        .all(|(len, part)| part.len() == *len && part.bytes().all(|b| b.is_ascii_hexdigit()))
        && parts.len() == 5
}

fn redact_proxy(proxy: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(proxy) else {
        return "<redacted>".into();
    };
    if url.username().is_empty() && url.password().is_none() {
        return proxy.to_owned();
    }
    let _ = url.set_username("redacted");
    let _ = url.set_password(Some("redacted"));
    url.to_string()
}

/// A process-safe settings store. Use [`ConfigStore::update`] for multi-setting changes;
/// it holds an exclusive sidecar lock across read, validation, and atomic replacement.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self, key: &str) -> Result<serde_json::Value> {
        let config = LauncherConfig::read_or_default(&self.path)?;
        config.validate()?;
        let value = config.display_json()?;
        lookup(&value, key)
            .cloned()
            .ok_or_else(|| Error::InvalidConfig(format!("unknown setting: {key}")))
    }

    pub fn set(&self, key: &str, value: serde_json::Value) -> Result<()> {
        self.update(|config| set_value(config, key, value))
    }

    /// Reset one dotted setting, or the complete configuration when key is None.
    pub fn reset(&self, key: Option<&str>) -> Result<()> {
        if key.is_none() {
            let _lock = FileLock::acquire(&self.path)?;
            return write_atomic(&self.path, &LauncherConfig::default());
        }
        self.update(|config| {
            if let Some(key) = key {
                let default = LauncherConfig::default();
                let defaults = serde_json::to_value(default).expect("serializing defaults");
                let value = lookup(&defaults, key)
                    .cloned()
                    .ok_or_else(|| Error::InvalidConfig(format!("unknown setting: {key}")))?;
                set_value(config, key, value)
            } else {
                *config = LauncherConfig::default();
                Ok(())
            }
        })
    }

    /// Serializes read-modify-write across processes so concurrent CLI commands cannot
    /// silently overwrite each other's settings. The closure must not recursively use this store.
    pub fn update<T>(&self, update: impl FnOnce(&mut LauncherConfig) -> Result<T>) -> Result<T> {
        let _lock = FileLock::acquire(&self.path)?;
        let mut config = LauncherConfig::read_or_default(&self.path)?;
        let result = update(&mut config)?;
        config.validate()?;
        write_atomic(&self.path, &config)?;
        Ok(result)
    }
}

fn set_value(config: &mut LauncherConfig, key: &str, value: serde_json::Value) -> Result<()> {
    let mut json = serde_json::to_value(&*config).expect("serializing config");
    let mut cursor = &mut json;
    let mut value = Some(value);
    let mut parts = key.split('.').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            let Some(object) = cursor.as_object_mut() else {
                return Err(Error::InvalidConfig(format!("unknown setting: {key}")));
            };
            if !object.contains_key(part) {
                return Err(Error::InvalidConfig(format!("unknown setting: {key}")));
            }
            object.insert(
                part.to_owned(),
                value.take().expect("setting value inserted once"),
            );
        } else {
            cursor = cursor
                .get_mut(part)
                .ok_or_else(|| Error::InvalidConfig(format!("unknown setting: {key}")))?;
        }
    }
    *config = serde_json::from_value(json).map_err(|source| Error::ConfigJson {
        path: PathBuf::from("<memory>"),
        source,
    })?;
    Ok(())
}

fn lookup<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    key.split('.')
        .try_fold(value, |current, part| current.get(part))
}

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

fn write_atomic(path: &Path, config: &LauncherConfig) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|source| Error::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let bytes = serde_json::to_vec_pretty(config).map_err(|source| Error::ConfigJson {
        path: path.to_path_buf(),
        source,
    })?;
    let mut temporary = path.to_path_buf();
    let suffix = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    temporary.set_extension(format!("tmp-{}-{suffix}", std::process::id()));
    let write_result = (|| {
        let mut file = create_private_file(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        if let Ok(dir) = OpenOptions::new().read(true).open(parent) {
            let _ = dir.sync_all();
        }
        Ok::<_, std::io::Error>(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result.map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> std::io::Result<fs::File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

struct FileLock {
    _file: fs::File,
}
impl FileLock {
    fn acquire(config_path: &Path) -> Result<Self> {
        let mut lock_path = config_path.as_os_str().to_owned();
        lock_path.push(".lock");
        let lock_path = PathBuf::from(lock_path);
        if let Some(parent) = lock_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|source| Error::Io {
                path: lock_path.clone(),
                source,
            })?;
        fs2::FileExt::lock_exclusive(&file).map_err(|source| Error::Io {
            path: lock_path,
            source,
        })?;
        Ok(Self { _file: file })
    }
}

/// Conventional user-level configuration location for the current platform.
pub fn default_config_path(
    home: &Path,
    appdata: Option<&Path>,
    xdg_config_home: Option<&Path>,
) -> PathBuf {
    if cfg!(target_os = "windows") {
        appdata
            .unwrap_or(home)
            .join("CialloCraftLauncher")
            .join("config.json")
    } else if cfg!(target_os = "macos") {
        home.join("Library/Application Support/CialloCraftLauncher/config.json")
    } else {
        xdg_config_home
            .unwrap_or(&home.join(".config"))
            .join("ciallocraftlauncher/config.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    fn temp_config() -> PathBuf {
        std::env::temp_dir().join(format!(
            "ccl-config-test-{}-{}.json",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn defaults_and_old_config_are_compatible() {
        let old: LauncherConfig =
            serde_json::from_str(r#"{"downloads":{"max_parallel":4}}"#).unwrap();
        assert_eq!(old.launch.max_memory_mb, 2048);
        assert!(old.launch.auto_install_java);
        assert!(old.auth.microsoft_client_id.is_none());
        assert!(!old.downloads.offline);
        let offline: LauncherConfig =
            serde_json::from_str(r#"{"downloads":{"offline":true}}"#).unwrap();
        assert!(offline.downloads.offline);
    }

    #[test]
    fn validates_settings_and_redacts_proxy_credentials() {
        let mut config = LauncherConfig::default();
        config.downloads.http_proxy = Some("http://alice:s3cret@proxy.example:8080".into());
        let display = config.display_json().unwrap().to_string();
        assert!(!display.contains("s3cret"));
        assert!(display.contains("redacted"));
        config.auth.microsoft_client_id = Some("not-a-uuid".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn store_sets_resets_and_creates_parent_atomically() {
        let path = temp_config().join("nested/config.json");
        let store = ConfigStore::new(&path);
        store
            .set("downloads.max_parallel", serde_json::json!(4))
            .unwrap();
        store
            .set("launch.auto_install_java", serde_json::json!(false))
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(store.get("downloads.max_parallel").unwrap(), 4);
        store.reset(Some("downloads.max_parallel")).unwrap();
        assert_eq!(store.get("downloads.max_parallel").unwrap(), 8);
        assert!(
            !store
                .get("launch.auto_install_java")
                .unwrap()
                .as_bool()
                .unwrap()
        );
        fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn get_does_not_create_a_missing_config_directory() {
        let path = temp_config().join("nested/config.json");
        let parent = path.parent().unwrap().to_path_buf();
        assert_eq!(
            ConfigStore::new(&path).get("launch.max_memory_mb").unwrap(),
            2048
        );
        assert!(!parent.exists());
    }

    #[test]
    fn failed_atomic_replace_preserves_the_existing_path() {
        let target = temp_config();
        fs::create_dir_all(&target).unwrap();
        let marker = target.join("keep.txt");
        fs::write(&marker, b"original").unwrap();
        assert!(write_atomic(&target, &LauncherConfig::default()).is_err());
        assert_eq!(fs::read(&marker).unwrap(), b"original");
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn concurrent_updates_do_not_lose_changes() {
        let path = temp_config();
        let store = Arc::new(ConfigStore::new(&path));
        let gate = Arc::new(Barrier::new(3));
        let mut joins = vec![];
        for i in 0..2 {
            let store = store.clone();
            let gate = gate.clone();
            joins.push(thread::spawn(move || {
                gate.wait();
                store
                    .set(
                        if i == 0 {
                            "downloads.max_parallel"
                        } else {
                            "launch.auto_install_java"
                        },
                        if i == 0 {
                            serde_json::json!(3)
                        } else {
                            serde_json::json!(false)
                        },
                    )
                    .unwrap();
            }));
        }
        gate.wait();
        for join in joins {
            join.join().unwrap();
        }
        let config = LauncherConfig::read(&path).unwrap();
        assert_eq!(config.downloads.max_parallel, 3);
        assert!(!config.launch.auto_install_java);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_unknown_keys_and_invalid_limits() {
        assert!(serde_json::from_str::<LauncherConfig>(r#"{"downlaods":{}}"#).is_err());
        let mut config = LauncherConfig::default();
        config.downloads.max_parallel = 0;
        assert!(config.validate().is_err());
    }
}
