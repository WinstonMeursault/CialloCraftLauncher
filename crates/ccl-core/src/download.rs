//! Reusable asynchronous artifact downloader.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use sha1::{Digest, Sha1};
use thiserror::Error;
use tokio::sync::Semaphore;

use crate::{TaskContext, TaskEvent};

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("download cancelled")]
    Cancelled,
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("request failed for {url}: {message}")]
    Request { url: String, message: String },
    #[error("server returned HTTP {status} for {url}")]
    HttpStatus { url: String, status: u16 },
    #[error("file operation failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("SHA-1 mismatch for {path}: expected {expected}, got {actual}")]
    Sha1Mismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("SHA-512 mismatch for {path}: expected {expected}, got {actual}")]
    Sha512Mismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("size mismatch for {path}: expected {expected}, got {actual}")]
    SizeMismatch {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },
    #[error("all URL candidates failed: {0}")]
    CandidatesExhausted(String),
    #[error("offline mode: required artifact is not available in the verified local cache: {0}")]
    OfflineCacheMiss(PathBuf),
}

#[derive(Clone)]
pub struct DownloadRequest {
    /// Candidate endpoints in preference order (normally from `source::url_candidates`).
    pub urls: Vec<String>,
    pub destination: PathBuf,
    pub expected_sha1: Option<String>,
    pub expected_sha512: Option<String>,
    pub expected_size: Option<u64>,
    /// When present, require HTTPS and permit only these hosts, including redirects.
    pub allowed_hosts: Option<Vec<String>>,
    require_https: bool,
    /// Optional event and cancellation context shared with the caller.
    pub task_context: Option<TaskContext>,
    sensitive_headers: Vec<ScopedHeader>,
}
#[derive(Clone)]
struct ScopedHeader {
    hosts: Vec<String>,
    value: reqwest::header::HeaderValue,
}
impl ScopedHeader {
    fn applies(&self, url: &reqwest::Url) -> bool {
        url.scheme() == "https"
            && url.host_str().is_some_and(|host| {
                self.hosts
                    .iter()
                    .any(|expected| expected.eq_ignore_ascii_case(host))
            })
    }
}

impl DownloadRequest {
    pub fn new(urls: Vec<String>, destination: impl Into<PathBuf>) -> Self {
        Self {
            urls,
            destination: destination.into(),
            expected_sha1: None,
            expected_sha512: None,
            expected_size: None,
            allowed_hosts: None,
            require_https: false,
            task_context: None,
            sensitive_headers: Vec::new(),
        }
    }

    /// Attach an API key only to these exact HTTPS hosts, including after redirects.
    pub fn with_api_key(mut self, key: &str, hosts: Vec<String>) -> Result<Self, DownloadError> {
        let mut value = reqwest::header::HeaderValue::from_str(key).map_err(|_| {
            DownloadError::InvalidRequest("API key is not a valid HTTP header value".into())
        })?;
        value.set_sensitive(true);
        self.sensitive_headers.push(ScopedHeader { hosts, value });
        Ok(self)
    }
    pub fn with_sha1(mut self, sha1: impl Into<String>) -> Self {
        self.expected_sha1 = Some(sha1.into().to_ascii_lowercase());
        self
    }

    pub fn with_sha512(mut self, sha512: impl Into<String>) -> Self {
        self.expected_sha512 = Some(sha512.into().to_ascii_lowercase());
        self
    }

    /// Require HTTPS for the initial request and all redirects, allowing delegated CDN hosts.
    pub fn with_https(mut self) -> Self {
        self.require_https = true;
        self
    }
    pub fn with_allowed_hosts(mut self, hosts: Vec<String>) -> Self {
        self.allowed_hosts = Some(hosts.into_iter().map(|h| h.to_ascii_lowercase()).collect());
        self
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.expected_size = Some(size);
        self
    }

