//! Installation and validation of Mojang's component based Java runtimes.
//!
//! Runtime manifests describe files individually (including hashes and sizes),
//! so no archive extraction is needed. Files are assembled in a private staging
//! directory and exposed only after the Java executable passes its own probe.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Component as PathComponent, Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;
use serde::Serialize;
use sha1::{Digest, Sha1};
use thiserror::Error;

use crate::{
    TaskContext, TaskEvent,
    config::DownloadConfig,
    download::{DownloadManager, DownloadRequest},
    java::{self, JavaRuntime},
    source::{self, DownloadKind},
};

const RUNTIME_CATALOG: &str = "https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime installation cancelled")]
    Cancelled,
    #[error("unsupported runtime platform: {0}")]
    UnsupportedPlatform(String),
    #[error("runtime component {0} is unavailable for this platform")]
    MissingComponent(String),
    #[error("invalid Mojang runtime manifest: {0}")]
    InvalidManifest(String),
    #[error("runtime installation failed: {0}")]
    Download(#[from] crate::download::DownloadError),
    #[error("runtime installation I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("installed runtime is not valid: {0}")]
    InvalidRuntime(String),
}

type Result<T> = std::result::Result<T, RuntimeError>;

/// Find a complete, valid managed Java runtime without creating directories or
/// making network requests. Missing or corrupt installations return `Ok(None)`;
/// unsafe cache roots are reported as errors.
pub fn find_installed_runtime(
    game_directory: &Path,
    component: &str,
    required_major: u32,
) -> std::result::Result<Option<JavaRuntime>, RuntimeError> {
    let platform = platform_key()?;
    let component = safe_component(component)?;
    let game_root = match fs::canonicalize(game_directory) {
        Ok(root) => root,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io(game_directory, error)),
    };
    let ccl_dir = game_root.join(".ccl");
    if !existing_real_directory(&ccl_dir)? {
        return Ok(None);
    }
    let root = ccl_dir.join("runtimes");
    if !existing_real_directory(&root)? {
        return Ok(None);
    }
    let runtime_dir = root.join(format!("{component}-{platform}"));
    Ok(valid_runtime(&runtime_dir, required_major))
}

