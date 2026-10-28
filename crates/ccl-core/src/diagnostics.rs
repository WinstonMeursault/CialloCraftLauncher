//! Bounded, local diagnostics for launcher health and saved game runs.

use crate::java;
use regex::Regex;
use serde::Serialize;
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_RUN_LOG_BYTES: u64 = 256 * 1024;
const MAX_RAW_PREVIEW_BYTES: usize = 16 * 1024;
const MAX_RUN_RECORD_BYTES: usize = 64 * 1024;
const MAX_CRASH_REPORT_BYTES: usize = 64 * 1024;
const MAX_CORRELATED_CRASH_REPORTS: usize = 4;

#[derive(Debug, Clone)]
pub struct DiagnosticsService {
    game_directory: PathBuf,
    launcher_data_directory: PathBuf,
    log_root: PathBuf,
    profile: Option<(String, String)>,
}

impl DiagnosticsService {
    pub fn new(
        game_directory: impl Into<PathBuf>,
        launcher_data_directory: impl Into<PathBuf>,
    ) -> Self {
        let game_directory = game_directory.into();
        Self {
            log_root: game_directory.clone(),
            game_directory,
            launcher_data_directory: launcher_data_directory.into(),
            profile: None,
        }
    }

    /// Locate saved launch records separately from the runtime's crash reports.
    pub fn with_log_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.log_root = root.into();
        self
    }

    /// Restrict explicit run diagnostics and exports to a selected profile.
    pub fn with_profile(
        mut self,
        id: impl Into<String>,
        legacy_version: impl Into<String>,
    ) -> Self {
        self.profile = Some((id.into(), legacy_version.into()));
        self
    }

    pub fn doctor(&self, options: &DoctorOptions) -> DoctorReport {
        let java_paths = options
            .java
            .as_deref()
            .map(|p| vec![p.to_path_buf()])
            .unwrap_or_else(java::discover_java);
        let mut runtimes = Vec::new();
        // Each probe has its own five-second process timeout; cap the total
        // doctor wait to two candidates even if PATH discovery is very large.
        for path in java_paths.into_iter().take(2) {
            if let Ok(runtime) = java::inspect_java(&path) {
                runtimes.push(JavaCheck {
                    major: runtime.major,
                    architecture: runtime.architecture,
                    compatible: options
                        .required_java_major
                        .is_none_or(|v| v == runtime.major),
                });
            }
        }
        let platform = std::env::consts::OS.to_owned();
        let game = directory_check(&self.game_directory);
        let launcher_data = directory_check(&self.launcher_data_directory);
        let credential_store = CredentialCapability {
            supported: cfg!(any(
                target_os = "windows",
                target_os = "macos",
                target_os = "linux"
            )),
            probe_performed: false,
            connectivity: "unknown",
        };
        let network = options
            .network_url
            .as_deref()
            .map(|url| probe_network(url, options.network_timeout, options.http_proxy.as_deref()));
        let write_probe = options
            .write_probe
            .then(|| probe_write(&self.launcher_data_directory));
        let mut launcher_data = launcher_data;
        if let Some(probe) = &write_probe {
            launcher_data = directory_check(&self.launcher_data_directory);
            launcher_data.writable = Some(probe.success);
        }
        DoctorReport {
            platform,
            java: runtimes,
            game_directory: game,
            launcher_data_directory: launcher_data,
            credential_store,
            network,
            write_probe,
        }
    }

    pub fn diagnose_run(&self, run_id: &str) -> Result<RunDiagnosis, DiagnosticError> {
        if !valid_run_id(run_id) {
            return Err(DiagnosticError::InvalidRunId);
        }
        let record: RunRecord = serde_json::from_slice(&read_run_artifact(
            &self.log_root,
            run_id,
            "json",
            MAX_RUN_RECORD_BYTES,
        )?)
        .map_err(|_| DiagnosticError::InvalidRunRecord)?;
        if record.run_id != run_id {
            return Err(DiagnosticError::InvalidRunRecord);
        }
        if let Some((id, legacy)) = &self.profile
            && !record
                .profile_id
                .as_ref()
                .map_or(record.version.as_ref() == Some(legacy), |profile| {
                    profile == id
                })
        {
            return Err(DiagnosticError::RunProfileMismatch);
        }
        let log = read_run_artifact(&self.log_root, run_id, "log", MAX_RUN_LOG_BYTES as usize)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let crash_reports = correlated_crash_reports(&self.game_directory, &record);
        let mut evidence_text = log;
        for report in &crash_reports {
            evidence_text.extend_from_slice(b"\n");
            evidence_text.extend_from_slice(&report.to_ascii_lowercase());
        }
        let (category, confidence, evidence, suggestions) =
            classify(&record, &evidence_text, crash_reports.len());
        Ok(RunDiagnosis {
            run_id: run_id.to_owned(),
            category,
            confidence,
            exit_code: record.exit_code,
            cancelled: record.cancelled,
            correlated_crash_reports: crash_reports.len(),
            evidence,
            suggestions,
        })
    }

    pub fn export(&self, options: &ExportOptions) -> Result<DiagnosticExport, DiagnosticError> {
        let doctor = self.doctor(&DoctorOptions::default());
        let run = options
            .run_id
            .as_deref()
            .map(|id| self.diagnose_run(id))
            .transpose()?;
        let raw_log_preview = if options.include_raw_log_preview {
            let run_id = options
                .run_id
                .as_deref()
                .ok_or(DiagnosticError::RunRequiredForRawLog)?;
            let bytes = read_run_artifact(&self.log_root, run_id, "log", MAX_RAW_PREVIEW_BYTES)?;
            Some(redact_raw_preview(
                &String::from_utf8_lossy(&bytes),
                &self.game_directory,
                &self.launcher_data_directory,
            ))
        } else {
            None
        };
        let structured = StructuredExport {
            schema_version: 1,
            platform: doctor.platform,
            java_count: doctor.java.len(),
            game_directory_exists: doctor.game_directory.exists,
            launcher_data_directory_exists: doctor.launcher_data_directory.exists,
            credential_store_supported: doctor.credential_store.supported,
            credential_store_connectivity: doctor.credential_store.connectivity,
            run,
        };
        let mut json =
            serde_json::to_value(structured).map_err(|_| DiagnosticError::Serialization)?;
        scrub_json(&mut json);
        Ok(DiagnosticExport {
            structured_json: serde_json::to_string_pretty(&json).map_err(|_| DiagnosticError::Serialization)?,
            raw_log_preview,
            raw_log_preview_notice: options.include_raw_log_preview.then_some("Raw game output is user and mod generated; this bounded preview may contain secrets and has not been comprehensively redacted."),
        })
    }
}