    pub fn with_task_context(mut self, context: TaskContext) -> Self {
        self.task_context = Some(context);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadOutcome {
    pub path: PathBuf,
    pub bytes: u64,
    pub source_url: Option<String>,
    pub reused_existing: bool,
}

/// Bounds simultaneous transfers across all calls made through this manager.
#[derive(Clone)]
pub struct DownloadManager {
    client: reqwest::Client,
    permits: Arc<Semaphore>,
    retries_per_candidate: usize,
    base_backoff: Duration,
    offline: bool,
}

impl DownloadManager {
    pub fn new(config: &crate::config::DownloadConfig) -> Result<Self, DownloadError> {
        if config.max_parallel == 0 {
            return Err(DownloadError::InvalidRequest(
                "max_parallel must be nonzero".into(),
            ));
        }
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("CialloCraftLauncher/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(30));
        if let Some(proxy) = &config.http_proxy {
            let proxy = reqwest::Proxy::all(proxy).map_err(|_| {
                DownloadError::InvalidRequest(
                    "downloads.http_proxy is invalid; check its scheme and address".into(),
                )
            })?;
            builder = builder.proxy(proxy);
        }
        let client = builder.build().map_err(|_| {
            DownloadError::InvalidRequest(
                "could not initialize HTTP client; check proxy settings".into(),
            )
        })?;
        Ok(Self {
            client,
            permits: Arc::new(Semaphore::new(config.max_parallel)),
            retries_per_candidate: 3,
            base_backoff: Duration::from_millis(250),
            offline: config.offline,
        })
    }

    /// Configure retry count and initial exponential backoff delay.
    pub fn with_retry_policy(
        mut self,
        retries_per_candidate: usize,
        base_backoff: Duration,
    ) -> Self {
        self.retries_per_candidate = retries_per_candidate;
        self.base_backoff = base_backoff;
        self
    }

    pub async fn download(
        &self,
        request: DownloadRequest,
    ) -> Result<DownloadOutcome, DownloadError> {
        if request.urls.is_empty() {
            return Err(DownloadError::InvalidRequest(
                "at least one URL candidate is required".into(),
            ));
        }
        check_cancelled(&request)?;
        let parent = parent_dir(&request.destination);
        fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
        if let Ok(bytes) = fs::read(&request.destination)
            && verify_bytes(&request, &bytes).is_ok()
        {
            return Ok(DownloadOutcome {
                path: request.destination,
                bytes: bytes.len() as u64,
                source_url: None,
                reused_existing: true,
            });
        }

        if self.offline {
            return Err(DownloadError::OfflineCacheMiss(request.destination));
        }

        let _permit = loop {
            check_cancelled(&request)?;
            tokio::select! {
                permit = self.permits.acquire() => break permit.map_err(|_| DownloadError::Cancelled)?,
                _ = tokio::time::sleep(Duration::from_millis(100)) => {},
            }
        };
        let mut failures = Vec::new();
        for url in &request.urls {
            for attempt in 0..=self.retries_per_candidate {
                check_cancelled(&request)?;
                match self.download_once(&request, url).await {
                    Ok(outcome) => return Ok(outcome),
                    Err(error) => {
                        if matches!(error, DownloadError::Cancelled) {
                            return Err(DownloadError::Cancelled);
                        }
                        let retryable = matches!(
                            error,
                            DownloadError::Request { .. }
                                | DownloadError::HttpStatus {
                                    status: 408 | 429 | 500..=599,
                                    ..
                                }
                                | DownloadError::Sha1Mismatch { .. }
                                | DownloadError::Sha512Mismatch { .. }
                                | DownloadError::SizeMismatch { .. }
                        );
                        failures.push(format!("{}: {error}", safe_url_text(url)));
                        if !retryable || attempt == self.retries_per_candidate {
                            break;
                        }
                        let factor = 1u32.checked_shl(attempt.min(16) as u32).unwrap_or(u32::MAX);
                        let mut remaining = self.base_backoff.saturating_mul(factor);
                        while !remaining.is_zero() {
                            check_cancelled(&request)?;
                            let interval = remaining.min(Duration::from_millis(100));
                            tokio::time::sleep(interval).await;
                            remaining = remaining.saturating_sub(interval);
                        }
                    }
                }
            }
        }
        Err(DownloadError::CandidatesExhausted(failures.join("; ")))
    }

    async fn download_once(
        &self,
        request: &DownloadRequest,
        url: &str,
    ) -> Result<DownloadOutcome, DownloadError> {
        if let Some(context) = &request.task_context {
            context
                .emit(TaskEvent::Started {
                    task: format!("download {}", request.destination.display()),
                })
                .map_err(|_| DownloadError::Cancelled)?;
        }
        let mut current_url = reqwest::Url::parse(url)
            .map_err(|_| DownloadError::InvalidRequest("invalid URL candidate".into()))?;
        check_url_policy(request, &current_url)?;
        let partial = PartialFile::for_destination(&request.destination);
        let _partial_lock = partial.acquire(request).await?;
        if let Ok(bytes) = fs::read(&request.destination)
            && verify_bytes(request, &bytes).is_ok()
        {
            return Ok(DownloadOutcome {
                path: request.destination.clone(),
                bytes: bytes.len() as u64,
                source_url: None,
                reused_existing: true,
            });
        }
        let scope = resume_scope(url, request);
        let (mut resume_at, mut saved_etag) = partial.resume_state(&scope)?;
        if request
            .expected_size
            .is_some_and(|expected| resume_at >= expected)
        {
            partial.discard()?;
            resume_at = 0;
            saved_etag = None;
        }
        let mut redirects = 0;
        let mut response = loop {
            let mut builder = self.client.get(current_url.clone());
            for header in &request.sensitive_headers {
                if header.applies(&current_url) {
                    builder = builder.header("x-api-key", header.value.clone());
                }
            }
            if resume_at > 0 {
                builder = builder
                    .header(reqwest::header::RANGE, format!("bytes={resume_at}-"))
                    .header(reqwest::header::IF_RANGE, saved_etag.as_deref().unwrap());
            }
            let send = builder.send();
            tokio::pin!(send);
            let response = loop {
                tokio::select! {
                    result = &mut send => break result.map_err(|e| DownloadError::Request {
                        url: safe_url(&current_url), message: request_error_message(e),
                    })?,
                    _ = tokio::time::sleep(Duration::from_millis(100)) => check_cancelled(request)?,
                }
            };
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| DownloadError::Request {
                        url: safe_url(&current_url),
                        message: "redirect missing a valid Location".into(),
                    })?;
                current_url = next_redirect(request, &current_url, location, redirects)?;
                redirects += 1;
                continue;
            }
            break response;
        };
        if resume_at > 0 && response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            partial.discard()?;
            return Err(DownloadError::Request {
                url: safe_url(&current_url),
                message: "saved range is no longer valid; restarting from zero".into(),
            });
        }
        if !response.status().is_success() {
            return Err(DownloadError::HttpStatus {
                url: safe_url(&current_url),
                status: response.status().as_u16(),
            });
        }
        let resumed = if resume_at > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT
        {
            let etag = response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|v| v.to_str().ok());
            let range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok());
            if etag != saved_etag.as_deref()
                || !range.is_some_and(|v| valid_content_range(v, resume_at, request.expected_size))
            {
                partial.discard()?;
                return Err(DownloadError::Request {
                    url: safe_url(&current_url),
                    message: "range response did not match saved ETag and offset".into(),
                });
            }
            true
        } else {
            if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
                partial.discard()?;
                return Err(DownloadError::Request {
                    url: safe_url(&current_url),
                    message: "unexpected partial response without a matching range request".into(),
                });
            }
            false
        };
        if !resumed {
            partial.discard()?;
            if let Some(etag) = response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|v| v.to_str().ok())
            {
                partial.save_etag(etag, &scope)?;
            }
        }
        let resumable = saved_etag.is_some()
            || response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|value| value.to_str().ok())
                .is_some_and(valid_strong_etag);
        let total = request
            .expected_size
            .or_else(|| {
                response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.rsplit_once('/').and_then(|(_, total)| total.parse().ok()))
            })
            .or_else(|| {
                response
                    .content_length()
                    .map(|n| n + if resumed { resume_at } else { 0 })
            });
        let mut file = OpenOptions::new()
            .read(resumed)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .create_new(!resumed)
            .open(&partial.path)
            .map_err(|e| io_error(&partial.path, e))?;
        let mut hasher = Sha1::new();
        let mut sha512 = sha2::Sha512::new();
        let mut completed = 0u64;
        if resumed {
            ensure_partial_identity(&file, &partial)?;
            let mut buffer = [0u8; 64 * 1024];
            use std::io::{Read, Seek, SeekFrom};
            file.seek(SeekFrom::Start(0))
                .map_err(|e| io_error(&partial.path, e))?;
            loop {
                let count = file
                    .read(&mut buffer)
                    .map_err(|e| io_error(&partial.path, e))?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
                sha512.update(&buffer[..count]);
                completed += count as u64;
            }
        }
        loop {
            let chunk = tokio::select! {
                result = response.chunk() => result.map_err(|e| DownloadError::Request {
                    url: safe_url(&current_url),
                    message: request_error_message(e),
                })?,
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    check_cancelled_partial(request, &partial, resumable)?;
                    continue;
                }
            };
            let Some(chunk) = chunk else { break };
            check_cancelled_partial(request, &partial, resumable)?;
            if request
                .expected_size
                .is_some_and(|size| completed.saturating_add(chunk.len() as u64) > size)
            {
                partial.discard()?;
                return Err(DownloadError::SizeMismatch {
                    path: request.destination.clone(),
                    expected: request.expected_size.unwrap(),
                    actual: completed + chunk.len() as u64,
                });
            }
            file.write_all(&chunk)
                .map_err(|e| io_error(&partial.path, e))?;
            hasher.update(&chunk);
            sha512.update(&chunk);
            completed += chunk.len() as u64;
            if let Some(context) = &request.task_context {
                context
                    .emit(TaskEvent::Progress {
                        task: format!("download {}", request.destination.display()),
                        completed,
                        total,
                    })
                    .map_err(|_| {
                        if !resumable {
                            let _ = partial.discard();
                        }
                        DownloadError::Cancelled
                    })?;
                // Event consumers may cancel synchronously while handling progress.
                check_cancelled_partial(request, &partial, resumable)?;
            }
        }
        file.sync_all().map_err(|e| io_error(&partial.path, e))?;
        check_cancelled_partial(request, &partial, resumable)?;
        if let Some(expected) = request.expected_size
            && expected != completed
        {
            partial.discard()?;
            return Err(DownloadError::SizeMismatch {
                path: request.destination.clone(),
                expected,
                actual: completed,
            });
        }
        if let Some(expected) = &request.expected_sha1 {
            let actual = format!("{:x}", hasher.finalize());
            if !expected.eq_ignore_ascii_case(&actual) {
                partial.discard()?;
                return Err(DownloadError::Sha1Mismatch {
                    path: request.destination.clone(),
                    expected: expected.clone(),
                    actual,
                });
            }
        }
        if let Some(expected) = &request.expected_sha512 {
            let actual = format!("{:x}", sha512.finalize());
            if !expected.eq_ignore_ascii_case(&actual) {
                partial.discard()?;
                return Err(DownloadError::Sha512Mismatch {
                    path: request.destination.clone(),
                    expected: expected.clone(),
                    actual,
                });
            }
        }
        ensure_partial_identity(&file, &partial)?;
        drop(file);
        publish_partial(&partial, request, resumable).await?;
        let _ = fs::remove_file(&partial.etag_path);
        if let Some(context) = &request.task_context {
            context
                .emit(TaskEvent::Finished {
                    task: format!("download {}", request.destination.display()),
                })
                .map_err(|_| DownloadError::Cancelled)?;
        }
        Ok(DownloadOutcome {
            path: request.destination.clone(),
            bytes: completed,
            source_url: Some(url.into()),
            reused_existing: false,
        })
    }
}

