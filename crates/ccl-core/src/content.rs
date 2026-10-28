//! Local, user-owned instance content and portable userdata backups.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use thiserror::Error;
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::{
    config::DownloadConfig,
    instances::{InstanceError, InstanceService},
};

const MAX_BACKUP_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_BACKUP_ENTRIES: usize = 100_000;
const MAX_MANIFEST_BYTES: u64 = 32 * 1024 * 1024;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Mod,
    ResourcePack,
    ShaderPack,
    Save,
}

impl ContentKind {
    fn directory(self) -> &'static str {
        match self {
            Self::Mod => "mods",
            Self::ResourcePack => "resourcepacks",
            Self::ShaderPack => "shaderpacks",
            Self::Save => "saves",
        }
    }
    fn allows(self, name: &str, is_dir: bool) -> bool {
        match self {
            Self::Mod => {
                !is_dir
                    && (name.to_ascii_lowercase().ends_with(".jar")
                        || name.to_ascii_lowercase().ends_with(".jar.disabled"))
            }
            Self::ResourcePack => !is_dir && name.to_ascii_lowercase().ends_with(".zip"),
            Self::ShaderPack => !is_dir && name.to_ascii_lowercase().ends_with(".zip"),
            Self::Save => is_dir,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContentEntry {
    pub path: String,
    pub bytes: u64,
    pub enabled: bool,
}

#[derive(Debug, Error)]
pub enum ContentError {
    #[error(transparent)]
    Instance(#[from] InstanceError),
    #[error("unsafe content path: {0}")]
    UnsafePath(String),
    #[error("invalid content: {0}")]
    Invalid(String),
    #[error("backup is invalid: {0}")]
    InvalidBackup(String),
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("backup archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("backup manifest error: {0}")]
    Json(#[from] serde_json::Error),
}
pub type ContentResult<T> = Result<T, ContentError>;

#[derive(Clone)]
pub struct ContentService {
    instances: InstanceService,
}

impl ContentService {
    pub fn new(root: PathBuf, config: DownloadConfig) -> ContentResult<Self> {
        Ok(Self {
            instances: InstanceService::new(root, config)?,
        })
    }

    pub fn new_with_runtime_subdirectory(
        root: PathBuf,
        config: DownloadConfig,
        relative: PathBuf,
    ) -> ContentResult<Self> {
        Ok(Self {
            instances: InstanceService::new(root, config)?.with_runtime_subdirectory(relative)?,
        })
    }

    pub fn list(&self, id: &str, kind: ContentKind) -> ContentResult<Vec<ContentEntry>> {
        let guard = self.instances.acquire_launch(id)?;
        let directory = self
            .instances
            .runtime_path_for(guard.game_directory())
            .join(kind.directory());
        reject_symlink(&directory)?;
        let mut result = Vec::new();
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
            Err(e) => return Err(io(&directory, e)),
        };
        for item in entries {
            let item = item.map_err(|e| io(&directory, e))?;
            let path = item.path();
            reject_symlink(&path)?;
            let ty = item.file_type().map_err(|e| io(&path, e))?;
            if !(ty.is_file() || ty.is_dir()) {
                continue;
            }
            let name = item.file_name().to_string_lossy().into_owned();
            if !kind.allows(&name, ty.is_dir()) {
                continue;
            }
            validate_relative(Path::new(&name))?;
            result.push(ContentEntry {
                path: name.clone(),
                bytes: tree_size(&path)?,
                enabled: !(kind == ContentKind::Mod
                    && name.to_ascii_lowercase().ends_with(".jar.disabled")),
            });
        }
        result.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(result)
    }

    /// Copy a local mod, pack, or save into the instance. The source is never removed.
    pub fn add(
        &self,
        id: &str,
        kind: ContentKind,
        source: &Path,
        name: Option<&str>,
    ) -> ContentResult<ContentEntry> {
        reject_symlink(source)?;
        let source_type = fs::symlink_metadata(source).map_err(|e| io(source, e))?;
        let is_dir = source_type.is_dir();
        if !source_type.is_file() && !is_dir {
            return Err(ContentError::Invalid(
                "source must be a regular file or directory".into(),
            ));
        }
        let default = source
            .file_name()
            .ok_or_else(|| ContentError::Invalid("source has no filename".into()))?
            .to_string_lossy();
        let name = name.unwrap_or(&default);
        let target_rel = validate_relative(Path::new(name))?;
        if target_rel.components().count() != 1 {
            return Err(ContentError::UnsafePath(name.into()));
        }
        if !kind.allows(name, is_dir) {
            return Err(ContentError::Invalid(format!(
                "source does not match {kind:?} content type"
            )));
        }
        if is_dir {
            validate_tree(source)?;
        }
        let target_name = name.to_owned();
        self.instances.edit_content(id, move |game| {
            let dir = game.join(kind.directory());
            ensure_real_dir(game, &dir)?;
            let target = dir.join(&target_name);
            reject_symlink(&target)?;
            for entry in fs::read_dir(&dir).map_err(|e| io(&dir, e))? {
                let entry = entry.map_err(|e| io(&dir, e))?;
                if entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&target_name)
                {
                    return Err(ContentError::Invalid(format!(
                        "content name collides with an existing file: {target_name}"
                    )));
                }
            }
            if target.exists() {
                return Err(ContentError::Invalid(format!(
                    "content already exists: {target_name}"
                )));
            }
            if is_dir {
                copy_tree(source, &target)?;
            } else {
                fs::copy(source, &target).map_err(|e| io(&target, e))?;
            }
            Ok(ContentEntry {
                path: target_name.clone(),
                bytes: tree_size(&target)?,
                enabled: !(kind == ContentKind::Mod
                    && target_name.to_ascii_lowercase().ends_with(".disabled")),
            })
        })
    }

    pub fn remove(&self, id: &str, kind: ContentKind, name: &str) -> ContentResult<()> {
        let rel = validate_relative(Path::new(name))?;
        if rel.components().count() != 1 {
            return Err(ContentError::UnsafePath(name.into()));
        }
        self.instances.edit_content(id, move |game| {
            let path = game.join(kind.directory()).join(rel);
            reject_symlink(&path)?;
            if !path.exists() {
                return Err(ContentError::Invalid(format!("content not found: {name}")));
            }
            if !kind.allows(name, path.is_dir()) {
                return Err(ContentError::Invalid(
                    "content type does not match selected collection".into(),
                ));
            }
            if path.is_dir() {
                fs::remove_dir_all(&path).map_err(|e| io(&path, e))?;
            } else {
                fs::remove_file(&path).map_err(|e| io(&path, e))?;
            }
            Ok(())
        })
    }

    pub fn set_enabled(&self, id: &str, name: &str, enabled: bool) -> ContentResult<ContentEntry> {
        if !name.to_ascii_lowercase().ends_with(".jar")
            && !name.to_ascii_lowercase().ends_with(".jar.disabled")
        {
            return Err(ContentError::Invalid(
                "only .jar mods can be enabled or disabled".into(),
            ));
        }
        let rel = validate_relative(Path::new(name))?;
        if rel.components().count() != 1 {
            return Err(ContentError::UnsafePath(name.into()));
        }
        self.instances.edit_content(id, move |game| {
            let mods = game.join("mods");
            ensure_real_dir(game, &mods)?;
            let supplied = rel.file_name().unwrap().to_string_lossy();
            let base_name = if supplied.to_ascii_lowercase().ends_with(".jar.disabled") {
                supplied[..supplied.len() - ".disabled".len()].to_owned()
            } else {
                supplied.into_owned()
            };
            let disabled_name = format!("{base_name}.disabled");
            let find_case_insensitive = |candidate: &str| -> ContentResult<Option<PathBuf>> {
                let mut matches = Vec::new();
                for entry in fs::read_dir(&mods).map_err(|e| io(&mods, e))? {
                    let entry = entry.map_err(|e| io(&mods, e))?;
                    if entry
                        .file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(candidate)
                    {
                        matches.push(entry.path());
                    }
                }
                if matches.len() > 1 {
                    return Err(ContentError::Invalid(format!(
                        "multiple case-colliding mod files match {candidate}"
                    )));
                }
                if let Some(path) = matches.first() {
                    reject_symlink(path)?;
                }
                Ok(matches.into_iter().next())
            };
            let active = find_case_insensitive(&base_name)?;
            let disabled = find_case_insensitive(&disabled_name)?;
            if active.is_some() && disabled.is_some() {
                return Err(ContentError::Invalid(format!(
                    "both enabled and disabled versions exist for {base_name}"
                )));
            }
            let (current, current_enabled) = active
                .map(|path| (path, true))
                .or_else(|| disabled.map(|path| (path, false)))
                .ok_or_else(|| ContentError::Invalid(format!("mod not found: {name}")))?;
            if !current.is_file() {
                return Err(ContentError::Invalid(format!(
                    "mod path is not a regular file: {}",
                    current.display()
                )));
            }
            let current_name = current.file_name().unwrap().to_string_lossy();
            if current_enabled == enabled {
                return Ok(ContentEntry {
                    path: current_name.into_owned(),
                    bytes: fs::metadata(&current).map_err(|e| io(&current, e))?.len(),
                    enabled,
                });
            }
            let target_name = if enabled { base_name } else { disabled_name };
            let target = mods.join(&target_name);
            reject_symlink(&target)?;
            if target.exists() {
                return Err(ContentError::Invalid(format!(
                    "target mod already exists: {target_name}"
                )));
            }
            fs::rename(&current, &target).map_err(|e| io(&current, e))?;
            Ok(ContentEntry {
                path: target_name,
                bytes: fs::metadata(&target).map_err(|e| io(&target, e))?.len(),
                enabled,
            })
        })
    }

    pub fn backup(&self, id: &str, archive_path: &Path) -> ContentResult<()> {
        let guard = self.instances.acquire_launch(id)?;
        let game = self.instances.runtime_path_for(guard.game_directory());
        let parent = archive_path.parent().unwrap_or_else(|| Path::new("."));
        let game_canonical = fs::canonicalize(&game).map_err(|e| io(&game, e))?;
        if canonicalize_future(archive_path)?.starts_with(&game_canonical) {
            return Err(ContentError::Invalid(
                "backup destination must be outside the instance game directory".into(),
            ));
        }
        fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        reject_symlink(parent)?;
        reject_symlink(archive_path)?;
        if archive_path.exists() {
            return Err(ContentError::Invalid(
                "backup destination already exists".into(),
            ));
        }
        let temp = temp_path(parent, "ccl-backup");
        let _cleanup = TempCleanup(temp.clone());
        let file = File::create(&temp).map_err(|e| io(&temp, e))?;
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut records = Vec::new();
        let mut directories = Vec::new();
        let mut root_states = Vec::new();
        for root in BACKUP_ROOTS {
            let path = game.join(root);
            reject_symlink(&path)?;
            let present = path.exists();
            if present {
                let expected_dir = matches!(
                    *root,
                    "mods"
                        | "config"
                        | "resourcepacks"
                        | "shaderpacks"
                        | "saves"
                        | "defaultconfigs"
                        | "datapacks"
                );
                if path.is_dir() != expected_dir || (!expected_dir && !path.is_file()) {
                    return Err(ContentError::Invalid(format!(
                        "userdata root has the wrong type: {root}"
                    )));
                }
            }
            root_states.push(BackupRoot {
                path: (*root).to_owned(),
                present,
            });
            if present {
                collect_files(&game, &path, &mut records, &mut directories)?;
            }
        }
        records.sort_by(|a, b| a.0.cmp(&b.0));
        directories.sort();
        let mut names = BTreeSet::new();
        for (name, _) in &records {
            if !names.insert(name.to_lowercase()) {
                let _ = fs::remove_file(&temp);
                return Err(ContentError::Invalid(format!(
                    "userdata paths collide on Windows: {name}"
                )));
            }
        }
        if records.len().saturating_add(directories.len()) > MAX_BACKUP_ENTRIES {
            let _ = fs::remove_file(&temp);
            return Err(ContentError::Invalid("too many files to back up".into()));
        }
        for name in &directories {
            if !names.insert(name.to_lowercase()) {
                let _ = fs::remove_file(&temp);
                return Err(ContentError::Invalid(format!(
                    "userdata paths collide on Windows: {name}"
                )));
            }
        }
        let mut manifest_files = Vec::new();
        let mut total = 0u64;
        for (relative, path) in records {
            let expected_len = fs::metadata(&path).map_err(|e| io(&path, e))?.len();
            total = total.saturating_add(expected_len);
            if expected_len > MAX_BACKUP_BYTES || total > MAX_BACKUP_BYTES {
                return Err(ContentError::Invalid("backup exceeds size limit".into()));
            }
            let mut input = File::open(&path).map_err(|e| io(&path, e))?;
            zip.start_file(&relative, options)?;
            let mut hasher = Sha512::new();
            let mut copied = 0u64;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let amount = input.read(&mut buffer).map_err(|e| io(&path, e))?;
                if amount == 0 {
                    break;
                }
                copied = copied.saturating_add(amount as u64);
                if copied > expected_len || copied > MAX_BACKUP_BYTES {
                    return Err(ContentError::Invalid(
                        "file changed or exceeded backup size limit".into(),
                    ));
                }
                hasher.update(&buffer[..amount]);
                zip.write_all(&buffer[..amount]).map_err(|e| io(&temp, e))?;
            }
            if copied != expected_len {
                return Err(ContentError::Invalid(format!(
                    "file changed while backing up: {}",
                    path.display()
                )));
            }
            manifest_files.push(BackupFile {
                path: relative,
                size: copied,
                sha512: format!("{:x}", hasher.finalize()),
            });
        }
        let manifest = BackupManifest {
            format: 1,
            roots: root_states,
            directories,
            files: manifest_files,
        };
        let bytes = serde_json::to_vec(&manifest)?;
        zip.start_file("manifest.json", options)?;
        zip.write_all(&bytes).map_err(|e| io(&temp, e))?;
        let file = zip.finish()?;
        file.sync_all().map_err(|e| io(&temp, e))?;
        if let Err(error) = fs::hard_link(&temp, archive_path) {
            let _ = fs::remove_file(&temp);
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                return Err(ContentError::Invalid(
                    "backup destination already exists".into(),
                ));
            }
            return Err(io(archive_path, error));
        }
        fs::remove_file(&temp).map_err(|e| io(&temp, e))?;
        Ok(())
    }

    /// Verify and stage the complete archive before replacing any selected userdata.
    pub fn restore(&self, id: &str, archive_path: &Path) -> ContentResult<()> {
        reject_symlink(archive_path)?;
        let archive_path = archive_path.to_path_buf();
        self.instances.edit_content(id, move |game| {
            let input = File::open(&archive_path).map_err(|e| io(&archive_path, e))?;
            let mut archive = ZipArchive::new(input)?;
            if archive.len() > MAX_BACKUP_ENTRIES + 1 {
                return Err(ContentError::InvalidBackup("too many entries".into()));
            }
            let mut manifest_data = None;
            let mut names = BTreeSet::new();
            for i in 0..archive.len() {
                let entry = archive.by_index(i)?;
                if entry.is_dir() {
                    return Err(ContentError::InvalidBackup(
                        "directory entries are not allowed".into(),
                    ));
                }
                let name = entry.name()?.into_owned();
                if name == "manifest.json" {
                    if manifest_data.is_some() || entry.size() > MAX_MANIFEST_BYTES {
                        return Err(ContentError::InvalidBackup("invalid manifest entry".into()));
                    }
                    let mut b = Vec::new();
                    entry
                        .take(MAX_MANIFEST_BYTES + 1)
                        .read_to_end(&mut b)
                        .map_err(|e| io(&archive_path, e))?;
                    if b.len() as u64 > MAX_MANIFEST_BYTES {
                        return Err(ContentError::InvalidBackup(
                            "manifest exceeds size limit".into(),
                        ));
                    }
                    manifest_data = Some(b);
                    continue;
                }
                let p = validate_backup_name(&name)?;
                let key = name.to_lowercase();
                if !names.insert(key) {
                    return Err(ContentError::InvalidBackup(
                        "duplicate or case-colliding paths".into(),
                    ));
                }
                if entry.size() > MAX_BACKUP_BYTES {
                    return Err(ContentError::InvalidBackup(
                        "file exceeds size limit".into(),
                    ));
                }
                let _ = p;
            }
            let raw = manifest_data
                .ok_or_else(|| ContentError::InvalidBackup("manifest.json is missing".into()))?;
            let manifest: BackupManifest = serde_json::from_slice(&raw)?;
            if manifest.format != 1
                || manifest
                    .files
                    .len()
                    .saturating_add(manifest.directories.len())
                    > MAX_BACKUP_ENTRIES
            {
                return Err(ContentError::InvalidBackup(
                    "unsupported manifest format or size".into(),
                ));
            }
            let expected_roots: BTreeSet<String> =
                BACKUP_ROOTS.iter().map(|r| (*r).to_owned()).collect();
            let mut roots = BTreeMap::new();
            for root in &manifest.roots {
                if !expected_roots.contains(&root.path)
                    || roots.insert(root.path.as_str(), root.present).is_some()
                {
                    return Err(ContentError::InvalidBackup(
                        "invalid or duplicate root state".into(),
                    ));
                }
            }
            if roots.len() != BACKUP_ROOTS.len() {
                return Err(ContentError::InvalidBackup(
                    "manifest omits a userdata root state".into(),
                ));
            }
            for root in ["options.txt", "servers.dat"] {
                let present = roots.get(root).copied() == Some(true);
                let listed = manifest.files.iter().any(|file| file.path == root);
                if present != listed {
                    return Err(ContentError::InvalidBackup(format!(
                        "file root state does not match manifest: {root}"
                    )));
                }
            }
            let mut listed = BTreeSet::new();
            let mut total = 0u64;
            let stage = temp_path(game, ".ccl-restore");
            fs::create_dir(&stage).map_err(|e| io(&stage, e))?;
            let _cleanup = DirCleanup(stage.clone());
            for file in &manifest.files {
                validate_backup_name(&file.path)?;
                let root_name = file.path.split('/').next().unwrap_or("");
                let directory_root = matches!(
                    root_name,
                    "mods"
                        | "config"
                        | "resourcepacks"
                        | "shaderpacks"
                        | "saves"
                        | "defaultconfigs"
                        | "datapacks"
                );
                if (directory_root && !file.path.contains('/'))
                    || (!directory_root && file.path.contains('/'))
                    || roots.get(root_name).copied() != Some(true)
                {
                    return Err(ContentError::InvalidBackup(format!(
                        "file path conflicts with userdata root state: {}",
                        file.path
                    )));
                }
                let key = file.path.to_lowercase();
                if !listed.insert(key) {
                    return Err(ContentError::InvalidBackup(
                        "duplicate manifest paths".into(),
                    ));
                }
                total = total.saturating_add(file.size);
                if total > MAX_BACKUP_BYTES {
                    return Err(ContentError::InvalidBackup(
                        "backup exceeds size limit".into(),
                    ));
                }
                let entry = archive.by_name(&file.path).map_err(|_| {
                    ContentError::InvalidBackup(format!("missing file: {}", file.path))
                })?;
                if entry.size() != file.size {
                    return Err(ContentError::InvalidBackup(format!(
                        "size mismatch: {}",
                        file.path
                    )));
                }
                let target = stage.join(validate_backup_name(&file.path)?);
                let parent = target.parent().unwrap();
                fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
                let mut output = File::create(&target).map_err(|e| io(&target, e))?;
                let mut input = entry.take(file.size.saturating_add(1));
                let mut hasher = Sha512::new();
                let mut copied = 0u64;
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let amount = input.read(&mut buffer).map_err(|e| io(&archive_path, e))?;
                    if amount == 0 {
                        break;
                    }
                    copied = copied.saturating_add(amount as u64);
                    if copied > file.size {
                        return Err(ContentError::InvalidBackup(format!(
                            "size mismatch: {}",
                            file.path
                        )));
                    }
                    hasher.update(&buffer[..amount]);
                    output
                        .write_all(&buffer[..amount])
                        .map_err(|e| io(&target, e))?;
                }
                if copied != file.size {
                    return Err(ContentError::InvalidBackup(format!(
                        "size mismatch: {}",
                        file.path
                    )));
                }
                if format!("{:x}", hasher.finalize()) != file.sha512 {
                    return Err(ContentError::InvalidBackup(format!(
                        "hash mismatch: {}",
                        file.path
                    )));
                }
            }
            if listed != names {
                return Err(ContentError::InvalidBackup(
                    "archive entries do not match manifest".into(),
                ));
            }
            for directory in &manifest.directories {
                validate_backup_name(directory)?;
                let root_name = directory.split('/').next().unwrap_or("");
                if !directory.contains('/')
                    || !matches!(
                        root_name,
                        "mods"
                            | "config"
                            | "resourcepacks"
                            | "shaderpacks"
                            | "saves"
                            | "defaultconfigs"
                            | "datapacks"
                    )
                    || roots.get(root_name).copied() != Some(true)
                {
                    return Err(ContentError::InvalidBackup(format!(
                        "invalid directory path: {directory}"
                    )));
                }
                let key = directory.to_lowercase();
                if !listed.insert(key) {
                    return Err(ContentError::InvalidBackup(format!(
                        "duplicate or colliding path: {directory}"
                    )));
                }
                let dir_key = directory.to_lowercase();
                for file in &manifest.files {
                    let file_key = file.path.to_lowercase();
                    if file_key == dir_key || dir_key.starts_with(&format!("{file_key}/")) {
                        return Err(ContentError::InvalidBackup(format!(
                            "file conflicts with directory: {directory}"
                        )));
                    }
                }
            }
            (|| -> ContentResult<()> {
                for name in &manifest.directories {
                    let target = stage.join(validate_backup_name(name)?);
                    fs::create_dir_all(&target).map_err(|e| io(&target, e))?;
                }
                // All archive data has been verified. The surrounding instance transaction commits
                // this stage only after this closure succeeds.
                for root in BACKUP_ROOTS {
                    let staged = stage.join(root);
                    let current = game.join(root);
                    reject_symlink(&current)?;
                    if current.exists() {
                        if current.is_dir() {
                            fs::remove_dir_all(&current).map_err(|e| io(&current, e))?;
                        } else {
                            fs::remove_file(&current).map_err(|e| io(&current, e))?;
                        }
                    }
                    if roots.get(*root).copied().unwrap_or(false) {
                        if !staged.exists() {
                            fs::create_dir_all(&staged).map_err(|e| io(&staged, e))?;
                        }
                        fs::rename(&staged, &current).map_err(|e| io(&current, e))?;
                    }
                }
                Ok(())
            })()
        })
    }
}

