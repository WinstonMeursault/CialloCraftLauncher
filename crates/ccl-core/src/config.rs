use std::{
    fs,
    path::{Path, PathBuf},
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DownloadConfig {
    pub version_list_source: SourceMode,
    pub file_source: SourceMode,
    pub mirror_root: String,
    pub http_proxy: Option<String>,
    pub max_parallel: usize,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            version_list_source: SourceMode::Auto,
            file_source: SourceMode::Auto,
            mirror_root: "https://bmclapi2.bangbang93.com".into(),
            http_proxy: None,
            max_parallel: 8,
        }
    }
}

/// User configuration. Unknown keys are rejected to catch misspelled settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LauncherConfig {
    pub downloads: DownloadConfig,
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
        if path.exists() {
            Self::read(path)
        } else {
            Ok(Self::default())
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
        Ok(())
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

    #[test]
    fn rejects_unknown_keys_and_invalid_limits() {
        assert!(serde_json::from_str::<LauncherConfig>(r#"{"downlaods":{}}"#).is_err());
        let mut config = LauncherConfig::default();
        config.downloads.max_parallel = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_mirror_credentials() {
        let mut config = LauncherConfig::default();
        config.downloads.mirror_root = "https://user:password@example.com".into();
        assert!(config.validate().is_err());
    }
}