fn check_cancelled(request: &DownloadRequest) -> Result<(), DownloadError> {
    if request
        .task_context
        .as_ref()
        .is_some_and(|c| c.cancellation.is_cancelled())
    {
        Err(DownloadError::Cancelled)
    } else {
        Ok(())
    }
}

fn check_cancelled_partial(
    request: &DownloadRequest,
    partial: &PartialFile,
    resumable: bool,
) -> Result<(), DownloadError> {
    if request
        .task_context
        .as_ref()
        .is_some_and(|context| context.cancellation.is_cancelled())
    {
        if !resumable {
            partial.discard()?;
        }
        Err(DownloadError::Cancelled)
    } else {
        Ok(())
    }
}

async fn publish_partial(
    partial: &PartialFile,
    request: &DownloadRequest,
    resumable: bool,
) -> Result<(), DownloadError> {
    // Windows antivirus/indexing services can briefly deny rename while they inspect a newly
    // completed artifact. Keep retries short and bounded, and never remove the old destination.
    #[cfg(windows)]
    const RETRY_DELAYS: [Duration; 6] = [
        Duration::from_millis(20),
        Duration::from_millis(40),
        Duration::from_millis(80),
        Duration::from_millis(160),
        Duration::from_millis(320),
        Duration::from_millis(640),
    ];
    #[cfg(not(windows))]
    const RETRY_DELAYS: [Duration; 0] = [];

    let mut attempt = 0usize;
    loop {
        check_cancelled_partial(request, partial, resumable)?;
        match fs::rename(&partial.path, &request.destination) {
            Ok(()) => return Ok(()),
            Err(error) if retryable_publish_error(&error) && attempt < RETRY_DELAYS.len() => {
                let mut remaining = RETRY_DELAYS[attempt];
                attempt += 1;
                while !remaining.is_zero() {
                    check_cancelled_partial(request, partial, resumable)?;
                    let interval = remaining.min(Duration::from_millis(50));
                    tokio::time::sleep(interval).await;
                    remaining = remaining.saturating_sub(interval);
                }
            }
            Err(error) => return Err(io_error(&request.destination, error)),
        }
    }
}

