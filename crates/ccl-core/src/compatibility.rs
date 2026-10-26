//! Read-only inspection of CCL data before a binary update or rollback.
//!
//! The current formats require no migration. A future migration must lock each affected
//! metadata file, retain a recoverable backup, stage the converted value, and commit it
//! atomically. It must be idempotent or recoverable after interruption, must never copy
//! operating-system credentials into files, and must reject rollback when the older binary
//! cannot read the resulting format. Game directories are never restored as a rollback
//! strategy.

use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::config::LauncherConfig;

/// Highest versioned receipt schema this binary can read. Unversioned legacy metadata
/// remains supported through its existing readers.
pub const MAX_SUPPORTED_RECEIPT_SCHEMA: u32 = 1;
const MAX_METADATA_BYTES: u64 = 2 * 1024 * 1024;
const MAX_REPORT_ITEMS: usize = 10_000;
const MAX_DIRECTORY_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityStatus {
    Compatible,
    Missing,
    Incompatible,
    Unreadable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityItem {
    pub path: PathBuf,
    pub format: String,
    pub status: CompatibilityStatus,
    /// Safe diagnostic only; never includes file contents.
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityReport {
    pub compatible: bool,
    pub items: Vec<CompatibilityItem>,
}

/// Inspect the launcher configuration and CCL-owned metadata below `game_root`.
/// Does not follow symlinks, write files, access the network, inspect credentials, or read
/// general game data. `config_path` is also used to locate the adjacent `accounts.json`.
pub fn inspect_compatibility(config_path: &Path, game_root: &Path) -> CompatibilityReport {
    let mut items = Vec::new();
    inspect_config(config_path, &mut items);
    inspect_accounts(&config_path.with_file_name("accounts.json"), &mut items);
    inspect_instances(&game_root.join(".ccl/instances"), &mut items);
    let compatible = items.iter().all(|i| {
        matches!(
            i.status,
            CompatibilityStatus::Compatible | CompatibilityStatus::Missing
        )
    });
    CompatibilityReport { compatible, items }
}

fn push(
    items: &mut Vec<CompatibilityItem>,
    path: &Path,
    format: &str,
    status: CompatibilityStatus,
    detail: impl Into<String>,
) {
    if items.len() >= MAX_REPORT_ITEMS {
        return;
    }
    if items.len() == MAX_REPORT_ITEMS - 1 {
        items.push(CompatibilityItem {
            path: path.to_owned(),
            format: "scan_limit".into(),
            status: CompatibilityStatus::Incompatible,
            detail: "compatibility report limit reached; remaining data was not inspected".into(),
        });
        return;
    }
    items.push(CompatibilityItem {
        path: path.to_owned(),
        format: format.into(),
        status,
        detail: detail.into(),
    });
}

fn inspect_config(path: &Path, items: &mut Vec<CompatibilityItem>) {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => push(
            items,
            path,
            "launcher_config",
            CompatibilityStatus::Missing,
            "configuration is absent; defaults apply",
        ),
        Err(_) => push(
            items,
            path,
            "launcher_config",
            CompatibilityStatus::Unreadable,
            "configuration metadata cannot be inspected",
        ),
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => push(
            items,
            path,
            "launcher_config",
            CompatibilityStatus::Incompatible,
            "expected a regular file",
        ),
        Ok(meta) if meta.len() > MAX_METADATA_BYTES => push(
            items,
            path,
            "launcher_config",
            CompatibilityStatus::Incompatible,
            "configuration exceeds the metadata size limit",
        ),
        Ok(_) => match read_limited(path) {
            Err(_) => push(
                items,
                path,
                "launcher_config",
                CompatibilityStatus::Unreadable,
                "configuration cannot be read",
            ),
            Ok(Some(bytes)) => match serde_json::from_slice::<LauncherConfig>(&bytes) {
                Ok(c) if c.validate().is_ok() => push(
                    items,
                    path,
                    "launcher_config",
                    CompatibilityStatus::Compatible,
                    "legacy schema-less and current configuration fields are readable",
                ),
                Ok(_) => push(
                    items,
                    path,
                    "launcher_config",
                    CompatibilityStatus::Incompatible,
                    "configuration values are invalid; no rewrite was attempted",
                ),
                Err(_) => push(
                    items,
                    path,
                    "launcher_config",
                    CompatibilityStatus::Incompatible,
                    "invalid configuration or unsupported fields; no rewrite was attempted",
                ),
            },
            Ok(None) => push(
                items,
                path,
                "launcher_config",
                CompatibilityStatus::Incompatible,
                "configuration exceeds the metadata size limit",
            ),
        },
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountIndex {
    #[serde(default)]
    accounts: Vec<crate::accounts::AccountMetadata>,
    #[serde(default)]
    selected: Option<String>,
}

fn inspect_accounts(path: &Path, items: &mut Vec<CompatibilityItem>) {
    match read_regular(path) {
        FileState::Missing => push(
            items,
            path,
            "account_metadata",
            CompatibilityStatus::Missing,
            "no account metadata file",
        ),
        FileState::Unreadable => push(
            items,
            path,
            "account_metadata",
            CompatibilityStatus::Unreadable,
            "account metadata cannot be read safely",
        ),
        FileState::TooLarge => push(
            items,
            path,
            "account_metadata",
            CompatibilityStatus::Incompatible,
            "account metadata exceeds the size limit",
        ),
        FileState::Bytes(bytes) => match serde_json::from_slice::<AccountIndex>(&bytes) {
            Ok(index) => {
                let _ = (&index.accounts, &index.selected);
                push(
                    items,
                    path,
                    "account_metadata",
                    CompatibilityStatus::Compatible,
                    "account metadata shape is readable; OS credential storage was not accessed",
                )
            }
            Err(_) => push(
                items,
                path,
                "account_metadata",
                CompatibilityStatus::Incompatible,
                "unknown fields or unsupported account metadata format",
            ),
        },
    }
}

enum FileState {
    Missing,
    Unreadable,
    TooLarge,
    Bytes(Vec<u8>),
}
/// Read at most the metadata limit plus one byte, so a concurrent size change cannot
/// turn inspection into an unbounded allocation.
fn read_limited(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_METADATA_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        Ok(None)
    } else {
        Ok(Some(bytes))
    }
}
fn read_regular(path: &Path) -> FileState {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileState::Missing,
        Err(_) => FileState::Unreadable,
        Ok(m) if m.file_type().is_symlink() || !m.is_file() => FileState::Unreadable,
        Ok(m) if m.len() > MAX_METADATA_BYTES => FileState::TooLarge,
        Ok(_) => match read_limited(path) {
            Ok(Some(bytes)) => FileState::Bytes(bytes),
            Ok(None) => FileState::TooLarge,
            Err(_) => FileState::Unreadable,
        },
    }
}

fn inspect_instances(root: &Path, items: &mut Vec<CompatibilityItem>) {
    let ccl_root = root.parent().unwrap_or(root);
    match fs::symlink_metadata(ccl_root) {
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            push(
                items,
                ccl_root,
                "instances",
                CompatibilityStatus::Incompatible,
                "CCL metadata root is not a regular directory",
            );
            return;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            push(
                items,
                root,
                "instances",
                CompatibilityStatus::Missing,
                "no managed instances",
            );
            return;
        }
        Err(_) => {
            push(
                items,
                ccl_root,
                "instances",
                CompatibilityStatus::Unreadable,
                "CCL metadata root cannot be inspected",
            );
            return;
        }
        _ => {}
    }
    match fs::symlink_metadata(root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => push(
            items,
            root,
            "instances",
            CompatibilityStatus::Missing,
            "no managed instances",
        ),
        Err(_) => push(
            items,
            root,
            "instances",
            CompatibilityStatus::Unreadable,
            "instance directory cannot be inspected",
        ),
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => push(
            items,
            root,
            "instances",
            CompatibilityStatus::Incompatible,
            "expected a regular directory",
        ),
        Ok(_) => match fs::read_dir(root) {
            Err(_) => push(
                items,
                root,
                "instances",
                CompatibilityStatus::Unreadable,
                "instance directory cannot be read",
            ),
            Ok(entries) => {
                for (position, entry) in entries.enumerate() {
                    if position >= MAX_DIRECTORY_ENTRIES || items.len() >= MAX_REPORT_ITEMS - 1 {
                        push(
                            items,
                            root,
                            "instances",
                            CompatibilityStatus::Incompatible,
                            "instance scan limit reached; compatibility is unverified",
                        );
                        break;
                    }
                    let Ok(entry) = entry else {
                        push(
                            items,
                            root,
                            "instances",
                            CompatibilityStatus::Unreadable,
                            "instance directory entry cannot be read",
                        );
                        continue;
                    };
                    let p = entry.path();
                    let Ok(meta) = fs::symlink_metadata(&p) else {
                        push(
                            items,
                            &p,
                            "instance",
                            CompatibilityStatus::Unreadable,
                            "instance cannot be inspected",
                        );
                        continue;
                    };
                    if meta.file_type().is_symlink() {
                        push(
                            items,
                            &p,
                            "instance",
                            CompatibilityStatus::Incompatible,
                            "symlinked instance is not inspected",
                        );
                        continue;
                    }
                    if !meta.is_dir() || entry.file_name().to_string_lossy().starts_with('.') {
                        continue;
                    }
                    inspect_instance(&p, items);
                }
            }
        },
    }
}