fn existing_real_directory(path: &Path) -> std::result::Result<bool, RuntimeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(RuntimeError::InvalidManifest(format!(
            "runtime directory is not a real directory: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io(path, error)),
    }
}

#[derive(Debug, Deserialize)]
struct CatalogEntry {
    manifest: Artifact,
    version: RuntimeVersion,
}
#[derive(Debug, Deserialize)]
struct RuntimeVersion {
    name: String,
}
#[derive(Debug, Deserialize)]
struct Artifact {
    url: String,
    sha1: String,
    size: u64,
}
#[derive(Debug, Deserialize)]
struct RuntimeManifest {
    files: BTreeMap<String, RuntimeFile>,
}
#[derive(Debug, Serialize, Deserialize)]
struct RuntimeIntegrity {
    files: Vec<IntegrityEntry>,
}
#[derive(Debug, Serialize, Deserialize)]
struct IntegrityEntry {
    path: String,
    kind: String,
    sha1: Option<String>,
    size: Option<u64>,
    target: Option<String>,
}
#[derive(Debug, Deserialize)]
struct RuntimeFile {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    executable: bool,
    #[serde(default)]
    downloads: Option<FileDownloads>,
    #[serde(default)]
    target: Option<String>,
}
#[derive(Debug, Deserialize)]
struct FileDownloads {
    raw: Option<Artifact>,
}

/// Ensure a Mojang managed runtime is installed and return its probed Java.
/// `component` is the `javaVersion.component` value from version metadata.
pub async fn ensure_runtime(
    game_directory: &Path,
    component: &str,
    required_major: u32,
    download_config: &DownloadConfig,
    context: Option<TaskContext>,
) -> Result<JavaRuntime> {
    ensure_runtime_with_catalog(
        game_directory,
        component,
        required_major,
        download_config,
        context,
        RUNTIME_CATALOG,
    )
    .await
}

/// Same as [`ensure_runtime`] with an alternate catalog endpoint, intended for
/// mirrors and integration tests. The selected component file URLs still use
/// the configured source policy where a recognized official Mojang host is used.
pub async fn ensure_runtime_with_catalog(
    game_directory: &Path,
    component: &str,
    required_major: u32,
    download_config: &DownloadConfig,
    context: Option<TaskContext>,
    catalog_url: &str,
) -> Result<JavaRuntime> {
    check_context(&context)?;
    let platform = platform_key()?;
    let game_root = fs::canonicalize(game_directory).map_err(|e| io(game_directory, e))?;
    let ccl_dir = game_root.join(".ccl");
    ensure_real_directory(&ccl_dir)?;
    let root = ccl_dir.join("runtimes");
    ensure_real_directory(&root)?;
    let final_dir = root.join(format!("{}-{platform}", safe_component(component)?));
    if let Some(runtime) = valid_runtime(&final_dir, required_major) {
        return Ok(runtime);
    }

    let _lock = InstallLock::acquire(
        root.join(format!("{}-{platform}.lock", safe_component(component)?)),
        &context,
    )
    .await?;
    cleanup_staging(&root, component, &platform)?;
    if let Some(runtime) = valid_runtime(&final_dir, required_major) {
        return Ok(runtime);
    }

    emit(
        &context,
        TaskEvent::Started {
            task: format!("install Java runtime {component}"),
        },
    )?;
    let manager = DownloadManager::new(download_config)?;
    let catalog_path = root.join(format!(
        ".catalog-{platform}-{}.json",
        safe_component(component)?
    ));
    let catalog_req = DownloadRequest::new(
        source::url_candidates(catalog_url, DownloadKind::VersionList, download_config),
        &catalog_path,
    );
    let catalog_bytes = fetch_json(&manager, catalog_req, &context).await?;
    let catalog: BTreeMap<String, BTreeMap<String, Vec<CatalogEntry>>> =
        serde_json::from_slice(&catalog_bytes)
            .map_err(|e| RuntimeError::InvalidManifest(e.to_string()))?;
    let entry = catalog
        .get(&platform)
        .and_then(|components| components.get(component))
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| version_major(&entry.version.name) == required_major)
        })
        .ok_or_else(|| RuntimeError::MissingComponent(component.into()))?;
    let manifest_tmp = root.join(format!(
        ".manifest-{platform}-{}.json",
        safe_component(component)?
    ));
    let manifest_req = DownloadRequest::new(
        source::url_candidates(&entry.manifest.url, DownloadKind::File, download_config),
        &manifest_tmp,
    )
    .with_sha1(entry.manifest.sha1.clone())
    .with_size(entry.manifest.size);
    let manifest_bytes = fetch_json(&manager, manifest_req, &context).await?;
    let manifest: RuntimeManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| RuntimeError::InvalidManifest(e.to_string()))?;
    let component_files = select_component_files(&manifest, component)?;

    let staging = root.join(format!(
        ".staging-{component}-{platform}-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let mut staging_guard = StagingDir::create(staging)?;
    let staging = staging_guard.path.clone();
    reuse_valid_files(&final_dir, &staging, &component_files)?;
    let result = install_files(
        &manager,
        &staging,
        &component_files,
        &context,
        download_config,
    )
    .await;
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    write_integrity(&staging, &component_files)?;
    let executable = java_executable(&staging);
    let runtime =
        java::inspect_java(&executable).map_err(|e| RuntimeError::InvalidRuntime(e.to_string()))?;
    if runtime.major != required_major {
        let _ = fs::remove_dir_all(&staging);
        return Err(RuntimeError::InvalidRuntime(format!(
            "expected Java {required_major}, found {}",
            runtime.major
        )));
    }
    check_context(&context)?;
    if final_dir.exists() {
        fs::remove_dir_all(&final_dir).map_err(|e| io(&final_dir, e))?;
    }
    fs::rename(&staging, &final_dir).map_err(|e| io(&final_dir, e))?;
    staging_guard.disarm();
    let runtime = java::inspect_java(&java_executable(&final_dir))
        .map_err(|e| RuntimeError::InvalidRuntime(e.to_string()))?;
    emit(
        &context,
        TaskEvent::Finished {
            task: format!("install Java runtime {component}"),
        },
    )?;
    Ok(runtime)
}

async fn fetch_json(
    manager: &DownloadManager,
    request: DownloadRequest,
    context: &Option<TaskContext>,
) -> Result<Vec<u8>> {
    let path = request.destination.clone();
    let request = if let Some(c) = context {
        request.with_task_context(c.clone())
    } else {
        request
    };
    manager.download(request).await?;
    fs::read(&path).map_err(|e| io(&path, e))
}

fn select_component_files<'a>(
    manifest: &'a RuntimeManifest,
    component: &str,
) -> Result<Vec<(&'a str, &'a RuntimeFile)>> {
    let prefix = format!("{component}/");
    let mut files = Vec::new();
    for (path, file) in &manifest.files {
        let relative = path.strip_prefix(&prefix).unwrap_or(path);
        if relative.is_empty() {
            continue;
        }
        validate_relative(relative)?;
        files.push((relative, file));
    }
    if files.is_empty() {
        return Err(RuntimeError::MissingComponent(component.into()));
    }
    let links: Vec<&str> = files
        .iter()
        .filter_map(|(path, file)| (file.kind == "link").then_some(*path))
        .collect();
    for link in &links {
        if files
            .iter()
            .any(|(path, _)| path.starts_with(&format!("{link}/")))
        {
            return Err(RuntimeError::InvalidManifest(format!(
                "link is used as a directory: {link}"
            )));
        }
    }
    Ok(files)
}

