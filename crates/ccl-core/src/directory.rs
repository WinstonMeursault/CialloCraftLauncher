use std::{
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{Error, Result};

/// Inputs are explicit so directory policy can be tested without changing process state.
#[derive(Debug, Clone)]
pub struct DirectoryInputs {
    pub working_directory: PathBuf,
    pub home_directory: PathBuf,
    pub platform_directory: Option<PathBuf>,
    pub explicit_directory: Option<PathBuf>,
}

impl DirectoryInputs {
    pub fn from_environment(explicit_directory: Option<PathBuf>) -> Result<Self> {
        let working_directory = env::current_dir().map_err(|source| Error::Io {
            path: PathBuf::from("."),
            source,
        })?;
        let home_directory = env::var_os(if cfg!(target_os = "windows") {
            "USERPROFILE"
        } else {
            "HOME"
        })
        .map(PathBuf::from)
        .ok_or_else(|| Error::InvalidConfig("user home directory is unavailable".into()))?;
        let platform_directory = if cfg!(target_os = "windows") {
            env::var_os("APPDATA").map(|root| PathBuf::from(root).join(".minecraft"))
        } else if cfg!(target_os = "macos") {
            Some(home_directory.join("Library/Application Support/minecraft"))
        } else {
            Some(home_directory.join(".minecraft"))
        };
        Ok(Self {
            working_directory,
            home_directory,
            platform_directory,
            explicit_directory,
        })
    }

    pub fn resolve(&self) -> Result<DirectoryResolution> {
        if let Some(explicit) = &self.explicit_directory {
            let path = if explicit.is_absolute() {
                explicit.clone()
            } else {
                self.working_directory.join(explicit)
            };
            return Ok(DirectoryResolution {
                exists: is_existing_directory(&path)?,
                path,
                origin: DirectoryOrigin::Explicit,
            });
        }

        let local = self.working_directory.join(".minecraft");
        if is_existing_directory(&local)? {
            return Ok(DirectoryResolution {
                path: local,
                origin: DirectoryOrigin::WorkingDirectory,
                exists: true,
            });
        }
        if let Some(platform) = &self.platform_directory
            && is_existing_directory(platform)?
        {
            return Ok(DirectoryResolution {
                path: platform.clone(),
                origin: DirectoryOrigin::PlatformDefault,
                exists: true,
            });
        }
        let home = self.home_directory.join(".minecraft");
        if is_existing_directory(&home)? {
            return Ok(DirectoryResolution {
                path: home,
                origin: DirectoryOrigin::HomeDirectory,
                exists: true,
            });
        }
        Ok(DirectoryResolution {
            path: local,
            origin: DirectoryOrigin::NewWorkingDirectory,
            exists: false,
        })
    }
}

fn is_existing_directory(path: &Path) -> Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(Error::NotDirectory(path.to_path_buf())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectoryOrigin {
    Explicit,
    WorkingDirectory,
    PlatformDefault,
    HomeDirectory,
    NewWorkingDirectory,
}

/// Resolved game root. Resolution is read-only; call `ensure_exists` to create a missing root.
#[derive(Debug, Clone, Serialize)]
pub struct DirectoryResolution {
    pub path: PathBuf,
    pub origin: DirectoryOrigin,
    pub exists: bool,
}

impl DirectoryResolution {
    pub fn ensure_exists(&self) -> Result<()> {
        fs::create_dir_all(&self.path).map_err(|source| Error::Io {
            path: self.path.clone(),
            source,
        })?;
        is_existing_directory(&self.path)?;
        tracing::info!(path = %self.path.display(), "game directory ready");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    fn sandbox() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        loop {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                env::temp_dir().join(format!("ccl-path-test-{}-{nonce}-{id}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return path,
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create test directory: {error}"),
            }
        }
    }

    #[test]
    fn reuses_existing_directory_before_creating_one() {
        let root = sandbox();
        let work = root.join("work");
        let home = root.join("home");
        let platform = root.join("platform-game");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir(&platform).unwrap();
        let mut inputs = DirectoryInputs {
            working_directory: work.clone(),
            home_directory: home,
            platform_directory: Some(platform.clone()),
            explicit_directory: None,
        };
        let resolution = inputs.resolve().unwrap();
        assert_eq!(resolution.path, platform);
        assert!(!work.join(".minecraft").exists());
        fs::create_dir(work.join(".minecraft")).unwrap();
        assert_eq!(
            inputs.resolve().unwrap().origin,
            DirectoryOrigin::WorkingDirectory
        );
        inputs.explicit_directory = Some(PathBuf::from("other"));
        let explicit = inputs.resolve().unwrap();
        assert_eq!(explicit.path, work.join("other"));
        assert!(!explicit.exists);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_at_game_root_is_an_error() {
        let root = sandbox();
        fs::write(root.join(".minecraft"), b"not a directory").unwrap();
        let inputs = DirectoryInputs {
            working_directory: root.clone(),
            home_directory: root.clone(),
            platform_directory: None,
            explicit_directory: None,
        };
        assert!(matches!(inputs.resolve(), Err(Error::NotDirectory(_))));
        fs::remove_dir_all(root).unwrap();
    }
}