fn inspect_instance(path: &Path, items: &mut Vec<CompatibilityItem>) {
    let metadata = path.join("instance.json");
    match read_regular(&metadata) {
        FileState::Missing => push(
            items,
            &metadata,
            "instance_metadata",
            CompatibilityStatus::Incompatible,
            "managed instance has no metadata",
        ),
        FileState::Unreadable => push(
            items,
            &metadata,
            "instance_metadata",
            CompatibilityStatus::Unreadable,
            "instance metadata cannot be read safely",
        ),
        FileState::TooLarge => push(
            items,
            &metadata,
            "instance_metadata",
            CompatibilityStatus::Incompatible,
            "instance metadata exceeds the size limit",
        ),
        FileState::Bytes(bytes) => {
            match serde_json::from_slice::<crate::instances::InstanceMetadata>(&bytes) {
                Ok(meta) if valid_instance_metadata(&meta, path) => push(
                    items,
                    &metadata,
                    "instance_metadata",
                    CompatibilityStatus::Compatible,
                    "instance metadata is readable",
                ),
                Err(_) => push(
                    items,
                    &metadata,
                    "instance_metadata",
                    CompatibilityStatus::Incompatible,
                    "invalid or unsupported instance metadata format",
                ),
                Ok(_) => push(
                    items,
                    &metadata,
                    "instance_metadata",
                    CompatibilityStatus::Incompatible,
                    "instance metadata does not match its directory or contains invalid version fields",
                ),
            }
        }
    }
    for generation in ["game", "previous"] {
        inspect_generation(&path.join(generation), items);
    }
    let history = path.join("history");
    match fs::symlink_metadata(&history) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => push(
            items,
            &history,
            "history",
            CompatibilityStatus::Unreadable,
            "history cannot be inspected",
        ),
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => push(
            items,
            &history,
            "history",
            CompatibilityStatus::Incompatible,
            "history is not a regular directory",
        ),
        Ok(_) => match fs::read_dir(&history) {
            Err(_) => push(
                items,
                &history,
                "history",
                CompatibilityStatus::Unreadable,
                "history entries cannot be read",
            ),
            Ok(entries) => {
                for (position, entry) in entries.enumerate() {
                    if position >= MAX_DIRECTORY_ENTRIES || items.len() >= MAX_REPORT_ITEMS - 1 {
                        push(
                            items,
                            &history,
                            "history",
                            CompatibilityStatus::Incompatible,
                            "history scan limit reached; compatibility is unverified",
                        );
                        break;
                    }
                    let Ok(entry) = entry else {
                        push(
                            items,
                            &history,
                            "history",
                            CompatibilityStatus::Unreadable,
                            "history entry cannot be read",
                        );
                        continue;
                    };
                    let p = entry.path();
                    match fs::symlink_metadata(&p) {
                        Ok(m) if m.file_type().is_symlink() => push(
                            items,
                            &p,
                            "history",
                            CompatibilityStatus::Incompatible,
                            "symlinked history entry is not inspected",
                        ),
                        Ok(m) if m.is_dir() => inspect_generation(&p.join("game"), items),
                        Ok(_) => push(
                            items,
                            &p,
                            "history",
                            CompatibilityStatus::Incompatible,
                            "history entry is not a directory",
                        ),
                        Err(_) => push(
                            items,
                            &p,
                            "history",
                            CompatibilityStatus::Unreadable,
                            "history entry cannot be inspected",
                        ),
                    }
                }
            }
        },
    }
}

