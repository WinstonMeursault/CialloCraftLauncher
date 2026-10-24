use crate::{GuiError, GuiResult};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub theme: Theme,
    pub language: String,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            language: "zh-CN".into(),
        }
    }
}
impl Preferences {
    fn validate(&self) -> GuiResult<()> {
        if !matches!(self.language.as_str(), "en" | "zh-CN") {
            return Err(GuiError::new(
                "invalid_preferences",
                "Unsupported interface language",
            ));
        }
        Ok(())
    }
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema: u32,
    preferences: Preferences,
    root: Option<PathBuf>,
}

#[derive(Clone)]
pub struct PreferencesStore {
    directory: PathBuf,
}
impl PreferencesStore {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    fn read(&self) -> GuiResult<Document> {
        let path = self.directory.join("gui.json");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Document {
                    schema: 1,
                    ..Document::default()
                });
            }
            Err(_) => {
                return Err(GuiError::new(
                    "preferences_io",
                    "Cannot inspect GUI preferences",
                ));
            }
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 {
            return Err(GuiError::new(
                "invalid_preferences",
                "GUI preferences must be a bounded regular file",
            ));
        }
        let mut bytes = Vec::new();
        fs::File::open(&path)
            .and_then(|f| f.take(64 * 1024 + 1).read_to_end(&mut bytes))
            .map_err(|_| GuiError::new("preferences_io", "Cannot read GUI preferences"))?;
        if bytes.len() > 64 * 1024 {
            return Err(GuiError::new(
                "invalid_preferences",
                "GUI preferences exceed size limit",
            ));
        }
        let document: Document = serde_json::from_slice(&bytes).map_err(|_| {
            GuiError::new(
                "invalid_preferences",
                "Invalid GUI preferences; the file was not overwritten",
            )
        })?;
        if document.schema != 1 || document.root.as_ref().is_some_and(|p| !p.is_absolute()) {
            return Err(GuiError::new(
                "invalid_preferences",
                "Unsupported GUI preferences schema or root",
            ));
        }
        document.preferences.validate()?;
        Ok(document)
    }
    pub fn load(&self) -> GuiResult<Preferences> {
        Ok(self.read()?.preferences)
    }
    pub fn root(&self) -> GuiResult<Option<PathBuf>> {
        Ok(self.read()?.root)
    }
    pub fn save(&self, preferences: Preferences) -> GuiResult<Preferences> {
        preferences.validate()?;
        self.update(|d| d.preferences = preferences.clone())?;
        Ok(preferences)
    }
    pub fn select_root(&self, root: PathBuf) -> GuiResult<()> {
        if !root.is_absolute() || !root.is_dir() {
            return Err(GuiError::new(
                "invalid_root",
                "Choose an existing absolute game directory",
            ));
        }
        self.update(|d| d.root = Some(root))
    }
    fn update(&self, edit: impl FnOnce(&mut Document)) -> GuiResult<()> {
        fs::create_dir_all(&self.directory).map_err(|_| {
            GuiError::new("preferences_io", "Cannot create GUI preferences directory")
        })?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.directory.join("gui.lock"))
            .map_err(|_| GuiError::new("preferences_io", "Cannot open GUI preferences lock"))?;
        lock.lock_exclusive()
            .map_err(|_| GuiError::new("preferences_io", "Cannot lock GUI preferences"))?;
        let mut document = self.read()?;
        edit(&mut document);
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)
            .map_err(|_| GuiError::new("preferences_io", "Cannot stage GUI preferences"))?;
        let bytes = serde_json::to_vec_pretty(&document)
            .map_err(|_| GuiError::new("preferences_io", "Cannot encode GUI preferences"))?;
        temporary
            .write_all(&bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|_| GuiError::new("preferences_io", "Cannot write GUI preferences"))?;
        temporary
            .persist(self.directory.join("gui.json"))
            .map_err(|_| GuiError::new("preferences_io", "Cannot replace GUI preferences"))?;
        Ok(())
    }
}