#[derive(Debug, Serialize)]
struct StructuredExport {
    schema_version: u32,
    platform: String,
    java_count: usize,
    game_directory_exists: bool,
    launcher_data_directory_exists: bool,
    credential_store_supported: bool,
    credential_store_connectivity: &'static str,
    run: Option<RunDiagnosis>,
}

#[derive(Debug, Clone)]
pub struct DoctorOptions {
    pub java: Option<PathBuf>,
    pub required_java_major: Option<u32>,
    /// Explicitly requested URL. Requests are bounded to `network_timeout`.
    pub network_url: Option<String>,
    pub http_proxy: Option<String>,
    pub network_timeout: Duration,
    /// Creates, syncs, and removes a uniquely named empty file in launcher data.
    pub write_probe: bool,
}
impl Default for DoctorOptions {
    fn default() -> Self {
        Self {
            java: None,
            required_java_major: None,
            network_url: None,
            http_proxy: None,
            network_timeout: Duration::from_secs(5),
            write_probe: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub platform: String,
    pub java: Vec<JavaCheck>,
    pub game_directory: DirectoryCheck,
    pub launcher_data_directory: DirectoryCheck,
    pub credential_store: CredentialCapability,
    pub network: Option<ProbeResult>,
    pub write_probe: Option<ProbeResult>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaCheck {
    pub major: u32,
    pub architecture: String,
    pub compatible: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct DirectoryCheck {
    pub exists: bool,
    pub directory: bool,
    pub readable: bool,
    pub writable: Option<bool>,
    pub permission_readonly_hint: Option<bool>,
}
#[derive(Debug, Clone, Serialize)]
pub struct CredentialCapability {
    /// True when this target has a compiled OS keyring adapter. This does not
    /// claim that a user session vault is unlocked or reachable.
    pub supported: bool,
    pub probe_performed: bool,
    pub connectivity: &'static str,
}
#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    pub success: bool,
    pub elapsed_ms: u64,
    pub detail: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunDiagnosis {
    pub run_id: String,
    pub category: String,
    pub confidence: f32,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub correlated_crash_reports: usize,
    pub evidence: Vec<String>,
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub run_id: Option<String>,
    pub include_raw_log_preview: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticExport {
    pub structured_json: String,
    pub raw_log_preview: Option<String>,
    pub raw_log_preview_notice: Option<&'static str>,
}

#[derive(Debug, thiserror::Error)]
pub enum DiagnosticError {
    #[error("invalid run identifier")]
    InvalidRunId,
    #[error("saved run record was not found")]
    RunNotFound,
    #[error("saved run record is invalid")]
    InvalidRunRecord,
    #[error("saved run does not belong to the selected profile")]
    RunProfileMismatch,
    #[error("raw log preview requires a run identifier")]
    RunRequiredForRawLog,
    #[error("diagnostic data could not be read")]
    Io(#[from] std::io::Error),
    #[error("diagnostic data could not be serialized")]
    Serialization,
}

#[derive(serde::Deserialize)]
struct RunRecord {
    run_id: String,
    #[serde(default)]
    profile_id: Option<String>,
    #[serde(default)]
    version: Option<String>,
    exit_code: Option<i32>,
    cancelled: bool,
    #[serde(default)]
    started_at_ms: Option<u64>,
    #[serde(default)]
    finished_at_ms: Option<u64>,
}

fn directory_check(path: &Path) -> DirectoryCheck {
    match fs::metadata(path) {
        Ok(m) => DirectoryCheck {
            exists: true,
            directory: m.is_dir(),
            readable: fs::read_dir(path).is_ok(),
            writable: None,
            permission_readonly_hint: Some(m.permissions().readonly()),
        },
        Err(_) => DirectoryCheck {
            exists: false,
            directory: false,
            readable: false,
            writable: None,
            permission_readonly_hint: None,
        },
    }
}
fn probe_network(url: &str, timeout: Duration, proxy: Option<&str>) -> ProbeResult {
    let start = Instant::now();
    let url = url.to_owned();
    let proxy = proxy.map(str::to_owned);
    let timeout = timeout.min(Duration::from_secs(10));
    let result = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()
            .and_then(|rt| {
                rt.block_on(async {
                    let mut builder = reqwest::Client::builder().timeout(timeout);
                    if let Some(proxy) = proxy {
                        builder = builder.proxy(reqwest::Proxy::all(proxy).ok()?);
                    }
                    let client = builder.build().ok()?;
                    let response = client.get(url).send().await.ok()?;
                    Some(response.status().is_success())
                })
            })
    })
    .join()
    .ok()
    .flatten();
    ProbeResult {
        success: result == Some(true),
        elapsed_ms: start.elapsed().as_millis() as u64,
        detail: if result == Some(true) {
            "request_succeeded"
        } else {
            "request_failed"
        },
    }
}
fn probe_write(dir: &Path) -> ProbeResult {
    let start = Instant::now();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = dir.join(format!(
        ".ccl-diagnostic-probe-{}-{nonce}",
        std::process::id()
    ));
    let mut created = false;
    let wrote = if fs::create_dir_all(dir).is_ok() {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                created = true;
                file.write_all(b"").and_then(|_| file.sync_all()).is_ok()
            }
            Err(_) => false,
        }
    } else {
        false
    };
    let cleaned = !created || fs::remove_file(path).is_ok();
    let success = wrote && cleaned;
    ProbeResult {
        success,
        elapsed_ms: start.elapsed().as_millis() as u64,
        detail: if success {
            "write_succeeded"
        } else {
            "write_failed"
        },
    }
}
fn valid_run_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
fn read_prefix(path: &Path, max: usize) -> Result<Vec<u8>, DiagnosticError> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(max as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn read_run_artifact(
    root: &Path,
    run_id: &str,
    extension: &str,
    max: usize,
) -> Result<Vec<u8>, DiagnosticError> {
    let ccl = root.join(".ccl");
    let logs = ccl.join("logs");
    for directory in [&ccl, &logs] {
        let metadata = fs::symlink_metadata(directory).map_err(|_| DiagnosticError::RunNotFound)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(DiagnosticError::RunNotFound);
        }
    }
    let path = logs.join(format!("{run_id}.{extension}"));
    let metadata = fs::symlink_metadata(&path).map_err(|_| DiagnosticError::RunNotFound)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(DiagnosticError::RunNotFound);
    }
    if extension == "json" && metadata.len() > max as u64 {
        return Err(DiagnosticError::InvalidRunRecord);
    }
    if extension == "log" {
        read_tail(&path, max)
    } else {
        read_prefix(&path, max)
    }
}
fn read_tail(path: &Path, max: usize) -> Result<Vec<u8>, DiagnosticError> {
    let mut file = fs::File::open(path)?;
    let start = file.metadata()?.len().saturating_sub(max as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(max as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}
fn correlated_crash_reports(root: &Path, record: &RunRecord) -> Vec<Vec<u8>> {
    let (Some(started), Some(finished)) = (record.started_at_ms, record.finished_at_ms) else {
        return Vec::new();
    };
    if finished < started {
        return Vec::new();
    }
    let directory = root.join("crash-reports");
    let Ok(metadata) = fs::symlink_metadata(&directory) else {
        return Vec::new();
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let lower = started.saturating_sub(30_000);
    let upper = finished.saturating_add(30_000);
    let mut reports = Vec::new();
    for entry in entries.flatten().take(128) {
        let path = entry.path();
        let name_matches = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| {
                name.to_ascii_lowercase().starts_with("crash-")
                    && path
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("txt"))
            });
        if !name_matches {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(modified) = modified.duration_since(UNIX_EPOCH) else {
            continue;
        };
        let timestamp = modified.as_millis().min(u64::MAX as u128) as u64;
        if !(lower..=upper).contains(&timestamp) {
            continue;
        }
        if let Ok(bytes) = read_prefix(&path, MAX_CRASH_REPORT_BYTES) {
            reports.push(bytes);
        }
        if reports.len() >= MAX_CORRELATED_CRASH_REPORTS {
            break;
        }
    }
    reports
}

fn redact_raw_preview(input: &str, game_root: &Path, launcher_root: &Path) -> String {
    let mut text = input.to_owned();
    let mut known_paths = vec![game_root.to_path_buf(), launcher_root.to_path_buf()];
    for key in ["HOME", "USERPROFILE"] {
        if let Some(path) = std::env::var_os(key) {
            known_paths.push(PathBuf::from(path));
        }
    }
    known_paths.sort_by_key(|path| std::cmp::Reverse(path.as_os_str().len()));
    for path in known_paths {
        if let Some(path) = path.to_str().filter(|path| !path.is_empty()) {
            text = text.replace(path, "<redacted-path>");
        }
    }
    for pattern in [
        r"(?i)(?:/Users|/home)/[^/\s]+(?:/[^\s,;)]*)?",
        r"(?i)[A-Z]:\\Users\\[^\\\s]+(?:\\[^\s,;)]*)?",
        r#"(?i)(?:--?)?(?:access[_-]?token|refresh[_-]?token|client[_-]?secret|authorization|password|passwd|proxy[_-]?(?:user|pass(?:word)?)|username|uuid|xuid|auth[_-]?code)\s*(?:=|\s|:)\s*(?:\"[^\"]*\"|'[^']*'|[^\s,;]+)"#,
        r#"(?i)([\w.-]*(?:token|secret|password|passwd|authorization)[\w.-]*\s*[\"']?\s*[:=]\s*[\"']?)[^\s\"'&,;}]+"#,
        r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]+",
        r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b",
    ] {
        if let Ok(regex) = Regex::new(pattern) {
            text = regex.replace_all(&text, "<redacted>").into_owned();
        }
    }
    let url_regex = Regex::new(r#"(?i)\b(?:https?|socks5h?)://[^\s"'<>]+"#).unwrap();
    text = url_regex
        .replace_all(&text, |captures: &regex::Captures<'_>| {
            redact_url(captures.get(0).unwrap().as_str())
        })
        .into_owned();
    for key in ["USER", "USERNAME", "LOGNAME"] {
        if let Ok(value) = std::env::var(key) {
            redact_identifier(&mut text, &value);
        }
    }
    for key in ["HOME", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(key).and_then(|path| {
            PathBuf::from(path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        }) {
            redact_identifier(&mut text, &value);
        }
    }
    if text.len() > MAX_RAW_PREVIEW_BYTES {
        let mut boundary = MAX_RAW_PREVIEW_BYTES;
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        text.truncate(boundary);
    }
    text
}

fn redact_identifier(text: &mut String, identifier: &str) {
    if identifier.chars().count() < 3 {
        return;
    }
    let pattern = format!(
        r"(?i)(^|[^A-Za-z0-9_]){}([^A-Za-z0-9_]|$)",
        regex::escape(identifier)
    );
    if let Ok(regex) = Regex::new(&pattern) {
        *text = regex.replace_all(text, "$1<redacted-user>$2").into_owned();
    }
}

fn redact_url(raw: &str) -> String {
    let trimmed = raw.trim_end_matches([',', ';', '.', ')', ']', '}']);
    let trailing = &raw[trimmed.len()..];
    let clean = reqwest::Url::parse(trimmed)
        .map(|mut url| {
            if !url.username().is_empty() {
                let _ = url.set_username("redacted");
            }
            if url.password().is_some() {
                let _ = url.set_password(Some("redacted"));
            }
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        })
        .unwrap_or_else(|_| trimmed.split('?').next().unwrap_or(trimmed).to_owned());
    format!("{clean}{trailing}")
}

fn classify(
    record: &RunRecord,
    log: &[u8],
    crash_reports: usize,
) -> (String, f32, Vec<String>, Vec<String>) {
    let text = String::from_utf8_lossy(log);
    if record.cancelled {
        return (
            "cancelled".into(),
            1.0,
            vec!["run record marks the process as cancelled".into()],
            vec!["No action is required unless the cancellation was unexpected.".into()],
        );
    }
    for (pattern, category, suggestion) in [
        (
            "outofmemoryerror",
            "memory_exhaustion",
            "Reduce memory-heavy mods or adjust the memory allocation.",
        ),
        (
            "could not create the java virtual machine",
            "java_startup_failure",
            "Check Java compatibility and JVM arguments.",
        ),
        (
            "org.lwjgl.opengl",
            "graphics_initialization",
            "Check graphics drivers and supported OpenGL level.",
        ),
        (
            "nosuchfileexception",
            "missing_file",
            "Repair the instance or reinstall the missing game component.",
        ),
    ] {
        if text.contains(pattern) {
            return (
                category.into(),
                0.92,
                vec![format!(
                    "saved log or correlated crash report contains {pattern}"
                )],
                vec![suggestion.into()],
            );
        }
    }
    if crash_reports > 0 {
        return (
            "crash_report_present".into(),
            0.78,
            vec!["a crash report timestamp matches this run".into()],
            vec!["Review the matching crash report and recent instance changes.".into()],
        );
    }
    if record.exit_code == Some(0) {
        return (
            "normal_exit".into(),
            0.95,
            vec!["run record reports exit code 0".into()],
            vec!["No action is required.".into()],
        );
    }
    if record.exit_code.is_some() {
        return (
            "nonzero_exit_unknown".into(),
            0.55,
            vec!["run record reports a nonzero exit code".into()],
            vec!["Review the bounded raw log preview and recent instance changes.".into()],
        );
    }
    (
        "unknown".into(),
        0.1,
        vec!["saved evidence does not identify a cause".into()],
        vec!["Collect more information and retry.".into()],
    )
}
fn scrub_json(value: &mut serde_json::Value) {
    // The caller can provide a run id; it is not needed in a shareable export.
    if let Some(run) = value
        .get_mut("run")
        .and_then(serde_json::Value::as_object_mut)
    {
        run.remove("run_id");
    }
}
