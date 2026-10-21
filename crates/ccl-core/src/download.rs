//! Reusable asynchronous artifact downloader.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use sha1::{Digest, Sha1};
use thiserror::Error;
use tokio::sync::Semaphore;

use crate::{TaskContext, TaskEvent};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

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
    #[error("size mismatch for {path}: expected {expected}, got {actual}")]
    SizeMismatch {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },
    #[error("all URL candidates failed: {0}")]
    CandidatesExhausted(String),
}

#[derive(Clone)]
pub struct DownloadRequest {
    /// Candidate endpoints in preference order (normally from `source::url_candidates`).
    pub urls: Vec<String>,
    pub destination: PathBuf,
    pub expected_sha1: Option<String>,
    pub expected_size: Option<u64>,
    /// Optional event and cancellation context shared with the caller.
    pub task_context: Option<TaskContext>,
}

impl DownloadRequest {
    pub fn new(urls: Vec<String>, destination: impl Into<PathBuf>) -> Self {
        Self {
            urls,
            destination: destination.into(),
            expected_sha1: None,
            expected_size: None,
            task_context: None,
        }
    }

    pub fn with_sha1(mut self, sha1: impl Into<String>) -> Self {
        self.expected_sha1 = Some(sha1.into().to_ascii_lowercase());
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
}

impl DownloadManager {
    pub fn new(config: &crate::config::DownloadConfig) -> Result<Self, DownloadError> {
        if config.max_parallel == 0 {
            return Err(DownloadError::InvalidRequest(
                "max_parallel must be nonzero".into(),
            ));
        }
        let mut builder = reqwest::Client::builder()
            .user_agent(concat!("CialloCraftLauncher/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(30));
        if let Some(proxy) = &config.http_proxy {
            let proxy = reqwest::Proxy::all(proxy)
                .map_err(|e| DownloadError::InvalidRequest(e.to_string()))?;
            builder = builder.proxy(proxy);
        }
        let client = builder
            .build()
            .map_err(|e| DownloadError::InvalidRequest(e.to_string()))?;
        Ok(Self {
            client,
            permits: Arc::new(Semaphore::new(config.max_parallel)),
            retries_per_candidate: 3,
            base_backoff: Duration::from_millis(250),
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
                                | DownloadError::SizeMismatch { .. }
                        );
                        failures.push(format!("{url}: {error}"));
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
        let send = self.client.get(url).send();
        tokio::pin!(send);
        let mut response = loop {
            tokio::select! {
                result = &mut send => {
                    break result.map_err(|e| DownloadError::Request {
                        url: url.into(),
                        message: e.to_string(),
                    })?;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => check_cancelled(request)?,
            }
        };
        if !response.status().is_success() {
            return Err(DownloadError::HttpStatus {
                url: url.into(),
                status: response.status().as_u16(),
            });
        }
        let total = response.content_length();
        let mut temp = TempFile::create(&request.destination)?;
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&temp.path)
            .map_err(|e| io_error(&temp.path, e))?;
        let mut hasher = Sha1::new();
        let mut completed = 0u64;
        loop {
            let chunk = tokio::select! {
                result = response.chunk() => result.map_err(|e| DownloadError::Request {
                    url: url.into(),
                    message: e.to_string(),
                })?,
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    check_cancelled(request)?;
                    continue;
                }
            };
            let Some(chunk) = chunk else { break };
            check_cancelled(request)?;
            file.write_all(&chunk)
                .map_err(|e| io_error(&temp.path, e))?;
            hasher.update(&chunk);
            completed += chunk.len() as u64;
            if let Some(context) = &request.task_context {
                context
                    .emit(TaskEvent::Progress {
                        task: format!("download {}", request.destination.display()),
                        completed,
                        total,
                    })
                    .map_err(|_| DownloadError::Cancelled)?;
            }
        }
        file.sync_all().map_err(|e| io_error(&temp.path, e))?;
        check_cancelled(request)?;
        if let Some(expected) = request.expected_size
            && expected != completed
        {
            return Err(DownloadError::SizeMismatch {
                path: request.destination.clone(),
                expected,
                actual: completed,
            });
        }
        if let Some(expected) = &request.expected_sha1 {
            let actual = format!("{:x}", hasher.finalize());
            if !expected.eq_ignore_ascii_case(&actual) {
                return Err(DownloadError::Sha1Mismatch {
                    path: request.destination.clone(),
                    expected: expected.clone(),
                    actual,
                });
            }
        }
        fs::rename(&temp.path, &request.destination)
            .map_err(|e| io_error(&request.destination, e))?;
        temp.disarm();
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
    Ok(())
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

struct TempFile {
    path: PathBuf,
    armed: bool,
}
impl TempFile {
    fn create(destination: &Path) -> Result<Self, DownloadError> {
        let parent = parent_dir(destination);
        let name = destination
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download");
        loop {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".{name}.ccl-tmp-{}-{id}", std::process::id()));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self { path, armed: true }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io_error(&path, e)),
            }
        }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for TempFile {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
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
        std::env::temp_dir().join(format!(
            "ccl-download-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
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
        assert!(!destination.exists());
        let parent = destination.parent().unwrap();
        assert_eq!(fs::read_dir(parent).unwrap().count(), 0);
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
        assert_eq!(
            fs::read_dir(destination.parent().unwrap()).unwrap().count(),
            0
        );
        fs::remove_dir(destination.parent().unwrap()).unwrap();
    }
}
