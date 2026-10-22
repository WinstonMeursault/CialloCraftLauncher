//! Reader and writer for Modrinth `.mrpack` archives.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

const MAX_ENTRIES: usize = 100_000;
const MAX_FILE_SIZE: u64 = 128 * 1024 * 1024;
const MAX_TOTAL_SIZE: u64 = 512 * 1024 * 1024;
const MANIFEST_NAME: &str = "modrinth.index.json";
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum FormatError {
    #[error("invalid Modrinth pack: {0}")]
    Invalid(String),
    #[error("unsupported Modrinth pack: {0}")]
    Unsupported(String),
    #[error("unsafe pack path `{0}`")]
    UnsafePath(String),
    #[error("invalid download URL `{url}`: {reason}")]
    InvalidUrl { url: String, reason: String },
    #[error("archive entry `{0}` exceeds the size limit")]
    TooLarge(String),
    #[error("archive contains duplicate or case-colliding path `{0}`")]
    Collision(String),
    #[error("output already exists: {0}")]
    Exists(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("manifest JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format_version: u32,
    pub game: String,
    pub version_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub files: Vec<PackFile>,
    pub dependencies: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackFile {
    pub path: String,
    pub hashes: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Environment>,
    pub downloads: Vec<String>,
    pub file_size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub client: EnvironmentValue,
    pub server: EnvironmentValue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentValue {
    Required,
    Optional,
    Unsupported,
}

#[derive(Clone, Debug)]
pub struct PackArchive {
    pub manifest: Manifest,
    /// Files from `overrides/` with `client-overrides/` already layered on top.
    pub overrides: BTreeMap<String, Vec<u8>>,
}

impl Manifest {
    pub fn validate(&self) -> Result<(), FormatError> {
        if self.format_version != 1 {
            return Err(FormatError::Unsupported(format!(
                "formatVersion {} (only version 1 is supported)",
                self.format_version
            )));
        }
        if self.game != "minecraft" {
            return Err(FormatError::Unsupported(format!("game `{}`", self.game)));
        }
        if self.version_id.trim().is_empty() {
            return Err(FormatError::Invalid("versionId must not be blank".into()));
        }
        if self.name.trim().is_empty() {
            return Err(FormatError::Invalid("name must not be blank".into()));
        }
        let minecraft = self
            .dependencies
            .get("minecraft")
            .ok_or_else(|| FormatError::Invalid("dependencies.minecraft is required".into()))?;
        if minecraft.trim().is_empty() {
            return Err(FormatError::Invalid(
                "dependencies.minecraft must not be blank".into(),
            ));
        }
        let loaders = ["fabric-loader", "quilt-loader", "forge", "neoforge"];
        let mut loader_count = 0;
        for (key, value) in &self.dependencies {
            if key != "minecraft" && !loaders.contains(&key.as_str()) {
                return Err(FormatError::Unsupported(format!("dependency `{key}`")));
            }
            if value.trim().is_empty() {
                return Err(FormatError::Invalid(format!(
                    "dependency `{key}` must not be blank"
                )));
            }
            if key != "minecraft" {
                loader_count += 1;
            }
        }
        if loader_count > 1 {
            return Err(FormatError::Invalid(
                "at most one mod loader dependency is allowed".into(),
            ));
        }

        let mut seen = HashSet::new();
        for file in &self.files {
            validate_content_path(&file.path)?;
            let key = file.path.to_lowercase();
            if !seen.insert(key) {
                return Err(FormatError::Collision(file.path.clone()));
            }
            for (algorithm, length) in [("sha1", 40), ("sha512", 128)] {
                let digest = file.hashes.get(algorithm).ok_or_else(|| {
                    FormatError::Invalid(format!("{} is missing {algorithm}", file.path))
                })?;
                if digest.len() != length || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(FormatError::Invalid(format!(
                        "{} has an invalid {algorithm} hash",
                        file.path
                    )));
                }
            }
            if file.downloads.is_empty() {
                return Err(FormatError::Invalid(format!(
                    "{} has no download URLs",
                    file.path
                )));
            }
            for url in &file.downloads {
                validate_https_url(url)?;
            }
        }
        validate_file_tree(self.files.iter().map(|f| f.path.as_str()))
    }
}

impl PackFile {
    pub fn client_required(&self, include_optional: bool) -> bool {
        match self.env.as_ref().map(|e| e.client) {
            None | Some(EnvironmentValue::Required) => true,
            Some(EnvironmentValue::Optional) => include_optional,
            Some(EnvironmentValue::Unsupported) => false,
        }
    }
}

/// Validate a path that will be written into an instance directory.
pub fn validate_content_path(path: &str) -> Result<(), FormatError> {
    let unsafe_path = || FormatError::UnsafePath(path.to_owned());
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.contains('\0')
        || path
            .chars()
            .any(|c| c.is_control() || "<>\"|?*".contains(c))
    {
        return Err(unsafe_path());
    }
    let parts: Vec<_> = path.split('/').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || *p == "." || *p == ".." || p.ends_with('.') || p.ends_with(' '))
    {
        return Err(unsafe_path());
    }
    for part in &parts {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(
            stem.as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        ) {
            return Err(unsafe_path());
        }
    }
    let first = parts[0].to_ascii_lowercase();
    let allowed_dir = [
        "mods",
        "resourcepacks",
        "shaderpacks",
        "config",
        "defaultconfigs",
        "datapacks",
        "saves",
    ];
    let allowed_root_file = [
        "options.txt",
        "servers.dat",
        "servers.dat_old",
        "servers.dat_new",
        "optionsof.txt",
        "optionsshaders.txt",
        "usercache.json",
        "realms_persistence.json",
    ];
    if parts.len() == 1 {
        if !allowed_root_file.contains(&first.as_str()) {
            return Err(unsafe_path());
        }
    } else if !allowed_dir.contains(&first.as_str()) {
        return Err(unsafe_path());
    }
    let forbidden = [
        ".ccl",
        ".minecraft",
        "libraries",
        "versions",
        "assets",
        "logs",
        "crash-reports",
        "runtime",
        "natives",
        "launcher_profiles.json",
        "launcher_accounts.json",
        "accounts.json",
        "credentials",
        "scripts",
    ];
    if parts
        .iter()
        .any(|p| forbidden.contains(&p.to_ascii_lowercase().as_str()))
    {
        return Err(unsafe_path());
    }
    Ok(())
}

/// Validate URL syntax and HTTPS policy. Explicit extra hosts are exact host matches.
pub fn validate_download_url(url: &str, extra_hosts: &[String]) -> Result<(), FormatError> {
    let invalid = |reason: &str| FormatError::InvalidUrl {
        url: url.to_owned(),
        reason: reason.to_owned(),
    };
    let parsed = validate_https_url(url)?;
    let host = parsed
        .host_str()
        .ok_or_else(|| invalid("host is missing"))?
        .to_ascii_lowercase();
    let allowed = [
        "cdn.modrinth.com",
        "github.com",
        "raw.githubusercontent.com",
        "gitlab.com",
    ];
    if !allowed.contains(&host.as_str())
        && !extra_hosts.iter().any(|h| h.eq_ignore_ascii_case(&host))
    {
        return Err(invalid("host is not in the download allowlist"));
    }
    Ok(())
}

fn validate_https_url(url: &str) -> Result<reqwest::Url, FormatError> {
    let invalid = |reason: &str| FormatError::InvalidUrl {
        url: url.to_owned(),
        reason: reason.to_owned(),
    };
    if url
        .bytes()
        .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return Err(invalid(
            "spaces and control characters must be percent-encoded",
        ));
    }
    let parsed = reqwest::Url::parse(url).map_err(|e| invalid(&e.to_string()))?;
    if parsed.scheme() != "https" {
        return Err(invalid("HTTPS is required"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(invalid("userinfo is not allowed"));
    }
    if parsed.host_str().is_none() {
        return Err(invalid("host is missing"));
    }
    Ok(parsed)
}

pub fn read_pack(path: &Path) -> Result<PackArchive, FormatError> {
    let file = fs::File::open(path)?;
    let mut zip = ZipArchive::new(file)?;
    if zip.len() > MAX_ENTRIES {
        return Err(FormatError::Invalid("archive has too many entries".into()));
    }
    let mut names = HashSet::new();
    let mut total = 0u64;
    let mut manifest_bytes = None;
    let mut overrides: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut client_overrides = BTreeMap::new();
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        let name = entry.name()?.into_owned();
        let is_dir = entry.is_dir();
        let clean = name.strip_suffix('/').unwrap_or(&name);
        if clean.is_empty()
            || clean.starts_with('/')
            || clean.contains('\\')
            || clean.contains(':')
            || clean
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err(FormatError::UnsafePath(name));
        }
        let folded = clean.to_lowercase();
        if !names.insert(folded.clone()) {
            return Err(FormatError::Collision(name));
        }
        if let Some(mode) = entry.unix_mode()
            && mode & 0o170000 == 0o120000
        {
            return Err(FormatError::Invalid(format!(
                "symbolic link entry `{name}` is forbidden"
            )));
        }
        let size = entry.size();
        if !is_dir && size > MAX_FILE_SIZE {
            return Err(FormatError::TooLarge(name));
        }
        total = total
            .checked_add(size)
            .ok_or_else(|| FormatError::TooLarge(name.clone()))?;
        if total > MAX_TOTAL_SIZE {
            return Err(FormatError::TooLarge("archive total".into()));
        }
        if is_dir {
            continue;
        }
        let mut data = Vec::with_capacity(size as usize);
        entry.take(MAX_FILE_SIZE + 1).read_to_end(&mut data)?;
        if data.len() as u64 != size {
            return Err(FormatError::Invalid(format!(
                "entry `{name}` has inconsistent size"
            )));
        }
        if name == MANIFEST_NAME {
            manifest_bytes = Some(data);
        } else if let Some(relative) = name.strip_prefix("overrides/") {
            validate_content_path(relative)?;
            overrides.insert(relative.to_owned(), data);
        } else if let Some(relative) = name.strip_prefix("client-overrides/") {
            validate_content_path(relative)?;
            client_overrides.insert(relative.to_owned(), data);
        } else if let Some(relative) = name.strip_prefix("server-overrides/") {
            // Server files are not installed here, but paths are still checked before ignoring them.
            validate_content_path(relative)?;
        } else {
            return Err(FormatError::Invalid(format!(
                "unexpected archive entry `{name}`"
            )));
        }
    }
    let manifest: Manifest = serde_json::from_slice(
        &manifest_bytes
            .ok_or_else(|| FormatError::Invalid("missing root modrinth.index.json".into()))?,
    )?;
    manifest.validate()?;
    validate_override_collisions(&overrides, &client_overrides)?;
    overrides.extend(client_overrides);
    validate_combined_tree(&manifest, &overrides)?;
    Ok(PackArchive {
        manifest,
        overrides,
    })
}

pub fn write_pack(
    path: &Path,
    manifest: &Manifest,
    overrides: &BTreeMap<String, Vec<u8>>,
) -> Result<(), FormatError> {
    manifest.validate()?;
    validate_combined_tree(manifest, overrides)?;
    validate_file_tree(overrides.keys().map(String::as_str))?;
    let mut total = 0u64;
    for (name, data) in overrides {
        validate_content_path(name)?;
        if data.len() as u64 > MAX_FILE_SIZE {
            return Err(FormatError::TooLarge(name.clone()));
        }
        total += data.len() as u64;
    }
    if overrides.len() + 1 > MAX_ENTRIES || total > MAX_TOTAL_SIZE {
        return Err(FormatError::TooLarge("archive total".into()));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| FormatError::Invalid("output path has no file name".into()))?
        .to_string_lossy();
    let mut temp_path = None;
    let mut temp_file = None;
    for _ in 0..16 {
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{file_name}.{}.{}.tmp", std::process::id(), id));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temp_path = Some(candidate);
                temp_file = Some(file);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let temp_path = temp_path
        .ok_or_else(|| FormatError::Invalid("could not allocate temporary output".into()))?;
    let result = (|| -> Result<(), FormatError> {
        let mut writer = ZipWriter::new(temp_file.take().expect("temporary file allocated"));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file(MANIFEST_NAME, options)?;
        writer.write_all(&serde_json::to_vec_pretty(manifest)?)?;
        for (name, data) in overrides {
            writer.start_file(format!("overrides/{name}"), options)?;
            writer.write_all(data)?;
        }
        let file = writer.finish()?;
        file.sync_all()?;
        match fs::hard_link(&temp_path, path) {
            Ok(()) => {
                fs::remove_file(&temp_path)?;
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(FormatError::Exists(path.display().to_string()))
            }
            Err(e) => Err(e.into()),
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn validate_file_tree<'a>(paths: impl Iterator<Item = &'a str>) -> Result<(), FormatError> {
    let mut nodes = HashMap::<String, (String, bool)>::new();
    for path in paths {
        let parts: Vec<_> = path.split('/').collect();
        for i in 1..=parts.len() {
            let prefix = parts[..i].join("/");
            let folded = prefix.to_lowercase();
            let is_file = i == parts.len();
            if let Some((previous, was_file)) = nodes.get(&folded) {
                if previous != &prefix || *was_file || is_file {
                    return Err(FormatError::Collision(path.to_owned()));
                }
            } else {
                nodes.insert(folded, (prefix, is_file));
            }
        }
    }
    Ok(())
}

fn validate_combined_tree(
    manifest: &Manifest,
    overrides: &BTreeMap<String, Vec<u8>>,
) -> Result<(), FormatError> {
    let paths: HashSet<&str> = manifest
        .files
        .iter()
        .map(|f| f.path.as_str())
        .chain(overrides.keys().map(String::as_str))
        .collect();
    validate_file_tree(paths.into_iter())
}

fn validate_override_collisions(
    a: &BTreeMap<String, Vec<u8>>,
    b: &BTreeMap<String, Vec<u8>>,
) -> Result<(), FormatError> {
    let mut keys: HashMap<String, String> =
        a.keys().map(|s| (s.to_lowercase(), s.clone())).collect();
    for key in b.keys() {
        if let Some(previous) = keys.get(&key.to_lowercase()) {
            if previous != key {
                return Err(FormatError::Collision(key.clone()));
            }
        } else {
            keys.insert(key.to_lowercase(), key.clone());
        }
    }
    let merged: HashSet<&str> = a.keys().chain(b.keys()).map(String::as_str).collect();
    validate_file_tree(merged.into_iter())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn manifest(files: Vec<PackFile>) -> Manifest {
        Manifest {
            format_version: 1,
            game: "minecraft".into(),
            version_id: "v1".into(),
            name: "Pack".into(),
            summary: None,
            files,
            dependencies: [("minecraft".into(), "1.20.1".into())].into(),
        }
    }
    fn pack_file(path: &str) -> PackFile {
        PackFile {
            path: path.into(),
            hashes: [
                ("sha1".into(), "a".repeat(40)),
                ("sha512".into(), "b".repeat(128)),
            ]
            .into(),
            env: None,
            downloads: vec!["https://cdn.modrinth.com/data/a.jar".into()],
            file_size: 1,
        }
    }
    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn rejects_directory_case_aliases_and_cross_layer_file_parents() {
        assert!(validate_file_tree(["config/Foo/a", "config/foo/b"].into_iter()).is_err());
        for files in [["config/a/b", "config/a"], ["config/a", "config/a/b"]] {
            assert!(validate_file_tree(files.into_iter()).is_err());
        }
        let m = manifest(vec![pack_file("config/a")]);
        let overrides = [("config/a/b".into(), b"data".to_vec())].into();
        assert!(validate_combined_tree(&m, &overrides).is_err());
        let legal = [("config/a".into(), b"replacement".to_vec())].into();
        assert!(validate_combined_tree(&m, &legal).is_ok());
    }

    #[test]
    fn validates_path_policy_and_windows_names() {
        for path in [
            "../mods/a.jar",
            "/mods/a.jar",
            "C:/mods/a.jar",
            "mods\\a.jar",
            "mods/CON.txt",
            "logs/latest.log",
            "scripts/run.sh",
            "mods/a. ",
            "mods/a?.jar",
            "config/a\u{1f}b",
        ] {
            assert!(validate_content_path(path).is_err(), "{path}");
        }
        assert!(validate_content_path("mods/a.jar").is_ok());
        assert!(validate_content_path("options.txt").is_ok());
    }

    #[test]
    fn validates_dependencies_hashes_and_environment_selection() {
        let mut m = manifest(vec![pack_file("mods/a.jar")]);
        m.dependencies.insert("quilt-loader".into(), "0.20".into());
        assert!(m.validate().is_ok());
        let mut m = manifest(vec![pack_file("mods/a.jar")]);
        m.dependencies.insert("forge".into(), "47".into());
        m.dependencies.insert("neoforge".into(), "20".into());
        assert!(m.validate().is_err());
        let mut file = pack_file("mods/optional.jar");
        file.env = Some(Environment {
            client: EnvironmentValue::Optional,
            server: EnvironmentValue::Required,
        });
        assert!(!file.client_required(false));
        assert!(file.client_required(true));
        file.env.as_mut().unwrap().client = EnvironmentValue::Unsupported;
        assert!(!file.client_required(true));
    }

    #[test]
    fn url_policy_allows_custom_https_host_explicitly() {
        assert!(validate_download_url("http://github.com/a", &[]).is_err());
        assert!(validate_download_url("https://evil.example/a", &[]).is_err());
        assert!(validate_download_url("https://mods.example/a", &["mods.example".into()]).is_ok());
        let mut custom = manifest(vec![pack_file("mods/a.jar")]);
        custom.files[0].downloads = vec!["https://mods.example/a".into()];
        assert!(custom.validate().is_ok());
    }

    #[test]
    fn rejects_archive_traversal_and_case_collisions() {
        let json = serde_json::to_vec(&manifest(vec![])).unwrap();
        let malicious = make_zip(&[(MANIFEST_NAME, &json), ("overrides/../mods/a", b"x")]);
        assert!(read_pack_bytes(&malicious).is_err());
        let collision = make_zip(&[
            (MANIFEST_NAME, &json),
            ("overrides/config/A.txt", b"a"),
            ("overrides/config/a.txt", b"b"),
        ]);
        assert!(matches!(
            read_pack_bytes(&collision),
            Err(FormatError::Collision(_))
        ));
    }

    #[test]
    fn client_layer_wins_and_server_layer_is_validated_but_ignored() {
        let json = serde_json::to_vec(&manifest(vec![])).unwrap();
        let bytes = make_zip(&[
            (MANIFEST_NAME, &json),
            ("overrides/options.txt", b"base"),
            ("client-overrides/options.txt", b"client"),
            ("server-overrides/options.txt", b"server"),
        ]);
        // Same path in layers is a valid replacement.
        let archive = read_pack_bytes(&bytes).unwrap();
        assert_eq!(archive.overrides["options.txt"], b"client");
    }

    #[test]
    fn writer_roundtrips_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!(
            "mrpack-test-{}",
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pack.mrpack");
        let overrides = [("options.txt".into(), b"ok".to_vec())].into();
        write_pack(&path, &manifest(vec![pack_file("mods/a.jar")]), &overrides).unwrap();
        assert!(matches!(
            write_pack(&path, &manifest(vec![]), &BTreeMap::new()),
            Err(FormatError::Exists(_))
        ));
        let loaded = read_pack(&path).unwrap();
        assert_eq!(loaded.overrides, overrides);
        fs::remove_dir_all(dir).unwrap();
    }

    fn read_pack_bytes(bytes: &[u8]) -> Result<PackArchive, FormatError> {
        let path = std::env::temp_dir().join(format!(
            "mrpack-read-{}.zip",
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, bytes)?;
        let result = read_pack(&path);
        let _ = fs::remove_file(path);
        result
    }
}
