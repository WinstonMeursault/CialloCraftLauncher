//! Opt-in streaming downloader benchmark using a deterministic local artifact.
//!
//! Run with `cargo run -p ccl-core --example artifact_benchmark -- --bytes 67108864 --iterations 3`.

use std::{
    env, fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use ccl_core::{
    config::DownloadConfig,
    download::{DownloadManager, DownloadRequest},
};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use sha2::{Sha256, Sha512};

const CHUNK: usize = 64 * 1024;
const ROUTE: &str = "/artifact.bin";

struct Server {
    address: String,
    stop: Arc<AtomicBool>,
    requests: Arc<AtomicU64>,
    sent_bytes: Arc<AtomicU64>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn start(fixture: PathBuf) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind benchmark server");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let address = listener.local_addr().expect("local address").to_string();
        let fixture = Arc::new(fixture);
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicU64::new(0));
        let sent_bytes = Arc::new(AtomicU64::new(0));
        let worker_stop = stop.clone();
        let worker_fixture = fixture.clone();
        let worker_requests = requests.clone();
        let worker_bytes = sent_bytes.clone();
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let fixture = worker_fixture.clone();
                        let requests = worker_requests.clone();
                        let bytes = worker_bytes.clone();
                        thread::spawn(move || serve(stream, &fixture, &requests, &bytes));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            stop,
            requests,
            sent_bytes,
            worker: Some(worker),
        }
    }

    fn url(&self) -> String {
        format!("http://{}{}", self.address, ROUTE)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn serve(mut stream: TcpStream, fixture: &Path, requests: &AtomicU64, sent_bytes: &AtomicU64) {
    // Accepted sockets can inherit the nonblocking listener mode on macOS.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    loop {
        let Ok(count) = stream.read(&mut buffer) else {
            return;
        };
        if count == 0 {
            return;
        }
        request.extend_from_slice(&buffer[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let line = String::from_utf8_lossy(&request);
    if line.split_whitespace().nth(1) != Some(ROUTE) {
        return;
    }
    requests.fetch_add(1, Ordering::Relaxed);
    let Ok(mut file) = fs::File::open(fixture) else {
        return;
    };
    let Ok(size) = file.metadata().map(|metadata| metadata.len()) else {
        return;
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\nContent-Type: application/octet-stream\r\n\r\n"
    );
    if stream.write_all(header.as_bytes()).is_err() {
        return;
    }
    let mut buffer = [0u8; CHUNK];
    loop {
        let Ok(count) = file.read(&mut buffer) else {
            return;
        };
        if count == 0 {
            break;
        }
        if stream.write_all(&buffer[..count]).is_err() {
            return;
        }
        sent_bytes.fetch_add(count as u64, Ordering::Relaxed);
    }
}

fn generate_fixture(path: &Path, size: u64) -> std::io::Result<(String, String, String)> {
    let mut file = fs::File::create(path)?;
    let mut sha1 = Sha1::new();
    let mut sha512 = Sha512::new();
    let mut sha256 = Sha256::new();
    let mut state = 0x4343_4c41_5254_4946u64;
    let mut remaining = size;
    let mut buffer = [0u8; CHUNK];
    while remaining > 0 {
        let count = remaining.min(CHUNK as u64) as usize;
        for byte in &mut buffer[..count] {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = (state >> 24) as u8;
        }
        file.write_all(&buffer[..count])?;
        sha1.update(&buffer[..count]);
        sha512.update(&buffer[..count]);
        sha256.update(&buffer[..count]);
        remaining -= count as u64;
    }
    file.sync_all()?;
    Ok((
        format!("{:x}", sha1.finalize()),
        format!("{:x}", sha512.finalize()),
        format!("{:x}", sha256.finalize()),
    ))
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_root() -> PathBuf {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    env::temp_dir().join(format!(
        "ccl-artifact-benchmark-{}-{id}",
        std::process::id()
    ))
}

fn request(
    url: &str,
    destination: PathBuf,
    hash: &str,
    sha1: &str,
    sha512: &str,
    size: u64,
) -> DownloadRequest {
    let request = DownloadRequest::new(vec![url.into()], destination).with_size(size);
    match hash {
        "sha1" => request.with_sha1(sha1),
        "sha512" => request.with_sha512(sha512),
        "both" => request.with_sha1(sha1).with_sha512(sha512),
        "size_only" => request,
        _ => unreachable!(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = 64 * 1024 * 1024u64;
    let mut iterations = 3usize;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bytes" => bytes = args.next().ok_or("--bytes requires a value")?.parse()?,
            "--iterations" => {
                iterations = args
                    .next()
                    .ok_or("--iterations requires a value")?
                    .parse()?
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    if bytes == 0 || iterations == 0 {
        return Err("bytes and iterations must be positive".into());
    }

    let root = temp_root();
    fs::create_dir_all(&root)?;
    let _cleanup = Cleanup(root.clone());
    let fixture = root.join("fixture.bin");
    let (sha1, sha512, fixture_sha256) = generate_fixture(&fixture, bytes)?;
    let server = Server::start(fixture);
    let manager =
        DownloadManager::new(&DownloadConfig::default())?.with_retry_policy(0, Duration::ZERO);
    let mut measurements: Vec<Value> = Vec::new();

    for mode in ["sha1", "sha512", "both", "size_only"] {
        for iteration in 0..iterations {
            let destination = root.join(format!("{mode}-{iteration}.bin"));
            let before_requests = server.requests.load(Ordering::Relaxed);
            let before_bytes = server.sent_bytes.load(Ordering::Relaxed);
            let started = Instant::now();
            let cold = manager
                .download(request(
                    &server.url(),
                    destination.clone(),
                    mode,
                    &sha1,
                    &sha512,
                    bytes,
                ))
                .await?;
            let cold_ms = started.elapsed().as_secs_f64() * 1000.0;
            let cold_requests = server.requests.load(Ordering::Relaxed) - before_requests;
            let cold_bytes = server.sent_bytes.load(Ordering::Relaxed) - before_bytes;
            if cold.reused_existing
                || cold.bytes != bytes
                || cold_requests != 1
                || cold_bytes != bytes
            {
                return Err(format!("unexpected cold result for {mode}: {cold:?}, requests={cold_requests}, bytes={cold_bytes}").into());
            }

            let before_requests = server.requests.load(Ordering::Relaxed);
            let before_bytes = server.sent_bytes.load(Ordering::Relaxed);
            let started = Instant::now();
            let warm = manager
                .download(request(
                    &server.url(),
                    destination,
                    mode,
                    &sha1,
                    &sha512,
                    bytes,
                ))
                .await?;
            let warm_ms = started.elapsed().as_secs_f64() * 1000.0;
            let warm_requests = server.requests.load(Ordering::Relaxed) - before_requests;
            let warm_bytes = server.sent_bytes.load(Ordering::Relaxed) - before_bytes;
            if !warm.reused_existing || warm.bytes != bytes || warm_requests != 0 || warm_bytes != 0
            {
                return Err(format!("unexpected warm result for {mode}: {warm:?}, requests={warm_requests}, bytes={warm_bytes}").into());
            }
            measurements.push(json!({
                "mode": mode, "iteration": iteration,
                "cold": {"elapsed_ms": cold_ms, "requests": cold_requests, "transferred_bytes": cold_bytes},
                "warm": {"elapsed_ms": warm_ms, "requests": warm_requests, "transferred_bytes": warm_bytes}
            }));
        }
    }

    println!(
        "{}",
        json!({
            "schema_version": 1,
            "benchmark": "streaming_artifact_cold_warm",
            "fixture": {"bytes": bytes, "sha256": fixture_sha256, "sha1": sha1, "sha512": sha512},
            "iterations": iterations,
            "measurements": measurements
        })
    );
    Ok(())
}
