//! User-facing library metadata stored separately from game and instance files.
//!
//! The catalog never adopts or moves game data. Its keys are unified SHA-256
//! descriptors (`instance:<hex>`), independent of where an entry was discovered.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
};
use tempfile::NamedTempFile;

use crate::{GuiError, GuiResult};

const SCHEMA: u32 = 1;
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CatalogRecord {
    pub display_name: String,
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogRequest {
    Rename { key: String, name: String },
    Hide { key: String },
    RestoreAll,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    schema: u32,
    #[serde(default)]
    entries: BTreeMap<String, CatalogRecord>,
}

impl Default for CatalogFile {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            entries: BTreeMap::new(),
        }
    }
}

/// Persistent metadata for the unified library. Each edit is serialized under a
/// cross-process lock and committed with an atomic same-directory rename.
#[derive(Debug, Clone)]
pub struct LibraryCatalog {
    root: PathBuf,
}

impl LibraryCatalog {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn directory(&self) -> PathBuf {
        self.root.join(".ccl")
    }
    fn path(&self) -> PathBuf {
        self.directory().join("library.json")
    }
    fn lock_path(&self) -> PathBuf {
        self.directory().join("library.lock")
    }

    fn read_file(&self) -> GuiResult<CatalogFile> {
        let directory = self.directory();
        match fs::symlink_metadata(&directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CatalogFile::default());
            }
            Err(error) => {
                return Err(catalog_error(format!(
                    "Cannot inspect library metadata: {error}"
                )));
            }
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(catalog_error("Library metadata directory is unsafe"));
            }
            Ok(_) => {}
        }
        let path = self.path();
        let metadata = match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CatalogFile::default());
            }
            Err(error) => {
                return Err(catalog_error(format!(
                    "Cannot inspect library catalog: {error}"
                )));
            }
            Ok(metadata) => metadata,
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(catalog_error("Library catalog is not a regular file"));
        }
        if metadata.len() > MAX_BYTES {
            return Err(catalog_error("Library catalog exceeds the size limit"));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        fs::File::open(path)
            .and_then(|file| file.take(MAX_BYTES + 1).read_to_end(&mut bytes))
            .map_err(|error| catalog_error(format!("Cannot read library catalog: {error}")))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(catalog_error("Library catalog exceeds the size limit"));
        }
        let catalog: CatalogFile = serde_json::from_slice(&bytes)
            .map_err(|error| catalog_error(format!("Library catalog is unreadable: {error}")))?;
        if catalog.schema != SCHEMA {
            return Err(catalog_error("Library catalog uses an unsupported version"));
        }
        for (key, record) in &catalog.entries {
            validate_key(key)?;
            validate_display_name(&record.display_name)?;
        }
        Ok(catalog)
    }

    pub fn records(&self) -> GuiResult<BTreeMap<String, CatalogRecord>> {
        Ok(self.read_file()?.entries)
    }

    pub fn set_display_name(&self, key: &str, display_name: &str) -> GuiResult<()> {
        validate_key(key)?;
        let display_name = display_name.trim();
        validate_display_name(display_name)?;
        self.update(|file| {
            let entry = file
                .entries
                .entry(key.to_owned())
                .or_insert_with(|| CatalogRecord {
                    display_name: key.rsplit(':').next().unwrap_or(key).to_owned(),
                    hidden: false,
                });
            entry.display_name = display_name.to_owned();
        })
    }

    /// Hide an entry from the visible library. Game files and catalog identity remain intact.
    pub fn hide(&self, key: &str, default_name: &str) -> GuiResult<()> {
        validate_key(key)?;
        validate_display_name(default_name)?;
        self.update(|file| {
            file.entries
                .entry(key.to_owned())
                .or_insert_with(|| CatalogRecord {
                    display_name: default_name.to_owned(),
                    hidden: false,
                })
                .hidden = true;
        })
    }

    /// Restore all hidden records. Discovery still controls which entries are shown.
    pub fn restore_all(&self) -> GuiResult<()> {
        self.update(|file| {
            for record in file.entries.values_mut() {
                record.hidden = false;
            }
        })
    }

    fn update(&self, edit: impl FnOnce(&mut CatalogFile)) -> GuiResult<()> {
        let root_meta = fs::symlink_metadata(&self.root)
            .map_err(|error| catalog_error(format!("Cannot access game directory: {error}")))?;
        if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
            return Err(catalog_error("Game directory is unsafe"));
        }
        let directory = self.directory();
        match fs::symlink_metadata(&directory) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(catalog_error("Library metadata directory is unsafe"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Err(error) = fs::create_dir(&directory)
                    && error.kind() != std::io::ErrorKind::AlreadyExists
                {
                    return Err(catalog_error(format!(
                        "Cannot create library metadata directory: {error}"
                    )));
                }
                let metadata = fs::symlink_metadata(&directory).map_err(|error| {
                    catalog_error(format!(
                        "Cannot inspect library metadata directory: {error}"
                    ))
                })?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(catalog_error("Library metadata directory is unsafe"));
                }
            }
            Err(error) => {
                return Err(catalog_error(format!(
                    "Cannot inspect library metadata directory: {error}"
                )));
            }
        }
        let lock_path = self.lock_path();
        if let Ok(metadata) = fs::symlink_metadata(&lock_path)
            && (metadata.file_type().is_symlink() || !metadata.is_file())
        {
            return Err(catalog_error("Library catalog lock is unsafe"));
        }
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| catalog_error(format!("Cannot lock library catalog: {error}")))?;
        lock.lock_exclusive()
            .map_err(|error| catalog_error(format!("Cannot lock library catalog: {error}")))?;
        let result = (|| {
            let mut file = self.read_file()?;
            edit(&mut file);
            let path = self.path();
            let mut temporary = NamedTempFile::new_in(&directory)
                .map_err(|error| catalog_error(format!("Cannot write library catalog: {error}")))?;
            serde_json::to_writer_pretty(&mut temporary, &file).map_err(|error| {
                catalog_error(format!("Cannot encode library catalog: {error}"))
            })?;
            temporary
                .write_all(b"\n")
                .map_err(|error| catalog_error(format!("Cannot write library catalog: {error}")))?;
            if temporary
                .as_file()
                .metadata()
                .map_err(|error| catalog_error(format!("Cannot inspect library catalog: {error}")))?
                .len()
                > MAX_BYTES
            {
                return Err(catalog_error("Library catalog exceeds the size limit"));
            }
            temporary
                .as_file()
                .sync_all()
                .map_err(|error| catalog_error(format!("Cannot save library catalog: {error}")))?;
            temporary.persist(&path).map_err(|error| {
                catalog_error(format!("Cannot commit library catalog: {}", error.error))
            })?;
            Ok(())
        })();
        let unlock_result = lock.unlock();
        result?;
        unlock_result
            .map_err(|error| catalog_error(format!("Cannot unlock library catalog: {error}")))
    }
}