async fn install_files(
    manager: &DownloadManager,
    staging: &Path,
    files: &[(&str, &RuntimeFile)],
    context: &Option<TaskContext>,
    config: &DownloadConfig,
) -> Result<()> {
    // Directories and files are materialized before links. This also permits
    // Windows hard links whose target appears later in Mojang's sorted map.
    for (relative, file) in files.iter().filter(|(_, f)| f.kind != "link") {
        check_context(context)?;
        let path = safe_join(staging, relative)?;
        match file.kind.as_str() {
            "directory" => {
                fs::create_dir_all(&path).map_err(|e| io(&path, e))?;
            }
            "file" => {
                let artifact = file
                    .downloads
                    .as_ref()
                    .and_then(|d| d.raw.as_ref())
                    .ok_or_else(|| {
                        RuntimeError::InvalidManifest(format!(
                            "missing raw download for {relative}"
                        ))
                    })?;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
                }
                let request = DownloadRequest::new(
                    source::url_candidates(&artifact.url, DownloadKind::File, config),
                    &path,
                )
                .with_sha1(artifact.sha1.clone())
                .with_size(artifact.size);
                let request = if let Some(c) = context {
                    request.with_task_context(c.clone())
                } else {
                    request
                };
                manager.download(request).await?;
                if file.executable {
                    set_executable(&path)?;
                }
            }
            "link" => unreachable!(),
            kind => {
                return Err(RuntimeError::InvalidManifest(format!(
                    "unknown file type {kind}"
                )));
            }
        }
    }
    let links: Vec<_> = files.iter().filter(|(_, f)| f.kind == "link").collect();
    #[cfg(unix)]
    for (relative, file) in links {
        check_context(context)?;
        let target = file.target.as_deref().ok_or_else(|| {
            RuntimeError::InvalidManifest(format!("missing link target for {relative}"))
        })?;
        let path = safe_join(staging, relative)?;
        create_safe_link(staging, &path, target)?;
    }
    #[cfg(windows)]
    {
        let mut pending = links;
        while !pending.is_empty() {
            check_context(context)?;
            let before = pending.len();
            pending.retain(|(relative, file)| {
                let target = file.target.as_deref().unwrap_or("");
                let Ok(path) = safe_join(staging, relative) else {
                    return true;
                };
                match create_safe_link(staging, &path, target) {
                    Ok(()) => false,
                    Err(RuntimeError::Io { source, .. })
                        if source.kind() == std::io::ErrorKind::NotFound =>
                    {
                        true
                    }
                    Err(_) => true,
                }
            });
            if pending.len() == before {
                return Err(RuntimeError::InvalidManifest(
                    "unresolvable Windows runtime links".into(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_relative(value: &str) -> Result<()> {
    let path = Path::new(value);
    if value == ".ccl-integrity.json"
        || path.is_absolute()
        || value.contains('\\')
        || path
            .components()
            .any(|c| !matches!(c, PathComponent::Normal(_)))
    {
        return Err(RuntimeError::InvalidManifest(format!(
            "unsafe runtime path {value}"
        )));
    }
    Ok(())
}
fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_relative(relative)?;
    Ok(root.join(relative))
}
fn create_safe_link(root: &Path, path: &Path, target: &str) -> Result<()> {
    if target.is_empty()
        || target.starts_with('/')
        || target.contains('\\')
        || target.as_bytes().contains(&0)
    {
        return Err(RuntimeError::InvalidManifest(format!(
            "unsafe link target {target}"
        )));
    }
    let parent = path.parent().unwrap_or(root);
    let target_path = normalize_under(
        root,
        &parent
            .strip_prefix(root)
            .unwrap_or(Path::new(""))
            .join(target),
    )?;
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| io(p, e))?;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, path).map_err(|e| io(path, e))?;
    }
    #[cfg(windows)]
    {
        // Mojang runtime links commonly alias a sibling executable/library. A
        // hard link keeps the target confined to the staged runtime tree.
        fs::hard_link(&target_path, path).map_err(|e| io(path, e))?;
    }
    #[cfg(unix)]
    let _ = target_path;
    Ok(())
}

fn reuse_valid_files(
    old_runtime: &Path,
    staging: &Path,
    files: &[(&str, &RuntimeFile)],
) -> Result<()> {
    if fs::symlink_metadata(old_runtime)
        .map(|meta| !meta.is_dir() || meta.file_type().is_symlink())
        .unwrap_or(true)
    {
        return Ok(());
    }
    for (relative, _) in files.iter().filter(|(_, file)| file.kind == "directory") {
        let destination = safe_join(staging, relative)?;
        fs::create_dir_all(&destination).map_err(|e| io(&destination, e))?;
    }
    for (relative, file) in files.iter().filter(|(_, file)| file.kind == "file") {
        let artifact = file
            .downloads
            .as_ref()
            .and_then(|d| d.raw.as_ref())
            .ok_or_else(|| {
                RuntimeError::InvalidManifest(format!("missing raw download for {relative}"))
            })?;
        let old = safe_join(old_runtime, relative)?;
        let new = safe_join(staging, relative)?;
        if !regular_file_matches(&old, &artifact.sha1, artifact.size) {
            continue;
        }
        if let Some(parent) = new.parent() {
            fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        }
        fs::copy(&old, &new).map_err(|e| io(&new, e))?;
        if file.executable {
            set_executable(&new)?;
        }
    }
    Ok(())
}

fn write_integrity(staging: &Path, files: &[(&str, &RuntimeFile)]) -> Result<()> {
    let entries = files
        .iter()
        .map(|(path, file)| {
            let artifact = file.downloads.as_ref().and_then(|d| d.raw.as_ref());
            IntegrityEntry {
                path: (*path).to_owned(),
                kind: file.kind.clone(),
                sha1: artifact.map(|a| a.sha1.to_ascii_lowercase()),
                size: artifact.map(|a| a.size),
                target: file.target.clone(),
            }
        })
        .collect();
    let marker = staging.join(".ccl-integrity.json");
    let bytes = serde_json::to_vec(&RuntimeIntegrity { files: entries })
        .map_err(|e| RuntimeError::InvalidManifest(e.to_string()))?;
    fs::write(&marker, bytes).map_err(|e| io(&marker, e))
}

fn verify_integrity(runtime: &Path) -> Result<()> {
    let marker = runtime.join(".ccl-integrity.json");
    let bytes = fs::read(&marker).map_err(|e| io(&marker, e))?;
    let integrity: RuntimeIntegrity =
        serde_json::from_slice(&bytes).map_err(|e| RuntimeError::InvalidManifest(e.to_string()))?;
    for entry in integrity.files {
        let path = safe_join(runtime, &entry.path)?;
        match entry.kind.as_str() {
            "directory" => {
                let metadata = fs::symlink_metadata(&path).map_err(|e| io(&path, e))?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(RuntimeError::InvalidRuntime(format!(
                        "invalid directory {}",
                        path.display()
                    )));
                }
            }
            "file" => {
                let sha = entry.sha1.ok_or_else(|| {
                    RuntimeError::InvalidManifest("integrity file missing SHA-1".into())
                })?;
                let size = entry.size.ok_or_else(|| {
                    RuntimeError::InvalidManifest("integrity file missing size".into())
                })?;
                if !regular_file_matches(&path, &sha, size) {
                    return Err(RuntimeError::InvalidRuntime(format!(
                        "integrity check failed for {}",
                        path.display()
                    )));
                }
            }
            "link" => {
                let target = entry.target.ok_or_else(|| {
                    RuntimeError::InvalidManifest("integrity link missing target".into())
                })?;
                let _ = normalize_under(
                    runtime,
                    &path
                        .parent()
                        .unwrap_or(runtime)
                        .strip_prefix(runtime)
                        .unwrap_or(Path::new(""))
                        .join(&target),
                )?;
                let actual = fs::read_link(&path).map_err(|e| io(&path, e))?;
                if actual != Path::new(&target) {
                    return Err(RuntimeError::InvalidRuntime(format!(
                        "link check failed for {}",
                        path.display()
                    )));
                }
            }
            _ => {
                return Err(RuntimeError::InvalidManifest(format!(
                    "unknown integrity entry type {}",
                    entry.kind
                )));
            }
        }
    }
    Ok(())
}

fn regular_file_matches(path: &Path, expected_sha1: &str, expected_size: u64) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() != expected_size {
        return false;
    }
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut hasher = Sha1::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buffer[..n]),
            Err(_) => return false,
        }
    }
    format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(expected_sha1)
}
fn normalize_under(root: &Path, relative: &Path) -> Result<PathBuf> {
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in relative.components() {
        match component {
            PathComponent::Normal(part) => parts.push(part),
            PathComponent::CurDir => {}
            PathComponent::ParentDir if !parts.is_empty() => {
                parts.pop();
            }
            _ => {
                return Err(RuntimeError::InvalidManifest(
                    "link escapes runtime root".into(),
                ));
            }
        }
    }
    let mut result = root.to_path_buf();
    for part in parts {
        result.push(part);
    }
    Ok(result)
}
#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut p = fs::metadata(path).map_err(|e| io(path, e))?.permissions();
    p.set_mode(p.mode() | 0o111);
    fs::set_permissions(path, p).map_err(|e| io(path, e))
}
#[cfg(not(unix))]
fn set_executable(_: &Path) -> Result<()> {
    Ok(())
}