#[cfg(windows)]
fn retryable_publish_error(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
        || matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

#[cfg(not(windows))]
fn retryable_publish_error(_error: &std::io::Error) -> bool {
    false
}

fn verify_bytes(request: &DownloadRequest, bytes: &[u8]) -> Result<(), DownloadError> {
    if let Some(expected) = request.expected_size
        && expected != bytes.len() as u64
    {
        return Err(DownloadError::SizeMismatch {
            path: request.destination.clone(),
            expected,
            actual: bytes.len() as u64,
        });
    }
    if let Some(expected) = &request.expected_sha1 {
        let actual = format!("{:x}", Sha1::digest(bytes));
        if !expected.eq_ignore_ascii_case(&actual) {
            return Err(DownloadError::Sha1Mismatch {
                path: request.destination.clone(),
                expected: expected.clone(),
                actual,
            });
        }
    }
    if let Some(expected) = &request.expected_sha512 {
        let actual = format!("{:x}", sha2::Sha512::digest(bytes));
        if !expected.eq_ignore_ascii_case(&actual) {
            return Err(DownloadError::Sha512Mismatch {
                path: request.destination.clone(),
                expected: expected.clone(),
                actual,
            });
        }
    }
    Ok(())
}

fn safe_url(url: &reqwest::Url) -> String {
    let mut safe = url.clone();
    let _ = safe.set_username("");
    let _ = safe.set_password(None);
    safe.set_query(None);
    safe.set_fragment(None);
    safe.to_string()
}

fn valid_strong_etag(etag: &str) -> bool {
    etag.len() >= 2
        && etag.len() < 2048
        && etag.starts_with('"')
        && etag.ends_with('"')
        && !etag.bytes().any(|b| b.is_ascii_control())
}
fn resume_scope(url: &str, request: &DownloadRequest) -> String {
    format!(
        "{:x}",
        sha2::Sha512::digest(
            format!(
                "{url}\n{:?}\n{:?}\n{:?}",
                request.expected_sha1, request.expected_sha512, request.expected_size
            )
            .as_bytes()
        )
    )
}

fn request_error_message(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "request timed out; check the connection or proxy and retry".into()
    } else {
        format!("connection or transfer failed: {}", error.without_url())
    }
}

fn safe_url_text(url: &str) -> String {
    reqwest::Url::parse(url)
        .map(|url| safe_url(&url))
        .unwrap_or_else(|_| "<invalid URL>".into())
}

fn check_url_policy(request: &DownloadRequest, url: &reqwest::Url) -> Result<(), DownloadError> {
    if request.require_https
        && (url.scheme() != "https" || !url.username().is_empty() || url.password().is_some())
    {
        return Err(DownloadError::InvalidRequest(
            "download redirects must remain HTTPS without embedded credentials".into(),
        ));
    }
    if let Some(hosts) = &request.allowed_hosts {
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        if url.scheme() != "https" || !hosts.iter().any(|allowed| allowed == &host) {
            return Err(DownloadError::InvalidRequest(format!(
                "URL source is not allowed: {}",
                safe_url(url)
            )));
        }
    }
    Ok(())
}

fn next_redirect(
    request: &DownloadRequest,
    current: &reqwest::Url,
    location: &str,
    redirects: usize,
) -> Result<reqwest::Url, DownloadError> {
    if redirects >= 10 {
        return Err(DownloadError::Request {
            url: safe_url(current),
            message: "too many redirects".into(),
        });
    }
    let target = current.join(location).map_err(|e| DownloadError::Request {
        url: safe_url(current),
        message: e.to_string(),
    })?;
    check_url_policy(request, &target)?;
    Ok(target)
}