fn validate_key(key: &str) -> GuiResult<()> {
    let Some(hash) = key.strip_prefix("instance:") else {
        return Err(catalog_error("Invalid library key"));
    };
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(catalog_error("Invalid library key"));
    }
    Ok(())
}

fn validate_display_name(name: &str) -> GuiResult<()> {
    if name.trim().is_empty()
        || name.chars().count() > 256
        || name.len() > 1024
        || name.chars().any(char::is_control)
    {
        return Err(catalog_error(
            "Display name must contain 1 to 256 printable characters (up to 1024 UTF-8 bytes)",
        ));
    }
    Ok(())
}

fn catalog_error(message: impl Into<String>) -> GuiError {
    GuiError::new("library_catalog", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_and_hidden_state_persist_without_touching_game_data() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        fs::create_dir_all(root.join("versions/external")).unwrap();
        fs::write(
            root.join("versions/external/external.json"),
            b"external-version",
        )
        .unwrap();
        let original = fs::read(root.join("versions/external/external.json")).unwrap();
        let catalog = LibraryCatalog::new(&root);
        let key = format!("instance:{}", "a".repeat(64));

        catalog
            .set_display_name(&key, "我的 existing world")
            .unwrap();
        assert_eq!(
            catalog.records().unwrap()[&key].display_name,
            "我的 existing world"
        );
        catalog.hide(&key, "Original profile name").unwrap();
        assert_eq!(
            catalog.records().unwrap()[&key].display_name,
            "我的 existing world"
        );
        assert!(catalog.records().unwrap()[&key].hidden);
        catalog.restore_all().unwrap();
        assert!(!catalog.records().unwrap()[&key].hidden);
        assert_eq!(
            fs::read(root.join("versions/external/external.json")).unwrap(),
            original
        );
    }

    #[test]
    fn unreadable_or_future_catalog_is_never_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let catalog = LibraryCatalog::new(temp.path());
        fs::create_dir_all(temp.path().join(".ccl")).unwrap();
        let path = temp.path().join(".ccl/library.json");
        let bytes = br#"{"schema":99,"entries":{}}"#;
        fs::write(&path, bytes).unwrap();

        assert!(
            catalog
                .set_display_name(&format!("instance:{}", "b".repeat(64)), "Demo")
                .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn malformed_catalog_is_never_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let catalog = LibraryCatalog::new(temp.path());
        fs::create_dir_all(temp.path().join(".ccl")).unwrap();
        let path = temp.path().join(".ccl/library.json");
        let bytes = b"{not valid json";
        fs::write(&path, bytes).unwrap();

        assert!(
            catalog
                .set_display_name(&format!("instance:{}", "c".repeat(64)), "Demo")
                .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn catalog_edit_waits_for_lock_and_preserves_other_records() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let catalog = LibraryCatalog::new(&root);
        let first_key = format!("instance:{}", "d".repeat(64));
        let second_key = format!("instance:{}", "e".repeat(64));
        catalog.set_display_name(&first_key, "First").unwrap();
        let lock_path = root.join(".ccl/library.lock");
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(lock_path)
            .unwrap();
        lock.lock_exclusive().unwrap();

        let worker_root = root.clone();
        let worker_key = second_key.clone();
        let worker = std::thread::spawn(move || {
            LibraryCatalog::new(worker_root)
                .set_display_name(&worker_key, "Second")
                .unwrap();
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
        lock.unlock().unwrap();
        worker.join().unwrap();

        let records = catalog.records().unwrap();
        assert_eq!(records[&first_key].display_name, "First");
        assert_eq!(records[&second_key].display_name, "Second");
    }

    #[test]
    fn parallel_catalog_edits_do_not_overwrite_each_other() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let workers = (0..8)
            .map(|index| {
                let root = root.clone();
                std::thread::spawn(move || {
                    let catalog = LibraryCatalog::new(root);
                    let key = format!("instance:{index:064x}");
                    catalog
                        .set_display_name(&key, &format!("Entry {index}"))
                        .unwrap();
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(LibraryCatalog::new(root).records().unwrap().len(), 8);
    }
}
