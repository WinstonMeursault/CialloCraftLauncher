//! Extraction of legacy classifier native archives.

use crate::{
    launch_plan::LaunchPlatform,
    metadata::{RuleContext, VersionMetadata},
};
use sha1::{Digest, Sha1};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use thiserror::Error;
use zip::ZipArchive;

const MAX_UNCOMPRESSED: u64 = 512 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum NativeError {
    #[error("native archive I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid native archive: {0}")]
    Archive(String),
    #[error("native archive checksum or size mismatch: {0}")]
    Integrity(String),
    #[error("selected native classifier is missing its {0}")]
    MissingMetadata(&'static str),
    #[error("unsafe native archive path: {0}")]
    UnsafePath(String),
    #[error("native output already exists: {0}")]
    DestinationExists(PathBuf),
}

/// Extract selected legacy native classifier archives into a fresh directory.
/// Newer native artifacts are intended to remain on the classpath and self-extract.
pub fn extract_natives(
    metadata: &VersionMetadata,
    game_directory: &Path,
    destination: &Path,
    platform: &LaunchPlatform,
) -> Result<usize, NativeError> {
    if destination.exists() {
        return Err(NativeError::DestinationExists(destination.to_owned()));
    }
    let features = HashMap::new();
    let context = RuleContext {
        os: &platform.os,
        os_version: &platform.os_version,
        arch: &platform.arch,
        features: &features,
    };
    let mut archives = Vec::new();
    for library in &metadata.libraries {
        let Some((_, download)) = metadata.native_classifier_for(library, &context) else {
            continue;
        };
        let relative = download
            .path
            .as_deref()
            .ok_or(NativeError::MissingMetadata("archive path"))?;
        if download.sha1.is_none() {
            return Err(NativeError::MissingMetadata("SHA1 checksum"));
        }
        let safe = safe_relative(relative)?;
        archives.push((
            game_directory.join("libraries").join(safe),
            download,
            library.extract.as_ref(),
        ));
    }

    fs::create_dir(destination)?;
    let mut written: HashSet<PathBuf> = HashSet::new();
    let result = (|| {
        for (archive_path, download, extract) in archives {
            let mut file = File::open(&archive_path)?;
            let mut hasher = Sha1::new();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            hasher.update(&bytes);
            if let Some(expected) = download.sha1.as_deref() {
                let actual = format!("{:x}", hasher.finalize());
                if !actual.eq_ignore_ascii_case(expected) {
                    return Err(NativeError::Integrity(archive_path.display().to_string()));
                }
            }
            if download
                .size
                .is_some_and(|expected| expected != bytes.len() as u64)
            {
                return Err(NativeError::Integrity(archive_path.display().to_string()));
            }
            let mut zip = ZipArchive::new(std::io::Cursor::new(bytes))
                .map_err(|e| NativeError::Archive(e.to_string()))?;
            let mut entries = Vec::new();
            let mut total = 0u64;
            for i in 0..zip.len() {
                let entry = zip
                    .by_index(i)
                    .map_err(|e| NativeError::Archive(e.to_string()))?;
                let name = entry
                    .name()
                    .map_err(|e| NativeError::Archive(e.to_string()))?
                    .into_owned();
                let mode = entry.unix_mode().unwrap_or(0);
                if mode & 0o170000 == 0o120000 {
                    return Err(NativeError::UnsafePath(name));
                }
                let rel = safe_relative(&name)?;
                if rel
                    .components()
                    .any(|c| matches!(c, Component::Normal(s) if s.to_string_lossy() == "META-INF"))
                {
                    continue;
                }
                let excluded = extract.is_some_and(|rules| {
                    rules.exclude.iter().any(|prefix| name.starts_with(prefix))
                });
                if excluded {
                    continue;
                }
                if entry.is_dir() {
                    continue;
                }
                total = total
                    .checked_add(entry.size())
                    .ok_or_else(|| NativeError::Archive("archive size overflow".into()))?;
                if total > MAX_UNCOMPRESSED {
                    return Err(NativeError::Archive(
                        "uncompressed size exceeds limit".into(),
                    ));
                }
                if !written.insert(rel.clone()) {
                    return Err(NativeError::Archive(format!(
                        "duplicate native output: {}",
                        rel.display()
                    )));
                }
                entries.push((i, rel));
            }
            for (i, rel) in entries {
                let target = destination.join(rel);
                if !target.starts_with(destination) {
                    return Err(NativeError::UnsafePath(target.display().to_string()));
                }
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut entry = zip
                    .by_index(i)
                    .map_err(|e| NativeError::Archive(e.to_string()))?;
                let mut out = File::create(&target)?;
                std::io::copy(&mut entry, &mut out)?;
                out.flush()?;
            }
        }
        Ok(written.len())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    result
}

fn safe_relative(value: &str) -> Result<PathBuf, NativeError> {
    if value.is_empty() || value.contains('\\') || value.starts_with('/') || value.contains(':') {
        return Err(NativeError::UnsafePath(value.to_owned()));
    }
    let path = Path::new(value);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(NativeError::UnsafePath(value.to_owned()));
    }
    Ok(path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use zip::write::SimpleFileOptions;

    fn temp() -> PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("ccl-native-{n}"))
    }
    fn archive(path: &Path, entries: &[(&str, &[u8])]) -> (String, u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let f = File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        for (name, data) in entries {
            w.start_file(*name, SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
        let bytes = fs::read(path).unwrap();
        (format!("{:x}", Sha1::digest(&bytes)), bytes.len() as u64)
    }
    fn metadata(path: &str, sha1: String, size: u64, exclude: Vec<String>) -> VersionMetadata {
        serde_json::from_value(serde_json::json!({"libraries":[{
            "name":"example:native:1", "natives":{"linux":"natives-linux"},
            "downloads":{"classifiers":{"natives-linux":{"path":path,"sha1":sha1,"size":size}}},
            "extract":{"exclude":exclude}
        }]}))
        .unwrap()
    }
    fn linux() -> LaunchPlatform {
        LaunchPlatform {
            os: "linux".into(),
            arch: "x86_64".into(),
            os_version: String::new(),
        }
    }

    #[test]
    fn extracts_selected_archive_and_honors_exclusions() {
        let root = temp();
        let (sha, size) = archive(
            &root.join("game/libraries/native.jar"),
            &[
                ("lib/native.so", b"native"),
                ("META-INF/NOTICE", b"ignored"),
                ("excluded/data", b"ignored"),
            ],
        );
        let md = metadata("native.jar", sha, size, vec!["excluded/".into()]);
        let dest = root.join("natives");
        assert_eq!(
            extract_natives(&md, &root.join("game"), &dest, &linux()).unwrap(),
            1
        );
        assert_eq!(fs::read(dest.join("lib/native.so")).unwrap(), b"native");
        assert!(!dest.join("META-INF").exists());
        assert!(!dest.join("excluded").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_traversal_and_does_not_extract_wrong_platform() {
        let root = temp();
        let (sha, size) = archive(
            &root.join("game/libraries/native.jar"),
            &[("../escape", b"bad")],
        );
        let md = metadata("native.jar", sha, size, vec![]);
        let dest = root.join("natives");
        assert!(matches!(
            extract_natives(&md, &root.join("game"), &dest, &linux()),
            Err(NativeError::UnsafePath(_))
        ));
        assert!(!root.join("escape").exists());
        let other = LaunchPlatform {
            os: "windows".into(),
            ..linux()
        };
        let dest2 = root.join("natives2");
        assert_eq!(
            extract_natives(&md, &root.join("game"), &dest2, &other).unwrap(),
            0
        );
        let _ = fs::remove_dir_all(root);
    }
}