fn valid_runtime(dir: &Path, major: u32) -> Option<JavaRuntime> {
    let metadata = fs::symlink_metadata(dir).ok()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return None;
    }
    verify_integrity(dir).ok()?;
    java::inspect_java(&java_executable(dir))
        .ok()
        .filter(|r| r.major == major && architecture_ok(&r.architecture))
}
fn architecture_ok(actual: &str) -> bool {
    let a = actual.to_ascii_lowercase();
    let h = std::env::consts::ARCH.to_ascii_lowercase();
    (a == h)
        || matches!(
            (a.as_str(), h.as_str()),
            ("amd64", "x86_64") | ("x86_64", "amd64") | ("aarch64", "arm64") | ("arm64", "aarch64")
        )
}
fn java_executable(home: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    let exe = "bin/java.exe";
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let exe = "bin/java";
    #[cfg(target_os = "macos")]
    {
        let bundle_exe = home.join("jre.bundle/Contents/Home/bin/java");
        if bundle_exe.is_file() {
            return bundle_exe;
        }
        home.join("bin/java")
    }
    #[cfg(not(target_os = "macos"))]
    home.join(exe)
}
fn platform_key() -> Result<String> {
    let os = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "mac-os",
        "linux" => "linux",
        other => return Err(RuntimeError::UnsupportedPlatform(other.into())),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" | "amd64" => "x64",
        "aarch64" | "arm64" => "arm64",
        other => return Err(RuntimeError::UnsupportedPlatform(other.into())),
    };
    let key = match (os, arch) {
        ("linux", "x64") => "linux",
        ("mac-os", "x64") => "mac-os",
        ("mac-os", "arm64") => "mac-os-arm64",
        ("windows", "x64") => "windows-x64",
        _ => return Err(RuntimeError::UnsupportedPlatform(format!("{os}-{arch}"))),
    };
    Ok(key.to_owned())
}
fn version_major(v: &str) -> u32 {
    let mut parts = v
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty());
    let first = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    if first == 1 {
        return parts.next().and_then(|part| part.parse().ok()).unwrap_or(1);
    }
    first
}
fn safe_component(c: &str) -> Result<&str> {
    if c.is_empty()
        || !c
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(RuntimeError::InvalidManifest(
            "unsafe runtime component name".into(),
        ));
    }
    Ok(c)
}
fn check_context(context: &Option<TaskContext>) -> Result<()> {
    if context
        .as_ref()
        .is_some_and(|c| c.cancellation.is_cancelled())
    {
        Err(RuntimeError::Cancelled)
    } else {
        Ok(())
    }
}
fn emit(context: &Option<TaskContext>, event: TaskEvent) -> Result<()> {
    if let Some(c) = context {
        c.emit(event).map_err(|_| RuntimeError::Cancelled)?;
    }
    Ok(())
}
fn io(path: &Path, source: std::io::Error) -> RuntimeError {
    RuntimeError::Io {
        path: path.to_path_buf(),
        source,
    }
}
fn ensure_real_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(RuntimeError::InvalidManifest(format!(
            "runtime directory is not a real directory: {}",
            path.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|e| io(path, e))
        }
        Err(e) => Err(io(path, e)),
    }
}
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn cleanup_staging(root: &Path, component: &str, platform: &str) -> Result<()> {
    let prefix = format!(".staging-{component}-{platform}-");
    for entry in fs::read_dir(root).map_err(|e| io(root, e))? {
        let entry = entry.map_err(|e| io(root, e))?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|e| io(&path, e))?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                fs::remove_dir_all(&path).map_err(|e| io(&path, e))?;
            } else {
                fs::remove_file(&path).map_err(|e| io(&path, e))?;
            }
        }
    }
    Ok(())
}

