//! Instance relocation, independent copies, retained revisions and bounded cleanup.
use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Revision {
    pub revision: String,
    pub metadata: InstanceMetadata,
    /// Logical file lengths, not exclusive allocated disk blocks (assets may be hard-linked).
    pub bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskUsage {
    pub id: String,
    pub active_bytes: u64,
    pub previous_bytes: u64,
    pub history_bytes: u64,
    pub stage_bytes: u64,
    pub logical_bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupTarget {
    pub revision: String,
    pub bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupPlan {
    pub id: String,
    pub targets: Vec<CleanupTarget>,
    pub logical_bytes: u64,
    pub applied: bool,
}

fn unique() -> String {
    format!(
        "{:020}-{:010}-{:010}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    )
}

impl InstanceService {
    fn lock_pair(&self, left: &str, right: &str) -> InstanceResult<(InstanceLock, InstanceLock)> {
        if left.to_ascii_lowercase() < right.to_ascii_lowercase() {
            Ok((self.lock_instance(left)?, self.lock_instance(right)?))
        } else {
            let right_lock = self.lock_instance(right)?;
            let left_lock = self.lock_instance(left)?;
            Ok((left_lock, right_lock))
        }
    }

    /// Stable locks survive directory renames and deletion. Lock files must not be removed.
    pub(super) fn lock_instance(&self, id: &str) -> InstanceResult<InstanceLock> {
        validate_id(id)?;
        let root = self.root.join(".ccl");
        reject_symlink(&root)?;
        let locks = root.join("instance-locks");
        reject_symlink(&locks)?;
        fs::create_dir_all(&locks).map_err(|e| io(&locks, e))?;
        let path = locks.join(format!("{}.lock", id.to_ascii_lowercase()));
        reject_symlink(&path)?;
        let lock = InstanceLock::acquire(&path, id)?;
        if let Ok(entries) = fs::read_dir(self.instances_dir()) {
            for entry in entries {
                let entry = entry.map_err(|e| io(self.instances_dir(), e))?;
                let existing = entry.file_name().to_string_lossy().into_owned();
                if existing.eq_ignore_ascii_case(id) && existing != id {
                    return Err(InstanceError::Conflict(format!(
                        "instance {existing} already exists with different casing"
                    )));
                }
            }
        }
        Ok(lock)
    }

    /// Rename the instance ID. Both IDs are locked; recovery completes metadata rebasing.
    pub fn rename(&self, id: &str, destination: &str) -> InstanceResult<InstanceMetadata> {
        let source = self.instance_path(id)?;
        let target = self.instance_path(destination)?;
        if id.eq_ignore_ascii_case(destination) {
            return Err(InstanceError::Conflict(
                "source and destination IDs must differ, including case".into(),
            ));
        }
        let (_source_lock, _target_lock) = self.lock_pair(id, destination)?;
        recover_instance(&source)?;
        read_metadata(&source.join("instance.json"))?;
        ensure_destination(&self.instances_dir(), destination)?;
        // Record destination before the atomic move. Recovery on either side uses actual path.
        atomic_write(
            &source.join(".relocate.json"),
            &serde_json::to_vec(&destination).unwrap(),
        )?;
        if let Err(e) = fs::rename(&source, &target) {
            let _ = fs::remove_file(source.join(".relocate.json"));
            return Err(io(&target, e));
        }
        recover_relocation(&target)?;
        read_metadata(&target.join("instance.json"))
    }

    /// Copy all active data and retained revisions; mutable files are independent copies.
    pub fn duplicate(&self, id: &str, destination: &str) -> InstanceResult<InstanceMetadata> {
        let source = self.instance_path(id)?;
        let target = self.instance_path(destination)?;
        if id.eq_ignore_ascii_case(destination) {
            return Err(InstanceError::Conflict(
                "source and destination IDs must differ".into(),
            ));
        }
        let (_source_lock, _target_lock) = self.lock_pair(id, destination)?;
        recover_instance(&source)?;
        read_metadata(&source.join("instance.json"))?;
        ensure_destination(&self.instances_dir(), destination)?;
        let temporary = self.instances_dir().join(format!(".copy-{}", unique()));
        fs::create_dir(&temporary).map_err(|e| io(&temporary, e))?;
        let result = (|| {
            for name in ["game", "previous"] {
                let from = source.join(name);
                if from.exists() {
                    clone_game_generation(&from, &temporary.join(name))?;
                }
            }
            for name in ["instance.json", "previous.json"] {
                let from = source.join(name);
                if from.is_file() {
                    fs::copy(&from, temporary.join(name)).map_err(|e| io(&from, e))?;
                }
            }
            let history = source.join("history");
            if history.exists() {
                reject_symlink(&history)?;
                for entry in fs::read_dir(&history).map_err(|e| io(&history, e))? {
                    let entry = entry.map_err(|e| io(&history, e))?;
                    reject_symlink(&entry.path())?;
                    let to = temporary.join("history").join(entry.file_name());
                    fs::create_dir_all(&to).map_err(|e| io(&to, e))?;
                    if entry.path().join("game").is_dir() {
                        clone_game_generation(&entry.path().join("game"), &to.join("game"))?;
                        fs::copy(entry.path().join("metadata.json"), to.join("metadata.json"))
                            .map_err(|e| io(&to, e))?;
                    }
                }
            }
            atomic_write(
                &temporary.join(".relocate.json"),
                &serde_json::to_vec(destination).unwrap(),
            )?;
            // Metadata must refer to the final path, so rebase after the atomic directory move.
            fs::rename(&temporary, &target).map_err(|e| io(&target, e))?;
            recover_relocation(&target)?;
            read_metadata(&target.join("instance.json"))
        })();
        if temporary.exists() {
            let _ = fs::remove_dir_all(&temporary);
        }
        result
    }

    /// Delete only this instance, including its saves and revisions. Caller must request it explicitly.
    pub fn delete(&self, id: &str) -> InstanceResult<()> {
        let source = self.instance_path(id)?;
        let _lock = self.lock_instance(id)?;
        recover_instance(&source)?;
        read_metadata(&source.join("instance.json"))?;
        let trash = self
            .instances_dir()
            .join(format!(".deleted-{}-{id}-{}", id.len(), unique()));
        fs::rename(&source, &trash).map_err(|e| io(&source, e))?;
        fs::remove_dir_all(&trash).map_err(|e| io(&trash, e))
    }

    pub fn history(&self, id: &str) -> InstanceResult<Vec<Revision>> {
        let path = self.instance_path(id)?;
        let _lock = self.lock_instance(id)?;
        recover_instance(&path)?;
        let mut entries = vec![Revision {
            revision: "current".into(),
            metadata: read_metadata(&path.join("instance.json"))?,
            bytes: tree_bytes(&path.join("game"))?,
        }];
        if path.join("previous").is_dir() {
            entries.push(Revision {
                revision: "previous".into(),
                metadata: read_metadata(&path.join("previous.json"))?,
                bytes: tree_bytes(&path.join("previous"))?,
            });
        }
        for revision in history_ids(&path)? {
            let archive = path.join("history").join(&revision);
            if archive.join("game").is_dir() {
                entries.push(Revision {
                    revision,
                    metadata: archive_metadata(
                        &archive.join("metadata.json"),
                        id,
                        &path.join("game"),
                    )?,
                    bytes: tree_bytes(&archive.join("game"))?,
                });
            }
        }
        Ok(entries)
    }

    /// Restore a retained snapshot as a new revision, retaining current data for rollback.
    pub fn restore_revision(&self, id: &str, revision: &str) -> InstanceResult<InstanceMetadata> {
        validate_revision(revision)?;
        let path = self.instance_path(id)?;
        let _lock = self.lock_instance(id)?;
        recover_instance(&path)?;
        read_metadata(&path.join("instance.json"))?;
        let (source, metadata) = if revision == "previous" {
            (
                path.join("previous"),
                read_metadata(&path.join("previous.json"))?,
            )
        } else {
            let archive = path.join("history").join(revision);
            reject_symlink(&path.join("history"))?;
            reject_symlink(&archive)?;
            (
                archive.join("game"),
                archive_metadata(&archive.join("metadata.json"), id, &path.join("game"))?,
            )
        };
        if !source.is_dir() {
            return Err(InstanceError::NotFound(revision.into()));
        }
        let stage = path.join(".stage");
        reject_symlink(&stage)?;
        if stage.exists() {
            fs::remove_dir_all(&stage).map_err(|e| io(&stage, e))?;
        }
        clone_game_generation(&source, &stage)?;
        mark_content_revision(&stage)?;
        commit_generation(
            &path,
            &stage,
            &path.join("game"),
            &path.join("previous"),
            &metadata,
        )?;
        Ok(metadata)
    }

    pub fn disk_usage(&self, id: &str) -> InstanceResult<DiskUsage> {
        let path = self.instance_path(id)?;
        let _lock = self.lock_instance(id)?;
        recover_instance(&path)?;
        read_metadata(&path.join("instance.json"))?;
        let active_bytes = tree_bytes(&path.join("game"))?;
        let previous_bytes = tree_bytes(&path.join("previous"))?;
        let history_bytes = tree_bytes(&path.join("history"))?;
        let stage_bytes = tree_bytes(&path.join(".stage"))?;
        Ok(DiskUsage {
            id: id.into(),
            active_bytes,
            previous_bytes,
            history_bytes,
            stage_bytes,
            logical_bytes: active_bytes + previous_bytes + history_bytes + stage_bytes,
        })
    }

    /// Preview by default. Keep the newest N archived snapshots; active data is never targeted.
    pub fn cleanup(
        &self,
        id: &str,
        keep: usize,
        include_previous: bool,
        apply: bool,
    ) -> InstanceResult<CleanupPlan> {
        let path = self.instance_path(id)?;
        let _lock = self.lock_instance(id)?;
        recover_instance(&path)?;
        read_metadata(&path.join("instance.json"))?;
        let mut targets = Vec::new();
        for revision in history_ids(&path)?.into_iter().skip(keep) {
            let archive = path.join("history").join(&revision);
            targets.push(CleanupTarget {
                revision,
                bytes: tree_bytes(&archive)?,
            });
        }
        for name in [".stage", "previous"] {
            if name == "previous" && !include_previous {
                continue;
            }
            let target = path.join(name);
            reject_symlink(&target)?;
            if target.is_dir() {
                targets.push(CleanupTarget {
                    revision: name.into(),
                    bytes: tree_bytes(&target)?,
                });
            }
        }
        let logical_bytes = targets.iter().map(|t| t.bytes).sum();
        if apply {
            for target in &targets {
                let entry = match target.revision.as_str() {
                    ".stage" | "previous" => path.join(&target.revision),
                    _ => path.join("history").join(&target.revision),
                };
                fs::remove_dir_all(&entry).map_err(|e| io(&entry, e))?;
                if target.revision == "previous" {
                    let meta = path.join("previous.json");
                    if meta.exists() {
                        fs::remove_file(&meta).map_err(|e| io(&meta, e))?;
                    }
                }
            }
        }
        Ok(CleanupPlan {
            id: id.into(),
            targets,
            logical_bytes,
            applied: apply,
        })
    }
}

pub(super) fn recover_deleted_instances(service: &InstanceService) -> InstanceResult<()> {
    let root = service.instances_dir();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io(&root, error)),
    };
    for entry in entries {
        let entry = entry.map_err(|e| io(&root, e))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(id) = deleted_instance_id(&name) else {
            continue;
        };
        reject_symlink(&entry.path())?;
        let lock = match service.lock_instance(id) {
            Ok(lock) => lock,
            Err(InstanceError::Conflict(_)) => continue,
            Err(error) => return Err(error),
        };
        reject_symlink(&entry.path())?;
        match fs::symlink_metadata(entry.path()) {
            Ok(metadata) if metadata.is_dir() => {
                fs::remove_dir_all(entry.path()).map_err(|e| io(entry.path(), e))?;
            }
            Ok(_) => {
                return Err(InstanceError::Conflict(format!(
                    "invalid deleted instance tombstone: {}",
                    entry.path().display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(entry.path(), error)),
        }
        drop(lock);
    }
    Ok(())
}

fn deleted_instance_id(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(".deleted-")?;
    let (length, rest) = rest.split_once('-')?;
    let length = length.parse::<usize>().ok()?;
    if length == 0 || length > 64 || rest.len() <= length || !rest.is_char_boundary(length) {
        return None;
    }
    let (id, suffix) = rest.split_at(length);
    if !suffix.starts_with('-') || validate_id(id).is_err() {
        return None;
    }
    Some(id)
}

fn ensure_destination(parent: &Path, id: &str) -> InstanceResult<()> {
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries {
            let entry = entry.map_err(|e| io(parent, e))?;
            if entry.file_name().to_string_lossy().eq_ignore_ascii_case(id) {
                return Err(InstanceError::Conflict(format!(
                    "destination instance already exists: {id}"
                )));
            }
        }
    }
    Ok(())
}
fn validate_revision(value: &str) -> InstanceResult<()> {
    if value == "previous"
        || (value.starts_with("generation-")
            && value.len() < 110
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() || b == b'-'))
    {
        Ok(())
    } else {
        Err(InstanceError::InvalidId(format!(
            "invalid revision: {value}"
        )))
    }
}
fn history_ids(instance: &Path) -> InstanceResult<Vec<String>> {
    let root = instance.join("history");
    reject_symlink(&root)?;
    let mut ids = Vec::new();
    if !root.exists() {
        return Ok(ids);
    }
    for entry in fs::read_dir(&root).map_err(|e| io(&root, e))? {
        let entry = entry.map_err(|e| io(&root, e))?;
        reject_symlink(&entry.path())?;
        let id = entry.file_name().to_string_lossy().into_owned();
        validate_revision(&id)?;
        ids.push(id);
    }
    ids.sort_by(|a, b| b.cmp(a));
    Ok(ids)
}
fn archive_metadata(path: &Path, id: &str, game: &Path) -> InstanceResult<InstanceMetadata> {
    reject_symlink(path)?;
    let mut metadata: InstanceMetadata =
        serde_json::from_slice(&fs::read(path).map_err(|e| io(path, e))?).map_err(|source| {
            InstanceError::Metadata {
                path: path.into(),
                source,
            }
        })?;
    validate_id(&metadata.id)?;
    validate_segment(&metadata.minecraft)?;
    if let Some(v) = &metadata.loader_version {
        validate_segment(v)?;
    }
    if metadata.loader.is_some() != metadata.loader_version.is_some() {
        return Err(InstanceError::InvalidId(
            "invalid archived loader metadata".into(),
        ));
    }
    metadata.id = id.into();
    metadata.game_directory = game.into();
    Ok(metadata)
}
fn tree_bytes(root: &Path) -> InstanceResult<u64> {
    let meta = match fs::symlink_metadata(root) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(io(root, e)),
    };
    if meta.is_symlink() {
        return Ok(meta.len());
    } // Never traverse symbolic links during sizing or cleanup.
    if meta.is_file() {
        return Ok(meta.len());
    }
    let mut bytes = 0u64;
    for entry in fs::read_dir(root).map_err(|e| io(root, e))? {
        let entry = entry.map_err(|e| io(root, e))?;
        bytes = bytes.saturating_add(tree_bytes(&entry.path())?);
    }
    Ok(bytes)
}

pub(super) fn archive_previous(instance: &Path) -> InstanceResult<()> {
    let saved = instance.join(".saved-previous");
    let metadata = instance.join(".saved-previous.json");
    reject_symlink(&saved)?;
    reject_symlink(&metadata)?;
    if saved.is_dir() {
        let history = instance.join("history");
        reject_symlink(&history)?;
        let target = history.join(format!("generation-{}", unique()));
        fs::create_dir_all(&target).map_err(|e| io(&target, e))?;
        if !metadata.is_file() {
            return Err(InstanceError::Conflict(
                "previous generation metadata is missing".into(),
            ));
        }
        fs::copy(&metadata, target.join("metadata.json")).map_err(|e| io(&target, e))?;
        fs::rename(&saved, target.join("game")).map_err(|e| io(&target, e))?;
    }
    if metadata.exists() {
        fs::remove_file(&metadata).map_err(|e| io(&metadata, e))?;
    }
    Ok(())
}

pub(super) fn recover_relocation(instance: &Path) -> InstanceResult<()> {
    let journal = instance.join(".relocate.json");
    reject_symlink(&journal)?;
    if !journal.is_file() {
        return Ok(());
    }
    let target: String = serde_json::from_slice(&fs::read(&journal).map_err(|e| io(&journal, e))?)
        .map_err(|source| InstanceError::Metadata {
            path: journal.clone(),
            source,
        })?;
    let id = instance.file_name().unwrap().to_string_lossy();
    validate_id(&id)?;
    validate_id(&target)?;
    if target != id {
        // The record is still at the old path, so the atomic directory move did not commit.
        fs::remove_file(&journal).map_err(|e| io(&journal, e))?;
        return Ok(());
    }
    for name in ["instance.json", "previous.json"] {
        let path = instance.join(name);
        if path.is_file() {
            let metadata = archive_metadata(&path, &id, &instance.join("game"))?;
            atomic_write(&path, &serde_json::to_vec_pretty(&metadata).unwrap())?;
        }
    }
    fs::remove_file(&journal).map_err(|e| io(&journal, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: PathBuf,
        service: InstanceService,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("ccl-lifecycle-{}", unique()));
            let service = InstanceService::new(root.clone(), DownloadConfig::default()).unwrap();
            let root = service.root.clone();
            let game = root.join(".ccl/instances/original/game");
            fs::create_dir_all(game.join("saves/world")).unwrap();
            fs::write(game.join("saves/world/level.dat"), b"world").unwrap();
            let metadata = InstanceMetadata {
                id: "original".into(),
                minecraft: "1.21.1".into(),
                loader: None,
                loader_version: None,
                game_directory: game.clone(),
            };
            atomic_write(
                &game.parent().unwrap().join("instance.json"),
                &serde_json::to_vec(&metadata).unwrap(),
            )
            .unwrap();
            Self { root, service }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn copy_and_rename_are_isolated_and_busy_operations_refuse() {
        let f = Fixture::new();
        let guard = f.service.acquire_launch("original").unwrap();
        assert!(matches!(
            f.service.rename("original", "renamed"),
            Err(InstanceError::Conflict(_))
        ));
        assert!(matches!(
            f.service.duplicate("original", "copy"),
            Err(InstanceError::Conflict(_))
        ));
        assert!(matches!(
            f.service.delete("original"),
            Err(InstanceError::Conflict(_))
        ));
        drop(guard);
        let copied = f.service.duplicate("original", "copy").unwrap();
        fs::write(
            copied.game_directory.join("saves/world/level.dat"),
            b"changed",
        )
        .unwrap();
        let original = f.service.game_directory("original").unwrap();
        assert_eq!(
            fs::read(original.join("saves/world/level.dat")).unwrap(),
            b"world"
        );
        let renamed = f.service.rename("original", "renamed").unwrap();
        assert_eq!(renamed.id, "renamed");
        assert!(!original.exists());
        assert_eq!(
            fs::read(renamed.game_directory.join("saves/world/level.dat")).unwrap(),
            b"world"
        );
        assert_eq!(f.service.list().unwrap().len(), 2);
        assert!(f.service.rename("renamed", "COPY").is_err());
        f.service.delete("renamed").unwrap();
        assert_eq!(f.service.list().unwrap().len(), 1);
        assert_eq!(
            fs::read(copied.game_directory.join("saves/world/level.dat")).unwrap(),
            b"changed"
        );
    }

    #[test]
    fn stable_external_locks_and_ordered_pairs_prevent_conflicts_and_deadlocks() {
        let f = Fixture::new();
        let other_service =
            InstanceService::new(f.root.clone(), DownloadConfig::default()).unwrap();
        let casefold_lock = f.service.lock_instance("MixedCase").unwrap();
        assert!(matches!(
            other_service.lock_instance("mixedcase"),
            Err(InstanceError::Conflict(_))
        ));
        drop(casefold_lock);
        drop(other_service.lock_instance("mixedcase").unwrap());

        let launch = f.service.acquire_launch("original").unwrap();
        assert!(matches!(
            other_service.acquire_launch("original"),
            Err(InstanceError::Conflict(_))
        ));
        drop(launch);

        let other_path = f.service.instance_path("second").unwrap();
        fs::create_dir_all(other_path.join("game")).unwrap();
        atomic_write(
            &other_path.join("instance.json"),
            &serde_json::to_vec(&InstanceMetadata {
                id: "second".into(),
                minecraft: "1.21.1".into(),
                loader: None,
                loader_version: None,
                game_directory: other_path.join("game"),
            })
            .unwrap(),
        )
        .unwrap();

        let left_service = f.service.clone();
        let right_service = other_service;
        let (tx, rx) = std::sync::mpsc::channel();
        let left_tx = tx.clone();
        let left = std::thread::spawn(move || {
            left_tx
                .send(left_service.rename("original", "second"))
                .unwrap();
        });
        let right = std::thread::spawn(move || {
            tx.send(right_service.rename("second", "original")).unwrap();
        });
        let first = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        let second = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        left.join().unwrap();
        right.join().unwrap();
        assert!(matches!(first, Err(InstanceError::Conflict(_))));
        assert!(matches!(second, Err(InstanceError::Conflict(_))));
    }

    #[test]
    fn interrupted_delete_tombstone_waits_for_lock_then_cleans_up() {
        let f = Fixture::new();
        let guard = f.service.acquire_launch("original").unwrap();
        let source = f.service.instance_path("original").unwrap();
        let tombstone = f
            .service
            .instances_dir()
            .join(format!(".deleted-8-original-{}", unique()));
        fs::rename(&source, &tombstone).unwrap();
        assert!(f.service.list().unwrap().is_empty());
        assert!(
            tombstone.exists(),
            "live lock must protect in-progress deletion"
        );
        drop(guard);
        assert!(f.service.list().unwrap().is_empty());
        assert!(
            !tombstone.exists(),
            "orphaned delete tombstone should be reclaimed"
        );
    }
    #[test]
    fn history_survives_edits_restore_and_relocation_and_cleanup_is_preview() {
        let f = Fixture::new();
        for n in 0..4 {
            f.service
                .edit_content::<_, InstanceError, _>("original", |game| {
                    fs::write(game.join("marker"), n.to_string()).map_err(|e| io(game, e))?;
                    Ok(())
                })
                .unwrap();
        }
        let history = f.service.history("original").unwrap();
        assert_eq!(history.len(), 5);
        let revision = history.last().unwrap().revision.clone();
        let clone = f.service.duplicate("original", "copy").unwrap();
        assert_eq!(f.service.history("copy").unwrap().len(), 5);
        assert_eq!(f.service.history("copy").unwrap()[0].metadata.id, "copy");
        f.service.rename("original", "renamed").unwrap();
        f.service.restore_revision("renamed", &revision).unwrap();
        let active = f.service.game_directory("renamed").unwrap();
        assert!(!active.join("marker").exists());
        assert_eq!(
            fs::read_to_string(clone.game_directory.join("marker")).unwrap(),
            "3"
        );
        let before = f.service.disk_usage("renamed").unwrap();
        let preview = f.service.cleanup("renamed", 0, true, false).unwrap();
        assert!(!preview.applied);
        assert!(!preview.targets.is_empty());
        assert_eq!(
            f.service.disk_usage("renamed").unwrap().logical_bytes,
            before.logical_bytes
        );
        let applied = f.service.cleanup("renamed", 0, true, true).unwrap();
        assert!(applied.applied);
        assert_eq!(f.service.history("renamed").unwrap().len(), 1);
        assert!(active.join("saves/world/level.dat").is_file());
        assert!(f.service.disk_usage("renamed").unwrap().logical_bytes < before.logical_bytes);
    }
    #[test]
    fn relocation_recovery_repairs_paths_after_directory_move() {
        let f = Fixture::new();
        let from = f.service.instance_path("original").unwrap();
        let to = f.service.instance_path("recovered").unwrap();
        atomic_write(&from.join(".relocate.json"), br#""recovered""#).unwrap();
        fs::rename(&from, &to).unwrap();
        let metadata = f.service.metadata("recovered").unwrap();
        assert_eq!(metadata.id, "recovered");
        assert_eq!(metadata.game_directory, to.join("game"));
        assert!(!to.join(".relocate.json").exists());
    }
    #[test]
    fn invalid_names_and_revisions_never_escape_storage() {
        let f = Fixture::new();
        for name in ["../outside", "/tmp/outside", "a/b", ""] {
            assert!(f.service.rename("original", name).is_err());
            assert!(f.service.duplicate("original", name).is_err());
        }
        for revision in ["../game", "current", "generation-../../outside"] {
            assert!(f.service.restore_revision("original", revision).is_err());
        }
        assert!(f.service.rename("original", "ORIGINAL").is_err());
        assert!(f.service.game_directory("original").unwrap().is_dir());
    }
    #[cfg(unix)]
    #[test]
    fn unsafe_copy_symlink_fails_and_cleanup_never_follows_external_targets() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let outside = f.root.join("outside");
        fs::write(&outside, b"keep").unwrap();
        let game = f.service.game_directory("original").unwrap();
        symlink(&outside, game.join("escape")).unwrap();
        assert!(f.service.duplicate("original", "copy").is_err());
        assert!(!f.service.instance_path("copy").unwrap().exists());
        f.service.delete("original").unwrap();
        assert_eq!(fs::read(outside).unwrap(), b"keep");
    }
}