fn io_error(path: &Path, source: std::io::Error) -> DownloadError {
    DownloadError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

struct PartialFile {
    path: PathBuf,
    etag_path: PathBuf,
    lock_path: PathBuf,
}

impl PartialFile {
    fn for_destination(destination: &Path) -> Self {
        let mut path = destination.as_os_str().to_owned();
        path.push(".ccl-partial");
        let path = PathBuf::from(path);
        let mut etag = path.as_os_str().to_owned();
        etag.push(".etag");
        let mut lock = path.as_os_str().to_owned();
        lock.push(".lock");
        Self {
            path,
            etag_path: PathBuf::from(etag),
            lock_path: PathBuf::from(lock),
        }
    }

    async fn acquire(&self, request: &DownloadRequest) -> Result<PartialLock, DownloadError> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        let lock_metadata = fs::symlink_metadata(&self.lock_path);
        if lock_metadata.is_ok_and(|m| m.file_type().is_symlink() || !m.is_file()) {
            return Err(DownloadError::InvalidRequest(
                "unsafe partial download lock path".into(),
            ));
        }
        let file = options
            .open(&self.lock_path)
            .map_err(|e| io_error(&self.lock_path, e))?;
        ensure_same_path_file(&file, &self.lock_path)?;
        loop {
            check_cancelled(request)?;
            match fs2::FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(PartialLock(file)),
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
                {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => return Err(io_error(&self.lock_path, error)),
            }
        }
    }

    fn resume_state(&self, scope: &str) -> Result<(u64, Option<String>), DownloadError> {
        let (Ok(metadata), Ok(sidecar)) = (
            fs::symlink_metadata(&self.path),
            fs::symlink_metadata(&self.etag_path),
        ) else {
            self.discard()?;
            return Ok((0, None));
        };
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || !sidecar.is_file()
            || sidecar.file_type().is_symlink()
            || sidecar.len() > 4096
            || metadata.len() == 0
        {
            self.discard()?;
            return Ok((0, None));
        }
        let mut etag_file = OpenOptions::new()
            .read(true)
            .open(&self.etag_path)
            .map_err(|e| io_error(&self.etag_path, e))?;
        ensure_same_path_file(&etag_file, &self.etag_path)?;
        use std::io::Read;
        let mut raw = String::new();
        etag_file
            .read_to_string(&mut raw)
            .map_err(|e| io_error(&self.etag_path, e))?;
        let parsed = serde_json::from_str::<(String, String)>(&raw).ok();
        if let Some((etag, saved_scope)) = parsed
            && saved_scope == scope
            && valid_strong_etag(&etag)
        {
            return Ok((metadata.len(), Some(etag)));
        }
        self.discard()?;
        Ok((0, None))
    }
    fn save_etag(&self, etag: &str, scope: &str) -> Result<(), DownloadError> {
        if !valid_strong_etag(etag) {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&(etag, scope))
            .map_err(|e| DownloadError::InvalidRequest(e.to_string()))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.etag_path)
            .map_err(|e| io_error(&self.etag_path, e))?;
        file.write_all(&bytes)
            .map_err(|e| io_error(&self.etag_path, e))
    }

    fn discard(&self) -> Result<(), DownloadError> {
        for path in [&self.path, &self.etag_path] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_error(path, e)),
            }
        }
        Ok(())
    }
}

fn ensure_partial_identity(file: &fs::File, partial: &PartialFile) -> Result<(), DownloadError> {
    ensure_same_path_file(file, &partial.path)
}