struct StagingDir {
    path: PathBuf,
    armed: bool,
}
impl StagingDir {
    fn create(path: PathBuf) -> Result<Self> {
        fs::create_dir(&path).map_err(|e| io(&path, e))?;
        Ok(Self { path, armed: true })
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for StagingDir {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

struct InstallLock {
    file: File,
}
impl InstallLock {
    async fn acquire(path: PathBuf, context: &Option<TaskContext>) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| io(&path, e))?;
        loop {
            check_context(context)?;
            match fs2::FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(Self { file }),
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
                {
                    tokio::time::sleep(Duration::from_millis(100)).await
                }
                Err(e) => return Err(io(&path, e)),
            }
        }
    }
}
impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha1::Digest;
    use std::sync::Arc;
    #[cfg(unix)]
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn rejects_traversal_and_component_escape() {
        assert!(validate_relative("../../outside").is_err());
        assert!(validate_relative("/absolute").is_err());
        assert!(safe_component("java-runtime/escape").is_err());
        let root = Path::new("/tmp/runtime");
        assert!(create_safe_link(root, &root.join("bin/java"), "../../outside").is_err());
    }

    #[test]
    fn runtime_executable_location_matches_platform() {
        let path = java_executable(Path::new("runtime"));
        #[cfg(target_os = "windows")]
        assert!(path.ends_with("bin/java.exe"));
        #[cfg(not(target_os = "windows"))]
        assert!(path.ends_with("bin/java"));
    }

    #[test]
    fn platform_mapping_uses_mojang_catalog_keys() {
        #[cfg(target_os = "linux")]
        assert_eq!(platform_key().unwrap(), "linux");
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert_eq!(platform_key().unwrap(), "mac-os-arm64");
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        assert_eq!(platform_key().unwrap(), "mac-os");
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        assert_eq!(platform_key().unwrap(), "windows-x64");
        assert!(architecture_ok(std::env::consts::ARCH));
    }

    #[test]
    fn integrity_marker_checks_every_manifest_file() {
        let dir = std::env::temp_dir().join(format!(
            "ccl-runtime-integrity-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join("bin/lib.jar"), b"valid runtime file").unwrap();
        let hash = format!("{:x}", sha1::Sha1::digest(b"valid runtime file"));
        fs::write(
            dir.join(".ccl-integrity.json"),
            serde_json::to_vec(&RuntimeIntegrity {
                files: vec![IntegrityEntry {
                    path: "bin/lib.jar".into(),
                    kind: "file".into(),
                    sha1: Some(hash),
                    size: Some(18),
                    target: None,
                }],
            })
            .unwrap(),
        )
        .unwrap();
        assert!(verify_integrity(&dir).is_ok());
        fs::write(dir.join("bin/lib.jar"), b"corrupt").unwrap();
        assert!(verify_integrity(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn pre_cancelled_install_returns_cancelled_without_creating_cache() {
        let dir = std::env::temp_dir().join(format!(
            "ccl-runtime-cancel-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let token = crate::CancellationToken::default();
        token.cancel();
        let context = TaskContext::new(token, Arc::new(|_| {}));
        let result = ensure_runtime_with_catalog(
            &dir,
            "java-runtime-beta",
            17,
            &DownloadConfig::default(),
            Some(context),
            "http://127.0.0.1:1/catalog",
        )
        .await;
        assert!(matches!(result, Err(RuntimeError::Cancelled)));
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn offline_runtime_install_reads_saved_catalog_without_http() {
        let dir = std::env::temp_dir().join(format!(
            "ccl-runtime-offline-cache-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let component = "fixture-runtime";
        let platform = platform_key().unwrap();
        let runtime_root = dir.join(".ccl/runtimes");
        fs::create_dir_all(&runtime_root).unwrap();
        let catalog_path = runtime_root.join(format!(".catalog-{platform}-{component}.json"));
        fs::write(
            &catalog_path,
            serde_json::json!({
                (platform): {(component): [{
                    "manifest":{"url":"http://127.0.0.1:1/manifest.json","sha1":"0123456789012345678901234567890123456789","size":10},
                    "version":{"name":"17.0.1"}
                }]}
            })
            .to_string(),
        )
        .unwrap();
        let config = DownloadConfig {
            offline: true,
            ..DownloadConfig::default()
        };
        let result = ensure_runtime_with_catalog(
            &dir,
            component,
            17,
            &config,
            None,
            "http://127.0.0.1:1/catalog-must-not-be-requested",
        )
        .await;
        assert!(matches!(
            result,
            Err(RuntimeError::Download(
                crate::download::DownloadError::OfflineCacheMiss(_)
            ))
        ));
        assert!(catalog_path.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parses_platform_component_catalog_shape_and_filters_by_major() {
        let data = br#"{"linux-x64":{"java-runtime-beta":[{"manifest":{"url":"http://localhost/m.json","sha1":"00","size":0},"version":{"name":"11.0.2"}},{"manifest":{"url":"http://localhost/m2.json","sha1":"00","size":0},"version":{"name":"17.0.1+2"}}]}}"#;
        let catalog: BTreeMap<String, BTreeMap<String, Vec<CatalogEntry>>> =
            serde_json::from_slice(data).unwrap();
        let selected = catalog
            .get("linux-x64")
            .unwrap()
            .get("java-runtime-beta")
            .unwrap()
            .iter()
            .find(|v| version_major(&v.version.name) == 17)
            .unwrap();
        assert_eq!(selected.version.name, "17.0.1+2");
        assert_eq!(version_major("1.8.0_402"), 8);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installs_direct_files_then_reuses_probed_runtime() {
        let file = format!(
            "#!/bin/sh\necho '    java.version = 999.0.1' >&2\necho '    java.specification.version = 999' >&2\necho '    os.arch = {}' >&2\n",
            std::env::consts::ARCH
        );
        let sha = format!("{:x}", sha1::Sha1::digest(file.as_bytes()));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let manifest = serde_json::json!({"files": {
            "bin": {"type":"directory"},
            "bin/java": {"type":"file", "executable":true, "downloads":{"raw":{"url":format!("http://{addr}/java"),"sha1":sha,"size":file.len()}}}
        }});
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let manifest_sha = format!("{:x}", sha1::Sha1::digest(&manifest_bytes));
        let server_state = Arc::new((manifest_bytes, file.into_bytes(), manifest_sha));
        let artifact_requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let artifact_counter = artifact_requests.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let server = thread::spawn(move || {
            loop {
                let (mut socket, _) = listener.accept().unwrap();
                let mut req = [0u8; 4096];
                let n = socket.read(&mut req).unwrap();
                let request = String::from_utf8_lossy(&req[..n]);
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                if path == "/__stop" {
                    break;
                }
                let body = match path {
                    "/catalog" => serde_json::to_vec(&serde_json::json!({(platform_key().unwrap()):{"test-runtime":[{"manifest":{"url":format!("http://{addr}/manifest"),"sha1":server_state.2,"size":server_state.0.len()},"version":{"name":"999.0.1"}}]}})).unwrap(),
                    "/manifest" => server_state.0.clone(),
                    "/java" => { artifact_counter.fetch_add(1,std::sync::atomic::Ordering::SeqCst); tx.send(()).unwrap(); server_state.1.clone() },
                    _ => b"not found".to_vec(),
                };
                let status = if path == "/" {
                    "404 Not Found"
                } else {
                    "200 OK"
                };
                write!(
                    socket,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                socket.write_all(&body).unwrap();
            }
        });
        let dir = std::env::temp_dir().join(format!(
            "ccl-runtime-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&dir).unwrap();
        let local_config = DownloadConfig {
            version_list_source: crate::config::SourceMode::Official,
            file_source: crate::config::SourceMode::Official,
            ..DownloadConfig::default()
        };
        let catalog_url = format!("http://{addr}/catalog");
        // Configure fixture's manifest artifact URL with the server address by
        // preparing the variant that the mock is serving.
        let result = ensure_runtime_with_catalog(
            &dir,
            "test-runtime",
            999,
            &local_config,
            None,
            &catalog_url,
        )
        .await
        .unwrap();
        assert_eq!(result.major, 999);
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), ());
        let reused = ensure_runtime_with_catalog(
            &dir,
            "test-runtime",
            999,
            &local_config,
            None,
            &catalog_url,
        )
        .await
        .unwrap();
        assert_eq!(reused.major, 999);
        assert_eq!(
            artifact_requests.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        let client_bytes = b"jar";
        let client_sha = format!("{:x}", sha1::Sha1::digest(client_bytes));
        let version_dir = dir.join("versions/runtime-cache-test");
        fs::create_dir_all(&version_dir).unwrap();
        fs::write(
            version_dir.join("runtime-cache-test.json"),
            serde_json::json!({
                "id":"runtime-cache-test",
                "mainClass":"example.Main",
                "javaVersion":{"component":"test-runtime","majorVersion":999},
                "downloads":{"client":{"sha1":client_sha,"size":client_bytes.len()}}
            })
            .to_string(),
        )
        .unwrap();
        fs::write(version_dir.join("runtime-cache-test.jar"), client_bytes).unwrap();
        let service = crate::launch::LaunchService::new(local_config.clone());
        let request = crate::launch::LaunchRequest {
            game_directory: dir.clone(),
            working_directory: None,
            version: "runtime-cache-test".into(),
            profile_id: None,
            username: "RuntimeCacheTest".into(),
            java: None,
            memory: crate::memory::MemoryPolicy::Manual(1024),
            identity: None,
            auto_install_java: false,
        };
        let prepared = service.prepare(&request, None).await.unwrap();
        assert_eq!(prepared.runtime.major, 999);
        assert_eq!(
            prepared.runtime.executable,
            fs::canonicalize(dir.join(format!(
                ".ccl/runtimes/test-runtime-{}/bin/java",
                platform_key().unwrap()
            )))
            .unwrap()
        );
        drop(prepared);
        let java_path = dir.join(format!(
            ".ccl/runtimes/test-runtime-{}/bin/java",
            platform_key().unwrap()
        ));
        fs::write(&java_path, b"corrupt").unwrap();
        let repaired = ensure_runtime_with_catalog(
            &dir,
            "test-runtime",
            999,
            &local_config,
            None,
            &catalog_url,
        )
        .await
        .unwrap();
        assert_eq!(repaired.major, 999);
        assert_eq!(
            artifact_requests.load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        let mut stop = std::net::TcpStream::connect(addr).unwrap();
        write!(stop, "GET /__stop HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        server.join().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
}