fn inspect_generation(game: &Path, items: &mut Vec<CompatibilityItem>) {
    match fs::symlink_metadata(game) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {
            push(
                items,
                game,
                "generation",
                CompatibilityStatus::Unreadable,
                "generation cannot be inspected",
            );
            return;
        }
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            push(
                items,
                game,
                "generation",
                CompatibilityStatus::Incompatible,
                "generation is not a regular directory",
            );
            return;
        }
        _ => {}
    }
    let ccl = game.join(".ccl");
    match fs::symlink_metadata(&ccl) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {
            push(
                items,
                &ccl,
                "generation_metadata",
                CompatibilityStatus::Unreadable,
                "generation metadata cannot be inspected",
            );
            return;
        }
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            push(
                items,
                &ccl,
                "generation_metadata",
                CompatibilityStatus::Incompatible,
                "generation metadata root is not a regular directory",
            );
            return;
        }
        _ => {}
    }
    for (rel, format) in [
        (".ccl/modpack.json", "modpack_receipt"),
        (".ccl/online-content.json", "online_content_receipt"),
    ] {
        let p = game.join(rel);
        match read_regular(&p) {
            FileState::Missing => (),
            FileState::Unreadable => push(
                items,
                &p,
                format,
                CompatibilityStatus::Unreadable,
                "receipt cannot be read safely",
            ),
            FileState::TooLarge => push(
                items,
                &p,
                format,
                CompatibilityStatus::Incompatible,
                "receipt exceeds the metadata size limit",
            ),
            FileState::Bytes(bytes) => match validate_receipt(format, &bytes) {
                Ok(()) => push(
                    items,
                    &p,
                    format,
                    CompatibilityStatus::Compatible,
                    "schema 1 receipt is supported",
                ),
                Err(ReceiptError::FutureSchema) => push(
                    items,
                    &p,
                    format,
                    CompatibilityStatus::Incompatible,
                    "future receipt schema; rollback is unsafe",
                ),
                Err(_) => push(
                    items,
                    &p,
                    format,
                    CompatibilityStatus::Incompatible,
                    "malformed or unsupported receipt structure",
                ),
            },
        }
    }
}