fn ensure_same_path_file(file: &fs::File, path: &Path) -> Result<(), DownloadError> {
    let opened = file.metadata().map_err(|e| io_error(path, e))?;
    let linked = fs::symlink_metadata(path).map_err(|e| io_error(path, e))?;
    if !opened.is_file()
        || !linked.is_file()
        || linked.file_type().is_symlink()
        || !same_file(&opened, &linked)
    {
        return Err(DownloadError::InvalidRequest(format!(
            "unsafe file path: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len() && left.modified().ok() == right.modified().ok()
}

struct PartialLock(fs::File);
impl Drop for PartialLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

fn valid_content_range(value: &str, offset: u64, expected_total: Option<u64>) -> bool {
    let Some(rest) = value.strip_prefix("bytes ") else {
        return false;
    };
    let Some((range, total)) = rest.split_once('/') else {
        return false;
    };
    let Some((start, end)) = range.split_once('-') else {
        return false;
    };
    let (Ok(start), Ok(end), Ok(total)) = (
        start.parse::<u64>(),
        end.parse::<u64>(),
        total.parse::<u64>(),
    ) else {
        return false;
    };
    start == offset
        && end >= start
        && end < total
        && expected_total.is_none_or(|expected| expected == total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn test_path() -> PathBuf {
        static NEXT_PATH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "ccl-download-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_PATH.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    #[test]
    fn verifies_sha1_and_size() {
        let path = test_path();
        let request = DownloadRequest::new(vec!["http://localhost".into()], &path)
            .with_sha1("a9993e364706816aba3e25717850c26c9cd0d89d")
            .with_size(3);
        assert!(verify_bytes(&request, b"abc").is_ok());
        assert!(matches!(
            verify_bytes(&request, b"abd"),
            Err(DownloadError::Sha1Mismatch { .. })
        ));
        assert!(matches!(
            verify_bytes(&request, b"ab"),
            Err(DownloadError::SizeMismatch { .. })
        ));
    }

    #[test]
    fn verifies_sha512_for_cached_bytes_and_url_policy() {
        let request = DownloadRequest::new(vec![], test_path())
            .with_sha512("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000")
            .with_allowed_hosts(vec!["cdn.example".into()]);
        assert!(verify_bytes(&request, b"abc").is_err());
        let mut request = request;
        request.expected_sha512 = Some(format!("{:x}", sha2::Sha512::digest(b"abc")));
        assert!(verify_bytes(&request, b"abc").is_ok());
        assert!(
            check_url_policy(
                &request,
                &reqwest::Url::parse("http://cdn.example/file").unwrap()
            )
            .is_err()
        );
        assert!(
            check_url_policy(
                &request,
                &reqwest::Url::parse("https://evil.example/file").unwrap()
            )
            .is_err()
        );
        assert!(
            check_url_policy(
                &request,
                &reqwest::Url::parse("https://cdn.example/file").unwrap()
            )
            .is_ok()
        );
    }

    #[test]
    fn validates_every_https_redirect_and_limits_chain_to_ten() {
        let request = DownloadRequest::new(vec![], test_path())
            .with_allowed_hosts(vec!["cdn.example".into()]);
        let mut current = reqwest::Url::parse("https://cdn.example/0").unwrap();
        for redirects in 0..10 {
            current = next_redirect(
                &request,
                &current,
                &format!("/{}/", redirects + 1),
                redirects,
            )
            .unwrap();
        }
        assert!(matches!(
            next_redirect(&request, &current, "/11", 10),
            Err(DownloadError::Request { .. })
        ));
        assert!(matches!(
            next_redirect(&request, &current, "http://cdn.example/file", 0),
            Err(DownloadError::InvalidRequest(_))
        ));
        assert!(matches!(
            next_redirect(&request, &current, "https://attacker.example/file", 0),
            Err(DownloadError::InvalidRequest(_))
        ));
    }

    #[tokio::test]
    async fn sha512_mismatch_in_cache_preserves_file_if_replacement_fails() {
        let path = test_path();
        fs::write(&path, b"cached but wrong").unwrap();
        let manager = DownloadManager::new(&crate::config::DownloadConfig::default())
            .unwrap()
            .with_retry_policy(0, Duration::ZERO);
        let result = manager
            .download(
                DownloadRequest::new(vec!["http://127.0.0.1:1/unreachable".into()], &path)
                    .with_sha512(format!("{:x}", sha2::Sha512::digest(b"expected"))),
            )
            .await;
        assert!(matches!(result, Err(DownloadError::CandidatesExhausted(_))));
        assert_eq!(fs::read(&path).unwrap(), b"cached but wrong");
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn reuses_a_valid_existing_file_without_network() {
        let path = test_path();
        fs::write(&path, b"abc").unwrap();
        let manager = DownloadManager::new(&crate::config::DownloadConfig::default()).unwrap();
        let result = manager
            .download(
                DownloadRequest::new(vec!["http://127.0.0.1:1/unreachable".into()], &path)
                    .with_sha1("a9993e364706816aba3e25717850c26c9cd0d89d"),
            )
            .await
            .unwrap();
        assert!(result.reused_existing);
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn offline_mode_reports_cold_cache_miss_without_network() {
        let path = test_path();
        let config = crate::config::DownloadConfig {
            offline: true,
            ..Default::default()
        };
        let manager = DownloadManager::new(&config).unwrap();
        let result = manager
            .download(DownloadRequest::new(
                vec!["http://127.0.0.1:1/must-not-connect".into()],
                &path,
            ))
            .await;
        assert!(
            matches!(&result, Err(DownloadError::OfflineCacheMiss(p)) if p == &path),
            "{result:?}"
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn offline_mode_reuses_verified_cached_artifact() {
        let path = test_path();
        fs::write(&path, b"abc").unwrap();
        let config = crate::config::DownloadConfig {
            offline: true,
            ..Default::default()
        };
        let manager = DownloadManager::new(&config).unwrap();
        let outcome = manager
            .download(
                DownloadRequest::new(vec!["http://127.0.0.1:1/must-not-connect".into()], &path)
                    .with_sha1("a9993e364706816aba3e25717850c26c9cd0d89d"),
            )
            .await
            .unwrap();
        assert!(outcome.reused_existing);
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn resumes_interrupted_transfer_only_with_matching_etag_and_range() {
        let body = b"complete verified artifact";
        let split = 9;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let _ = first.read(&mut request).unwrap();
            write!(first, "HTTP/1.1 200 OK\r\nETag: \"stable-v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            first.write_all(&body[..split]).unwrap();
            drop(first);

            let (mut second, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let count = second.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..count]);
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains(&format!("range: bytes={split}-"))
            );
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("if-range: \"stable-v1\"")
            );
            write!(second, "HTTP/1.1 206 Partial Content\r\nETag: \"stable-v1\"\r\nContent-Range: bytes {split}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()-1, body.len(), body.len()-split).unwrap();
            second.write_all(&body[split..]).unwrap();
        });
        let path = test_path();
        let manager = DownloadManager::new(&crate::config::DownloadConfig::default())
            .unwrap()
            .with_retry_policy(1, Duration::ZERO);
        let result = manager
            .download(
                DownloadRequest::new(vec![format!("http://{address}/artifact")], &path)
                    .with_sha1(format!("{:x}", Sha1::digest(body)))
                    .with_size(body.len() as u64),
            )
            .await
            .unwrap();
        worker.join().unwrap();
        assert_eq!(fs::read(&path).unwrap(), body);
        assert_eq!(result.bytes, body.len() as u64);
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn changed_etag_discards_partial_before_clean_retry() {
        let body = b"same artifact, new representation";
        let split = 7;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let _ = first.read(&mut request).unwrap();
            write!(first, "HTTP/1.1 200 OK\r\nETag: \"v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            first.write_all(&body[..split]).unwrap();
            drop(first);

            let (mut second, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let count = second.read(&mut request).unwrap();
            assert!(
                String::from_utf8_lossy(&request[..count])
                    .to_ascii_lowercase()
                    .contains("range: bytes=")
            );
            write!(second, "HTTP/1.1 206 Partial Content\r\nETag: \"v2\"\r\nContent-Range: bytes {split}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()-1, body.len(), body.len()-split).unwrap();
            second.write_all(&body[split..]).unwrap();

            let (mut third, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let count = third.read(&mut request).unwrap();
            assert!(
                !String::from_utf8_lossy(&request[..count])
                    .to_ascii_lowercase()
                    .contains("range: bytes=")
            );
            write!(third, "HTTP/1.1 200 OK\r\nETag: \"v2\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            third.write_all(body).unwrap();
        });
        let path = test_path();
        let manager = DownloadManager::new(&crate::config::DownloadConfig::default())
            .unwrap()
            .with_retry_policy(2, Duration::ZERO);
        manager
            .download(
                DownloadRequest::new(vec![format!("http://{address}/artifact")], &path)
                    .with_sha1(format!("{:x}", Sha1::digest(body)))
                    .with_size(body.len() as u64),
            )
            .await
            .unwrap();
        worker.join().unwrap();
        assert_eq!(fs::read(&path).unwrap(), body);
        fs::remove_file(path).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn publish_retries_windows_sharing_error_after_destination_handle_closes() {
        use std::os::windows::fs::OpenOptionsExt;

        let destination = test_path();
        let partial = PartialFile::for_destination(&destination);
        fs::write(&destination, b"old verified destination").unwrap();
        fs::write(&partial.path, b"new verified artifact").unwrap();
        // Permit reads and writes but deny delete/rename while this handle is open.
        let held = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0x1 | 0x2)
            .open(&destination)
            .unwrap();
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(120));
            drop(held);
        });
        let request = DownloadRequest::new(Vec::<String>::new(), &destination);
        publish_partial(&partial, &request, true).await.unwrap();
        release.join().unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new verified artifact");
        assert!(!partial.path.exists());
        fs::remove_file(destination).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn exhausted_publish_retries_preserve_old_destination_and_valid_partial() {
        use std::os::windows::fs::OpenOptionsExt;

        let destination = test_path();
        let partial = PartialFile::for_destination(&destination);
        fs::write(&destination, b"old verified destination").unwrap();
        fs::write(&partial.path, b"new verified artifact").unwrap();
        let held = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0x1 | 0x2)
            .open(&destination)
            .unwrap();
        let request = DownloadRequest::new(Vec::<String>::new(), &destination);
        let result = publish_partial(&partial, &request, true).await;
        assert!(matches!(result, Err(DownloadError::Io { .. })));
        assert_eq!(fs::read(&destination).unwrap(), b"old verified destination");
        assert_eq!(fs::read(&partial.path).unwrap(), b"new verified artifact");
        drop(held);
        fs::remove_file(destination).unwrap();
        fs::remove_file(partial.path).unwrap();
    }

    #[tokio::test]
    async fn server_ignoring_range_restarts_from_full_response_safely() {
        let body = b"full body when range is ignored";
        let split = 6;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let _ = first.read(&mut request).unwrap();
            write!(first, "HTTP/1.1 200 OK\r\nETag: \"same\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            first.write_all(&body[..split]).unwrap();
            drop(first);

            let (mut second, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let count = second.read(&mut request).unwrap();
            assert!(
                String::from_utf8_lossy(&request[..count])
                    .to_ascii_lowercase()
                    .contains("range: bytes=")
            );
            write!(second, "HTTP/1.1 200 OK\r\nETag: \"same\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            second.write_all(body).unwrap();
        });
        let path = test_path();
        let manager = DownloadManager::new(&crate::config::DownloadConfig::default())
            .unwrap()
            .with_retry_policy(1, Duration::ZERO);
        manager
            .download(
                DownloadRequest::new(vec![format!("http://{address}/artifact")], &path)
                    .with_sha1(format!("{:x}", Sha1::digest(body)))
                    .with_size(body.len() as u64),
            )
            .await
            .unwrap();
        worker.join().unwrap();
        assert_eq!(fs::read(&path).unwrap(), body);
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn range_not_satisfiable_discards_partial_and_restarts_from_zero() {
        let body = b"recovered from 416";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let path = test_path();
        let url = format!("http://{address}/artifact");
        let request = DownloadRequest::new(vec![url.clone()], &path)
            .with_sha1(format!("{:x}", Sha1::digest(body)))
            .with_size(body.len() as u64);
        let partial = PartialFile::for_destination(&path);
        fs::write(&partial.path, b"oldpartial").unwrap();
        partial
            .save_etag("\"old\"", &resume_scope(&url, &request))
            .unwrap();
        let worker = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let count = first.read(&mut request).unwrap();
            assert!(
                String::from_utf8_lossy(&request[..count])
                    .to_ascii_lowercase()
                    .contains("range: bytes=")
            );
            first.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            drop(first);
            let (mut second, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let count = second.read(&mut request).unwrap();
            assert!(
                !String::from_utf8_lossy(&request[..count])
                    .to_ascii_lowercase()
                    .contains("range: bytes=")
            );
            write!(second, "HTTP/1.1 200 OK\r\nETag: \"fresh\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            second.write_all(body).unwrap();
        });
        let manager = DownloadManager::new(&crate::config::DownloadConfig::default())
            .unwrap()
            .with_retry_policy(1, Duration::ZERO);
        manager.download(request).await.unwrap();
        worker.join().unwrap();
        assert_eq!(fs::read(&path).unwrap(), body);
        fs::remove_file(&path).unwrap();
        fs::remove_file(partial.lock_path).unwrap();
    }

    #[tokio::test]
    async fn concurrent_requests_for_one_destination_publish_only_one_transfer() {
        let body = b"one atomic artifact";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let _ = socket.read(&mut request).unwrap();
            thread::sleep(Duration::from_millis(100));
            write!(socket, "HTTP/1.1 200 OK\r\nETag: \"single\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            socket.write_all(body).unwrap();
        });
        let path = test_path();
        let manager = DownloadManager::new(&crate::config::DownloadConfig::default()).unwrap();
        let make_request = || {
            DownloadRequest::new(vec![format!("http://{address}/artifact")], &path)
                .with_sha1(format!("{:x}", Sha1::digest(body)))
        };
        let (first, second) = tokio::join!(
            manager.download(make_request()),
            manager.download(make_request())
        );
        worker.join().unwrap();
        assert!(
            first.is_ok() && second.is_ok(),
            "first: {first:?}; second: {second:?}"
        );
        assert_eq!(
            usize::from(first.unwrap().reused_existing)
                + usize::from(second.unwrap().reused_existing),
            1
        );
        assert_eq!(fs::read(&path).unwrap(), body);
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn content_range_must_match_offset_end_and_expected_total() {
        assert!(valid_content_range("bytes 12-19/20", 12, Some(20)));
        assert!(!valid_content_range("bytes 11-19/20", 12, Some(20)));
        assert!(!valid_content_range("bytes 12-20/20", 12, Some(20)));
        assert!(!valid_content_range("bytes 12-19/21", 12, Some(20)));
        assert!(!valid_content_range("items 12-19/20", 12, Some(20)));
    }

    #[test]
    fn partial_etag_cache_is_bound_to_url_and_integrity_request() {
        let destination = test_path();
        let part = PartialFile::for_destination(&destination);
        let url = "https://cdn.example/artifact";
        let request = DownloadRequest::new(vec![url.into()], &destination)
            .with_sha1("a9993e364706816aba3e25717850c26c9cd0d89d")
            .with_size(3);
        let scope = resume_scope(url, &request);
        fs::write(&part.path, b"ab").unwrap();
        part.save_etag("\"stable\"", &scope).unwrap();
        assert_eq!(
            part.resume_state(&scope).unwrap(),
            (2, Some("\"stable\"".into()))
        );
        assert_eq!(
            part.resume_state("different-url-or-hash").unwrap(),
            (0, None)
        );
        assert!(!part.path.exists());
        fs::remove_file(&part.lock_path).ok();
    }

    #[test]
    fn sensitive_headers_apply_only_to_the_exact_https_host() {
        let request = DownloadRequest::new(vec![], test_path())
            .with_api_key("do-not-log-this", vec!["api.example".into()])
            .unwrap();
        let header = &request.sensitive_headers[0];
        assert!(header.applies(&reqwest::Url::parse("https://api.example/v1/file").unwrap()));
        assert!(!header.applies(&reqwest::Url::parse("https://cdn.example/v1/file").unwrap()));
        assert!(!header.applies(&reqwest::Url::parse("https://sub.api.example/v1/file").unwrap()));
        assert!(!header.applies(&reqwest::Url::parse("http://api.example/v1/file").unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn partial_cache_symlink_is_discarded_without_touching_target() {
        use std::os::unix::fs::symlink;
        let destination = test_path();
        let partial = PartialFile::for_destination(&destination);
        let victim = test_path();
        fs::write(&victim, b"keep me").unwrap();
        symlink(&victim, &partial.path).unwrap();
        fs::write(&partial.etag_path, "\"stable\"").unwrap();
        assert_eq!(partial.resume_state("fixture").unwrap(), (0, None));
        assert_eq!(fs::read(&victim).unwrap(), b"keep me");
        assert!(!partial.path.exists());
        fs::remove_file(victim).unwrap();
    }

    #[tokio::test]
    async fn rejects_corrupt_http_body_and_cleans_temporary_file() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = socket.read(&mut request);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nxyz")
                .unwrap();
        });
        let destination = test_path().join("artifact.jar");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, b"keep").unwrap();
        let manager = DownloadManager::new(&crate::config::DownloadConfig::default())
            .unwrap()
            .with_retry_policy(0, Duration::ZERO);
        let result = manager
            .download(
                DownloadRequest::new(vec![format!("http://{address}/artifact")], &destination)
                    .with_sha1("a9993e364706816aba3e25717850c26c9cd0d89d"),
            )
            .await;
        server.join().unwrap();
        assert!(matches!(result, Err(DownloadError::CandidatesExhausted(_))));
        assert_eq!(fs::read(&destination).unwrap(), b"keep");
        let parent = destination.parent().unwrap();
        let artifacts = fs::read_dir(parent)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(
            artifacts.len(),
            2,
            "unexpected leftover transfer artifacts: {artifacts:?}"
        ); // destination plus persistent per-file lock
        fs::remove_file(PartialFile::for_destination(&destination).lock_path).unwrap();
        fs::remove_file(&destination).unwrap();
        fs::remove_dir(parent).unwrap();
    }

    #[tokio::test]
    async fn cancellation_before_transfer_preserves_existing_file() {
        let path = test_path();
        fs::write(&path, b"keep").unwrap();
        let token = crate::task::CancellationToken::default();
        token.cancel();
        let context = TaskContext::new(token, std::sync::Arc::new(|_| {}));
        let manager = DownloadManager::new(&crate::config::DownloadConfig::default()).unwrap();
        let result = manager
            .download(
                DownloadRequest::new(vec!["http://127.0.0.1:1/nope".into()], &path)
                    .with_task_context(context),
            )
            .await;
        assert!(matches!(result, Err(DownloadError::Cancelled)));
        assert_eq!(fs::read(&path).unwrap(), b"keep");
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn cancellation_during_a_stalled_response_removes_partial_file() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = socket.read(&mut request);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\na")
                .unwrap();
            thread::sleep(Duration::from_secs(1));
        });
        let destination = test_path().join("partial.jar");
        let token = crate::task::CancellationToken::default();
        let cancellation = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            cancellation.cancel();
        });
        let manager = DownloadManager::new(&crate::config::DownloadConfig::default()).unwrap();
        let context = TaskContext::new(token, std::sync::Arc::new(|_| {}));
        let result = manager
            .download(
                DownloadRequest::new(vec![format!("http://{address}/partial")], &destination)
                    .with_task_context(context),
            )
            .await;
        assert!(matches!(result, Err(DownloadError::Cancelled)));
        server.join().unwrap();
        assert!(!destination.exists());
        let partial = PartialFile::for_destination(&destination);
        assert!(!partial.path.exists());
        assert!(!partial.etag_path.exists());
        assert_eq!(
            fs::read_dir(destination.parent().unwrap()).unwrap().count(),
            1
        );
        fs::remove_file(partial.lock_path).unwrap();
        fs::remove_dir(destination.parent().unwrap()).unwrap();
    }
}
