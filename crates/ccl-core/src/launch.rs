//! Installed-version selection, launch preparation, process ownership, and logs.
pub use crate::launch_plan::LaunchPlan;
use crate::{
    TaskContext, TaskEvent,
    config::DownloadConfig,
    download::{DownloadManager, DownloadRequest},
    java::{self, JavaRuntime},
    launch_plan::{self, LaunchPlatform, OfflineAccount, PlanOptions},
    metadata::{self, VersionMetadata},
    natives,
    source::{DownloadKind, url_candidates},
};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncBufReadExt, BufReader};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("file operation failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid installation: {0}")]
    Invalid(String),
    #[error(transparent)]
    Metadata(#[from] metadata::MetadataError),
    #[error(transparent)]
    Java(#[from] java::JavaError),
    #[error(transparent)]
    Plan(#[from] launch_plan::PlanError),
    #[error(transparent)]
    Native(#[from] natives::NativeError),
    #[error(transparent)]
    Download(#[from] crate::download::DownloadError),
    #[error("launch cancelled")]
    Cancelled,
    #[error("process worker failed: {0}")]
    Worker(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct InstalledVersion {
    pub id: String,
    pub version_type: Option<String>,
    pub required_java: u32,
}

#[derive(Debug, Clone)]
pub struct LaunchRequest {
    pub game_directory: PathBuf,
    pub version: String,
    pub username: String,
    pub java: Option<PathBuf>,
    pub max_memory_mb: u32,
}

#[derive(Debug, Serialize)]
pub struct PreparedLaunch {
    pub plan: LaunchPlan,
    pub runtime: JavaRuntime,
    pub version: String,
    pub natives_directory: PathBuf,
}
impl Drop for PreparedLaunch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.natives_directory);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchResult {
    pub run_id: String,
    pub version: String,
    pub pid: u32,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub log_path: PathBuf,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
}

pub fn list_installed_versions(root: &Path) -> Result<Vec<InstalledVersion>, LaunchError> {
    let directory = root.join("versions");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut versions = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|source| io(&directory, source))? {
        let entry = entry.map_err(|source| io(&directory, source))?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if !entry.path().is_dir() || safe_segment(&id).is_err() {
            continue;
        }
        let path = entry.path().join(format!("{id}.json"));
        if !path.is_file() {
            continue;
        }
        let resolved = load_installed_metadata(root, &id)?;
        versions.push(InstalledVersion {
            id,
            version_type: resolved.version_type.clone(),
            required_java: required_java(&resolved),
        });
    }
    versions.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(versions)
}

pub fn load_installed_metadata(root: &Path, version: &str) -> Result<VersionMetadata, LaunchError> {
    let mut all = HashMap::new();
    let mut seen = HashSet::new();
    let mut id = version.to_owned();
    loop {
        safe_segment(&id)?;
        if !seen.insert(id.clone()) || seen.len() > 32 {
            return Err(LaunchError::Invalid(
                "version inheritance cycle or excessive depth".into(),
            ));
        }
        let path = root.join("versions").join(&id).join(format!("{id}.json"));
        let text = fs::read_to_string(&path).map_err(|source| io(&path, source))?;
        let document = VersionMetadata::parse(&text)?;
        if document.id.as_deref() != Some(id.as_str()) {
            return Err(LaunchError::Invalid(format!(
                "metadata ID does not match {id}"
            )));
        }
        let parent = document.inherits_from.clone();
        all.insert(id, document);
        match parent {
            Some(parent) => id = parent,
            None => break,
        }
    }
    Ok(metadata::resolve_inheritance(version, &all)?)
}

fn required_java(metadata: &VersionMetadata) -> u32 {
    metadata
        .java_version
        .as_ref()
        .and_then(|v| v.major_version)
        .unwrap_or(8)
}

#[derive(Clone)]
pub struct LaunchService {
    config: DownloadConfig,
}
impl LaunchService {
    pub fn new(config: DownloadConfig) -> Self {
        Self { config }
    }

    pub async fn prepare(
        &self,
        request: &LaunchRequest,
        context: Option<TaskContext>,
    ) -> Result<PreparedLaunch, LaunchError> {
        check_cancelled(&context)?;
        safe_segment(&request.version)?;
        if !(256..=65536).contains(&request.max_memory_mb) {
            return Err(LaunchError::Invalid(
                "memory must be in 256..=65536 MiB".into(),
            ));
        }
        let account = OfflineAccount::new(&request.username)?;
        let root = fs::canonicalize(&request.game_directory)
            .map_err(|source| io(&request.game_directory, source))?;
        let metadata = load_installed_metadata(&root, &request.version)?;
        let runtime = java::detect_java(request.java.as_deref(), required_java(&metadata))?;
        let platform = LaunchPlatform::current();
        if !same_architecture(&runtime.architecture, &platform.arch) {
            return Err(LaunchError::Invalid(format!(
                "Java architecture {} does not match host {}",
                runtime.architecture, platform.arch
            )));
        }
        verify_installation(&root, &request.version, &metadata, &platform)?;
        let logging_argument = self.prepare_logging(&root, &metadata, &context).await?;
        prepare_asset_projection(&root, &metadata)?;
        check_cancelled(&context)?;
        let parent = root.join(".ccl/natives");
        fs::create_dir_all(&parent).map_err(|source| io(&parent, source))?;
        let natives_directory = parent.join(unique_id());
        natives::extract_natives(&metadata, &root, &natives_directory, &platform)?;
        if let Err(error) = check_cancelled(&context) {
            let _ = fs::remove_dir_all(&natives_directory);
            return Err(error);
        }
        let plan_result = launch_plan::build_launch_plan(
            &metadata,
            &PlanOptions {
                game_directory: root,
                version: request.version.clone(),
                natives_directory: natives_directory.clone(),
                java_executable: runtime.executable.clone(),
                max_memory_mb: request.max_memory_mb,
                account,
                platform,
                logging_argument,
            },
        );
        match plan_result {
            Ok(plan) => Ok(PreparedLaunch {
                plan,
                runtime,
                version: request.version.clone(),
                natives_directory,
            }),
            Err(error) => {
                let _ = fs::remove_dir_all(&natives_directory);
                Err(error.into())
            }
        }
    }

    async fn prepare_logging(
        &self,
        root: &Path,
        metadata: &VersionMetadata,
        context: &Option<TaskContext>,
    ) -> Result<Option<String>, LaunchError> {
        let Some(client) = metadata.logging.as_ref().and_then(|l| l.get("client")) else {
            return Ok(None);
        };
        let info: metadata::DownloadInfo = serde_json::from_value(
            client
                .get("file")
                .cloned()
                .ok_or_else(|| LaunchError::Invalid("logging configuration has no file".into()))?,
        )
        .map_err(metadata::MetadataError::from)?;
        let id = info
            .id
            .as_deref()
            .ok_or_else(|| LaunchError::Invalid("logging file has no ID".into()))?;
        safe_segment(id)?;
        let url = info
            .url
            .as_deref()
            .ok_or_else(|| LaunchError::Invalid("logging file has no URL".into()))?;
        let hash = info
            .sha1
            .as_deref()
            .ok_or_else(|| LaunchError::Invalid("logging file has no SHA-1".into()))?;
        let path = root.join("assets/log_configs").join(id);
        let mut download =
            DownloadRequest::new(url_candidates(url, DownloadKind::File, &self.config), &path)
                .with_sha1(hash);
        if let Some(size) = info.size {
            download = download.with_size(size);
        }
        if let Some(context) = context {
            download = download.with_task_context(context.clone());
        }
        DownloadManager::new(&self.config)?
            .download(download)
            .await?;
        let argument = client
            .get("argument")
            .and_then(|v| v.as_str())
            .ok_or_else(|| LaunchError::Invalid("logging configuration has no argument".into()))?;
        Ok(Some(
            argument.replace("${path}", &launch_plan::path_text(&path)),
        ))
    }

    pub async fn launch(
        &self,
        request: &LaunchRequest,
        context: Option<TaskContext>,
    ) -> Result<LaunchResult, LaunchError> {
        let prepared = self.prepare(request, context.clone()).await?;
        run_process(
            &prepared.plan,
            &prepared.version,
            &request.game_directory.join(".ccl/logs"),
            context,
        )
        .await
    }
}

fn verify_installation(
    root: &Path,
    version: &str,
    metadata: &VersionMetadata,
    platform: &LaunchPlatform,
) -> Result<(), LaunchError> {
    let client = metadata
        .client()
        .ok_or_else(|| LaunchError::Invalid("version has no client".into()))?;
    verify_file(
        &root
            .join("versions")
            .join(version)
            .join(format!("{version}.jar")),
        client.sha1.as_deref(),
        client.size,
    )?;
    let features = HashMap::new();
    let context = metadata::RuleContext {
        os: &platform.os,
        arch: &platform.arch,
        os_version: &platform.os_version,
        features: &features,
    };
    for library in &metadata.libraries {
        if !metadata::library_allowed(&library.rules, &context) {
            continue;
        }
        if let Some(artifact) = &library.downloads.artifact {
            let path = safe_relative(artifact.path.as_deref().ok_or_else(|| {
                LaunchError::Invalid(format!("library {} has no path", library.name))
            })?)?;
            verify_file(
                &root.join("libraries").join(path),
                artifact.sha1.as_deref(),
                artifact.size,
            )?;
        }
    }
    if let Some(index) = &metadata.asset_index {
        let id = index
            .id
            .as_deref()
            .ok_or_else(|| LaunchError::Invalid("asset index has no ID".into()))?;
        safe_segment(id)?;
        verify_file(
            &root.join("assets/indexes").join(format!("{id}.json")),
            index.sha1.as_deref(),
            index.size,
        )?;
    }
    Ok(())
}

fn verify_file(path: &Path, hash: Option<&str>, size: Option<u64>) -> Result<(), LaunchError> {
    let mut file = File::open(path).map_err(|source| io(path, source))?;
    if let Some(size) = size
        && file.metadata().map_err(|source| io(path, source))?.len() != size
    {
        return Err(LaunchError::Invalid(format!(
            "damaged file (size mismatch): {}; run install again",
            path.display()
        )));
    }
    if let Some(hash) = hash {
        let mut digest = Sha1::new();
        let mut buffer = [0; 65536];
        loop {
            let read = file.read(&mut buffer).map_err(|source| io(path, source))?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        if !format!("{:x}", digest.finalize()).eq_ignore_ascii_case(hash) {
            return Err(LaunchError::Invalid(format!(
                "damaged file (SHA-1 mismatch): {}; run install again",
                path.display()
            )));
        }
    }
    Ok(())
}

fn prepare_asset_projection(root: &Path, metadata: &VersionMetadata) -> Result<(), LaunchError> {
    let Some(index) = &metadata.asset_index else {
        return Ok(());
    };
    let id = index
        .id
        .as_deref()
        .ok_or_else(|| LaunchError::Invalid("asset index has no ID".into()))?;
    safe_segment(id)?;
    let index_path = root.join("assets/indexes").join(format!("{id}.json"));
    let index_data = fs::read(&index_path).map_err(|source| io(&index_path, source))?;
    let value: serde_json::Value =
        serde_json::from_slice(&index_data).map_err(metadata::MetadataError::from)?;
    let mut targets = Vec::new();
    if value["virtual"].as_bool() == Some(true) {
        targets.push(root.join("assets/virtual").join(id));
    }
    if value["map_to_resources"].as_bool() == Some(true) {
        targets.push(root.join("resources"));
    }
    if targets.is_empty() {
        return Ok(());
    }
    let objects = value["objects"]
        .as_object()
        .ok_or_else(|| LaunchError::Invalid("asset index has no objects".into()))?;
    for (name, object) in objects {
        let relative = safe_relative(name)?;
        let hash = object["hash"]
            .as_str()
            .filter(|hash| hash.len() == 40 && hash.bytes().all(|c| c.is_ascii_hexdigit()))
            .ok_or_else(|| LaunchError::Invalid("invalid asset hash".into()))?;
        let source = root.join("assets/objects").join(&hash[..2]).join(hash);
        verify_file(&source, Some(hash), object["size"].as_u64())?;
        for target in &targets {
            let path = target.join(&relative);
            if path.is_file() && verify_file(&path, Some(hash), object["size"].as_u64()).is_ok() {
                continue;
            }
            let parent = path.parent().unwrap();
            fs::create_dir_all(parent).map_err(|source| io(parent, source))?;
            let temp = parent.join(format!(".ccl-{}", unique_id()));
            fs::copy(&source, &temp).map_err(|source| io(&temp, source))?;
            if let Err(source) = fs::rename(&temp, &path) {
                let _ = fs::remove_file(&temp);
                return Err(io(&path, source));
            }
        }
    }
    Ok(())
}

/// Execute exactly the prepared argument vector. Game output never goes to stdout
/// implicitly; clients receive task events and a durable combined log.
pub async fn run_process(
    plan: &LaunchPlan,
    version: &str,
    log_directory: &Path,
    context: Option<TaskContext>,
) -> Result<LaunchResult, LaunchError> {
    check_cancelled(&context)?;
    fs::create_dir_all(log_directory).map_err(|source| io(log_directory, source))?;
    let log_directory =
        fs::canonicalize(log_directory).map_err(|source| io(log_directory, source))?;
    let run_id = unique_id();
    let log_path = log_directory.join(format!("{run_id}.log"));
    let log = File::create(&log_path).map_err(|source| io(&log_path, source))?;
    let log = Arc::new(Mutex::new(log));
    let started_at_ms = now_ms();
    let mut command = tokio::process::Command::new(&plan.executable);
    command
        .args(&plan.arguments)
        .current_dir(&plan.working_directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|source| io(&plan.executable, source))?;
    let pid = child
        .id()
        .ok_or_else(|| LaunchError::Worker("child has no PID".into()))?;
    emit(&context, TaskEvent::ProcessStarted { pid });
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out_worker = tokio::spawn(capture(stdout, "stdout", log.clone(), context.clone()));
    let err_worker = tokio::spawn(capture(stderr, "stderr", log.clone(), context.clone()));
    let mut cancelled = false;
    let status = loop {
        tokio::select! {
            status = child.wait() => break status.map_err(|source| io(&plan.executable, source))?,
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                if context.as_ref().is_some_and(|ctx| ctx.cancellation.is_cancelled()) {
                    cancelled = true;
                    let _ = child.start_kill();
                    break child.wait().await.map_err(|source| io(&plan.executable, source))?;
                }
            }
        }
    };
    for worker in [out_worker, err_worker] {
        worker
            .await
            .map_err(|error| LaunchError::Worker(error.to_string()))?
            .map_err(|source| io(&log_path, source))?;
    }
    log.lock()
        .map_err(|_| LaunchError::Worker("log lock poisoned".into()))?
        .sync_all()
        .map_err(|source| io(&log_path, source))?;
    let result = LaunchResult {
        run_id: run_id.clone(),
        version: version.into(),
        pid,
        exit_code: status.code(),
        cancelled,
        log_path,
        started_at_ms,
        finished_at_ms: now_ms(),
    };
    let record_path = log_directory.join(format!("{run_id}.json"));
    let bytes = serde_json::to_vec_pretty(&result).map_err(metadata::MetadataError::from)?;
    fs::write(&record_path, bytes).map_err(|source| io(&record_path, source))?;
    emit(
        &context,
        TaskEvent::ProcessExited {
            code: result.exit_code,
        },
    );
    Ok(result)
}

async fn capture<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    stream: &'static str,
    log: Arc<Mutex<File>>,
    context: Option<TaskContext>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line).await?;
        if read == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        {
            let mut file = log
                .lock()
                .map_err(|_| std::io::Error::other("log lock poisoned"))?;
            write!(file, "[{stream}] {text}")?;
            if !text.ends_with('\n') {
                writeln!(file)?;
            }
        }
        emit(
            &context,
            TaskEvent::Log {
                stream: stream.into(),
                line: text.trim_end_matches(['\r', '\n']).into(),
            },
        );
    }
    Ok(())
}

pub fn read_logs(
    root: &Path,
    run_id: Option<&str>,
    lines: usize,
) -> Result<(String, String), LaunchError> {
    let directory = root.join(".ccl/logs");
    let id = if let Some(id) = run_id {
        safe_segment(id)?;
        id.to_owned()
    } else {
        let mut names = Vec::new();
        for entry in fs::read_dir(&directory).map_err(|source| io(&directory, source))? {
            let path = entry.map_err(|source| io(&directory, source))?.path();
            if path.extension().is_some_and(|extension| extension == "log") {
                names.push(path.file_stem().unwrap().to_string_lossy().into_owned());
            }
        }
        names.sort();
        names
            .pop()
            .ok_or_else(|| LaunchError::Invalid("no launch logs found".into()))?
    };
    let path = directory.join(format!("{id}.log"));
    let file = File::open(&path).map_err(|source| io(&path, source))?;
    let mut tail = std::collections::VecDeque::new();
    for line in std::io::BufRead::lines(std::io::BufReader::new(file)) {
        let line = line.map_err(|source| io(&path, source))?;
        if lines > 0 {
            tail.push_back(line);
            if tail.len() > lines {
                tail.pop_front();
            }
        }
    }
    let tail = tail.into_iter().collect::<Vec<_>>().join("\n");
    Ok((id, tail))
}

fn same_architecture(a: &str, b: &str) -> bool {
    let normalize = |arch: &str| match arch {
        "amd64" | "x86_64" => "x64",
        "arm64" | "aarch64" => "arm64",
        "i386" | "i686" | "x86" => "x86",
        _ => "unknown",
    };
    normalize(a) == normalize(b) && normalize(a) != "unknown"
}
fn safe_segment(segment: &str) -> Result<(), LaunchError> {
    if segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.contains(['/', '\\', ':'])
        || segment.chars().any(char::is_control)
    {
        return Err(LaunchError::Invalid(format!(
            "unsafe path component: {segment}"
        )));
    }
    Ok(())
}
fn safe_relative(path: &str) -> Result<PathBuf, LaunchError> {
    if path.is_empty()
        || path.contains(['\\', ':'])
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(LaunchError::Invalid(format!(
            "unsafe relative path: {path}"
        )));
    }
    Ok(path.into())
}
fn check_cancelled(context: &Option<TaskContext>) -> Result<(), LaunchError> {
    if context
        .as_ref()
        .is_some_and(|ctx| ctx.cancellation.is_cancelled())
    {
        Err(LaunchError::Cancelled)
    } else {
        Ok(())
    }
}
fn emit(context: &Option<TaskContext>, event: TaskEvent) {
    if let Some(context) = context {
        let _ = context.emit(event);
    }
}
fn io(path: &Path, source: std::io::Error) -> LaunchError {
    LaunchError::Io {
        path: path.into(),
        source,
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn unique_id() -> String {
    format!(
        "{:013}-{}-{:06}",
        now_ms(),
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!("ccl-prepare-{}", unique_id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn loads_local_inheritance_and_rejects_mismatched_ids() {
        let root = root();
        for (id, document) in [
            (
                "parent",
                json!({"id":"parent","mainClass":"Parent","javaVersion":{"majorVersion":17}}),
            ),
            (
                "child",
                json!({"id":"child","inheritsFrom":"parent","mainClass":"Child"}),
            ),
        ] {
            let path = root.join("versions").join(id);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join(format!("{id}.json")), document.to_string()).unwrap();
        }
        let child = load_installed_metadata(&root, "child").unwrap();
        assert_eq!(child.main_class.as_deref(), Some("Child"));
        assert_eq!(required_java(&child), 17);
        fs::write(
            root.join("versions/child/child.json"),
            r#"{"id":"different"}"#,
        )
        .unwrap();
        assert!(matches!(
            load_installed_metadata(&root, "child"),
            Err(LaunchError::Invalid(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn projects_verified_assets_and_repairs_damage_without_accepting_traversal() {
        let root = root();
        let hash = format!("{:x}", Sha1::digest(b"asset"));
        let source = root.join("assets/objects").join(&hash[..2]).join(&hash);
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"asset").unwrap();
        let index_path = root.join("assets/indexes/legacy.json");
        fs::create_dir_all(index_path.parent().unwrap()).unwrap();
        let index = json!({"virtual":true,"map_to_resources":true,"objects":{"sound/test.ogg":{"hash":hash,"size":5}}});
        fs::write(&index_path, index.to_string()).unwrap();
        let metadata =
            VersionMetadata::parse(r#"{"id":"v","assetIndex":{"id":"legacy"}}"#).unwrap();
        prepare_asset_projection(&root, &metadata).unwrap();
        let projected = root.join("resources/sound/test.ogg");
        assert_eq!(fs::read(&projected).unwrap(), b"asset");
        fs::write(&projected, b"broken").unwrap();
        prepare_asset_projection(&root, &metadata).unwrap();
        assert_eq!(fs::read(&projected).unwrap(), b"asset");
        let bad = json!({"virtual":true,"objects":{"../../escape":{"hash":hash,"size":5}}});
        fs::write(&index_path, bad.to_string()).unwrap();
        assert!(matches!(
            prepare_asset_projection(&root, &metadata),
            Err(LaunchError::Invalid(_))
        ));
        fs::write(&source, b"bad").unwrap();
        assert!(verify_file(&source, Some(&hash), Some(5)).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