const BACKUP_ROOTS: &[&str] = &[
    "mods",
    "config",
    "resourcepacks",
    "shaderpacks",
    "saves",
    "defaultconfigs",
    "datapacks",
    "options.txt",
    "servers.dat",
];
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupManifest {
    format: u32,
    roots: Vec<BackupRoot>,
    directories: Vec<String>,
    files: Vec<BackupFile>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupRoot {
    path: String,
    present: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupFile {
    path: String,
    size: u64,
    sha512: String,
}

fn validate_backup_name(name: &str) -> ContentResult<PathBuf> {
    let path = validate_relative(Path::new(name))?;
    let first = path
        .components()
        .next()
        .and_then(|c| {
            if let Component::Normal(s) = c {
                Some(s.to_string_lossy())
            } else {
                None
            }
        })
        .unwrap_or_default();
    if !BACKUP_ROOTS.iter().any(|r| first.eq_ignore_ascii_case(r)) {
        return Err(ContentError::InvalidBackup(format!(
            "path is outside userdata roots: {name}"
        )));
    }
    if path.components().count() == 1 && first.eq_ignore_ascii_case("manifest.json") {
        return Err(ContentError::InvalidBackup(
            "reserved manifest filename".into(),
        ));
    }
    Ok(path)
}
fn validate_relative(path: &Path) -> ContentResult<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(ContentError::UnsafePath(path.display().to_string()));
    }
    let mut out = PathBuf::new();
    for c in path.components() {
        let Component::Normal(part) = c else {
            return Err(ContentError::UnsafePath(path.display().to_string()));
        };
        let s = part.to_string_lossy();
        if s.is_empty()
            || s == "."
            || s == ".."
            || s.ends_with([' ', '.'])
            || s.contains([':', '\\'])
            || s.chars().any(char::is_control)
            || is_windows_reserved(&s)
        {
            return Err(ContentError::UnsafePath(path.display().to_string()));
        }
        out.push(part);
    }
    Ok(out)
}
fn is_windows_reserved(s: &str) -> bool {
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}
fn reject_symlink(path: &Path) -> ContentResult<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            Err(ContentError::UnsafePath(path.display().to_string()))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(path, e)),
    }
}
fn ensure_real_dir(base: &Path, path: &Path) -> ContentResult<()> {
    reject_symlink(base)?;
    reject_symlink(path)?;
    if !path.exists() {
        fs::create_dir_all(path).map_err(|e| io(path, e))?;
    }
    if !path.is_dir() {
        return Err(ContentError::Invalid(format!(
            "not a directory: {}",
            path.display()
        )));
    }
    Ok(())
}
fn tree_size(path: &Path) -> ContentResult<u64> {
    reject_symlink(path)?;
    let m = fs::metadata(path).map_err(|e| io(path, e))?;
    if m.is_file() {
        return Ok(m.len());
    }
    let mut n = 0u64;
    for e in fs::read_dir(path).map_err(|e| io(path, e))? {
        let p = e.map_err(|e| io(path, e))?.path();
        reject_symlink(&p)?;
        n = n.saturating_add(tree_size(&p)?);
    }
    Ok(n)
}
fn validate_tree(path: &Path) -> ContentResult<()> {
    let mut names = BTreeSet::new();
    validate_tree_inner(path, path, &mut names)
}
fn validate_tree_inner(
    root: &Path,
    path: &Path,
    names: &mut BTreeSet<String>,
) -> ContentResult<()> {
    reject_symlink(path)?;
    if path.is_dir() {
        for e in fs::read_dir(path).map_err(|e| io(path, e))? {
            let p = e.map_err(|e| io(path, e))?.path();
            let relative = p.strip_prefix(root).unwrap_or(&p);
            validate_relative(relative)?;
            let key = relative.to_string_lossy().replace('\\', "/").to_lowercase();
            if !names.insert(key) {
                return Err(ContentError::UnsafePath(relative.display().to_string()));
            }
            validate_tree_inner(root, &p, names)?;
        }
    }
    Ok(())
}
fn copy_tree(source: &Path, target: &Path) -> ContentResult<()> {
    reject_symlink(source)?;
    if source.is_dir() {
        fs::create_dir(target).map_err(|e| io(target, e))?;
        for e in fs::read_dir(source).map_err(|e| io(source, e))? {
            let p = e.map_err(|e| io(source, e))?.path();
            let dest = target.join(p.file_name().unwrap());
            copy_tree(&p, &dest)?;
        }
    } else {
        fs::copy(source, target).map_err(|e| io(target, e))?;
    }
    Ok(())
}
fn collect_files(
    game: &Path,
    path: &Path,
    out: &mut Vec<(String, PathBuf)>,
    directories: &mut Vec<String>,
) -> ContentResult<()> {
    reject_symlink(path)?;
    if path.is_dir() {
        for e in fs::read_dir(path).map_err(|e| io(path, e))? {
            collect_files(game, &e.map_err(|e| io(path, e))?.path(), out, directories)?;
        }
    } else {
        let rel = path
            .strip_prefix(game)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        validate_backup_name(&rel)?;
        out.push((rel, path.to_path_buf()));
        return Ok(());
    }
    if path
        != game.join(
            path.strip_prefix(game)
                .unwrap()
                .components()
                .next()
                .unwrap()
                .as_os_str(),
        )
    {
        let rel = path
            .strip_prefix(game)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        validate_backup_name(&rel)?;
        directories.push(rel);
    }
    Ok(())
}
fn temp_path(parent: &Path, prefix: &str) -> PathBuf {
    let n = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    parent.join(format!("{prefix}-{}-{t}-{n}.tmp", std::process::id()))
}
fn canonicalize_future(path: &Path) -> ContentResult<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| io(Path::new("."), e))?
            .join(path)
    };
    let mut ancestor = absolute.clone();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        let name = ancestor
            .file_name()
            .ok_or_else(|| ContentError::UnsafePath(path.display().to_string()))?
            .to_os_string();
        suffix.push(name);
        if !ancestor.pop() {
            return Err(ContentError::UnsafePath(path.display().to_string()));
        }
    }
    let mut result = fs::canonicalize(&ancestor).map_err(|e| io(&ancestor, e))?;
    for part in suffix.iter().rev() {
        result.push(part);
    }
    Ok(result)
}
struct TempCleanup(PathBuf);
impl Drop for TempCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
struct DirCleanup(PathBuf);
impl Drop for DirCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn io(path: &Path, source: std::io::Error) -> ContentError {
    ContentError::Io {
        path: path.into(),
        source,
    }
}