fn valid_instance_metadata(meta: &crate::instances::InstanceMetadata, path: &Path) -> bool {
    let expected_id = path.file_name().and_then(|name| name.to_str());
    let id_valid = crate::instances::is_valid_id(&meta.id);
    let segment_valid = |value: &str| {
        !value.is_empty()
            && value != "."
            && value != ".."
            && !value.contains(['/', '\\', ':'])
            && !value.chars().any(char::is_control)
    };
    let expected_game = path.join("game");
    id_valid
        && expected_id == Some(meta.id.as_str())
        && segment_valid(&meta.minecraft)
        && meta.loader.is_some() == meta.loader_version.is_some()
        && meta.loader_version.as_deref().is_none_or(segment_valid)
        && meta.loader_version.as_deref() != Some(meta.minecraft.as_str())
        && (meta.game_directory.as_os_str().is_empty() || meta.game_directory == expected_game)
}

#[derive(Deserialize)]
struct OnlineReceiptShape {
    schema: u32,
    versions: Vec<crate::online_content::OnlineInstalledVersion>,
    owned_files: BTreeMap<String, String>,
}

enum ReceiptError {
    FutureSchema,
    Invalid,
}

fn validate_receipt(format: &str, bytes: &[u8]) -> Result<(), ReceiptError> {
    if format == "modpack_receipt" {
        let receipt: crate::modpacks::PackReceipt =
            serde_json::from_slice(bytes).map_err(|_| ReceiptError::Invalid)?;
        if receipt.schema > MAX_SUPPORTED_RECEIPT_SCHEMA {
            return Err(ReceiptError::FutureSchema);
        }
        if receipt.schema != MAX_SUPPORTED_RECEIPT_SCHEMA || receipt.manifest.validate().is_err() {
            return Err(ReceiptError::Invalid);
        }
        for (path, digest) in &receipt.owned_files {
            if crate::modpack_format::validate_content_path(path).is_err()
                || digest.len() != 128
                || !digest.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(ReceiptError::Invalid);
            }
        }
        Ok(())
    } else {
        let receipt: OnlineReceiptShape =
            serde_json::from_slice(bytes).map_err(|_| ReceiptError::Invalid)?;
        if receipt.schema > MAX_SUPPORTED_RECEIPT_SCHEMA {
            return Err(ReceiptError::FutureSchema);
        }
        if receipt.schema != MAX_SUPPORTED_RECEIPT_SCHEMA {
            return Err(ReceiptError::Invalid);
        }
        let _ = receipt.versions;
        for (name, digest) in receipt.owned_files {
            if !name.to_ascii_lowercase().ends_with(".jar")
                || name.contains(['/', '\\'])
                || crate::modpack_format::validate_content_path(&format!("mods/{name}")).is_err()
                || digest.len() != 128
                || !digest.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(ReceiptError::Invalid);
            }
        }
        Ok(())
    }
}
