use crate::{GuiError, GuiResult};
use base64::Engine as _;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

fn decode_background(path: &Path) -> GuiResult<image::DynamicImage> {
    let metadata = fs::metadata(path).map_err(|_| {
        GuiError::new(
            "invalid_background",
            "Choose a readable JPG, PNG, or WebP image",
        )
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 32 * 1024 * 1024 {
        return Err(GuiError::new(
            "invalid_background",
            "Background image must be a regular file under 32 MB",
        ));
    }
    let mut reader = image::ImageReader::open(path)
        .map_err(|_| GuiError::new("invalid_background", "Cannot open background image"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(12000);
    limits.max_image_height = Some(12000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| {
        GuiError::new(
            "invalid_background",
            "Unsupported or invalid image; choose JPG, PNG, or WebP",
        )
    })?;
    let (w, h) = (image.width(), image.height());
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 40_000_000 {
        return Err(GuiError::new(
            "invalid_background",
            "Background image dimensions exceed the limit",
        ));
    }
    Ok(image.thumbnail(1920, 1080).to_rgb8().into())
}

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
    #[serde(default)]
    pub reduce_transparency: bool,
    #[serde(default)]
    pub reduce_motion: bool,
    #[serde(default)]
    pub last_selected_instance: Option<String>,
    #[serde(default)]
    pub background_image: Option<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            language: "zh-CN".into(),
            reduce_transparency: false,
            reduce_motion: false,
            last_selected_instance: None,
            background_image: None,
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
        if self
            .last_selected_instance
            .as_ref()
            .is_some_and(|key| key.is_empty() || key.len() > 512 || key.contains('\0'))
        {
            return Err(GuiError::new(
                "invalid_preferences",
                "Invalid selected instance key",
            ));
        }
        if self
            .background_image
            .as_deref()
            .is_some_and(|name| name != "background.jpg")
        {
            return Err(GuiError::new(
                "invalid_preferences",
                "Invalid managed background image",
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
    #[serde(default)]
    config: Option<PathBuf>,
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
        if document.schema != 1
            || document.root.as_ref().is_some_and(|p| !p.is_absolute())
            || document.config.as_ref().is_some_and(|p| !p.is_absolute())
        {
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
    pub fn config_path(&self) -> GuiResult<Option<PathBuf>> {
        Ok(self.read()?.config)
    }
    pub fn select_config(&self, config: PathBuf) -> GuiResult<()> {
        if !config.is_absolute() || !config.is_file() {
            return Err(GuiError::new(
                "invalid_config_path",
                "Choose an existing configuration file",
            ));
        }
        self.update(|d| d.config = Some(config))
    }
    pub fn root(&self) -> GuiResult<Option<PathBuf>> {
        Ok(self.read()?.root)
    }
    pub fn save(&self, preferences: Preferences) -> GuiResult<Preferences> {
        preferences.validate()?;
        self.update(|d| d.preferences = preferences.clone())?;
        Ok(preferences)
    }
    /// Save appearance controls without overwriting independently updated selection or wallpaper state.
    pub fn save_appearance(&self, preferences: Preferences) -> GuiResult<Preferences> {
        preferences.validate()?;
        self.update(|d| {
            d.preferences.theme = preferences.theme;
            d.preferences.language = preferences.language;
            d.preferences.reduce_transparency = preferences.reduce_transparency;
            d.preferences.reduce_motion = preferences.reduce_motion;
        })?;
        self.load()
    }
    pub fn select_instance(&self, key: Option<String>) -> GuiResult<Preferences> {
        let mut current = self.load()?;
        current.last_selected_instance = key;
        current.validate()?;
        self.update(|d| {
            d.preferences.last_selected_instance = current.last_selected_instance.clone()
        })?;
        self.load()
    }
    pub fn import_background(&self, source: &Path) -> GuiResult<Preferences> {
        let image = decode_background(source)?;
        fs::create_dir_all(&self.directory)
            .map_err(|_| GuiError::new("preferences_io", "Cannot create appearance directory"))?;
        let path = self.directory.join("background.jpg");
        let mut bytes = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut bytes, image::ImageFormat::Jpeg)
            .map_err(|_| {
                GuiError::new("invalid_background", "Cannot encode selected background")
            })?;
        if bytes.get_ref().len() > 8 * 1024 * 1024 {
            return Err(GuiError::new(
                "invalid_background",
                "Background image exceeds the cache size limit",
            ));
        }
        let mut staged = tempfile::NamedTempFile::new_in(&self.directory)
            .map_err(|_| GuiError::new("preferences_io", "Cannot stage background image"))?;
        staged
            .write_all(bytes.get_ref())
            .and_then(|()| staged.as_file().sync_all())
            .map_err(|_| GuiError::new("preferences_io", "Cannot save background image"))?;
        staged
            .persist(path)
            .map_err(|_| GuiError::new("preferences_io", "Cannot replace background image"))?;
        let mut current = self.load()?;
        current.background_image = Some("background.jpg".into());
        self.update(|d| d.preferences.background_image = current.background_image.clone())?;
        self.load()
    }
    pub fn reset_background(&self) -> GuiResult<Preferences> {
        self.update(|d| d.preferences.background_image = None)?;
        let path = self.directory.join("background.jpg");
        if path.exists() {
            let _ = fs::remove_file(path);
        }
        self.load()
    }
    pub fn background_data_url(&self) -> GuiResult<Option<String>> {
        if self.load()?.background_image.is_none() {
            return Ok(None);
        }
        let path = self.directory.join("background.jpg");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => {
                return Err(GuiError::new(
                    "preferences_io",
                    "Cannot inspect background image",
                ));
            }
        };
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > 8 * 1024 * 1024
        {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        fs::File::open(path)
            .and_then(|file| file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes))
            .map_err(|_| GuiError::new("preferences_io", "Cannot read background image"))?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Ok(None);
        }
        Ok(Some(format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        )))
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
