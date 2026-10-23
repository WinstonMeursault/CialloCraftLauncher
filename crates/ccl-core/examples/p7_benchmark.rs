//! Deterministic offline-friendly installation benchmark for P7 CI.
//!
//! Uses a localhost HTTP mirror and the public InstallService. No upstream
//! service, Minecraft account, display server, or remote fixture is required.

use std::{
    collections::HashMap,
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
    config::{DownloadConfig, SourceMode},
    install::{InstallOptions, InstallService},
};
use serde_json::json;
use sha1::{Digest, Sha1};
use sha2::Sha256;

struct Server {
    address: String,
    stop: Arc<AtomicBool>,
    requests: Arc<AtomicU64>,
    sent_bytes: Arc<AtomicU64>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn start(routes: HashMap<String, Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local benchmark mirror");
        listener
            .set_nonblocking(true)
            .expect("set listener nonblocking");
        let address = listener.local_addr().expect("local address").to_string();
        let routes = Arc::new(routes);
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicU64::new(0));
        let sent_bytes = Arc::new(AtomicU64::new(0));
        let worker_stop = stop.clone();
        let worker_requests = requests.clone();
        let worker_bytes = sent_bytes.clone();
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let routes = routes.clone();
                        let requests = worker_requests.clone();
                        let bytes = worker_bytes.clone();
                        thread::spawn(move || serve(stream, &routes, &requests, &bytes));
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

    fn root(&self) -> String {
        format!("http://{}", self.address)
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

fn serve(
    mut stream: TcpStream,
    routes: &HashMap<String, Vec<u8>>,
    requests: &AtomicU64,
    sent_bytes: &AtomicU64,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
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
    let path = line.split_whitespace().nth(1).unwrap_or("/");
    requests.fetch_add(1, Ordering::Relaxed);
    let body = routes.get(path).map(Vec::as_slice);
    if body.is_none() {
        eprintln!("benchmark mirror route missing: {path}");
    }
    let (status, body) = body.map_or(("404 Not Found", &[][..]), |body| ("200 OK", body));
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/octet-stream\r\n\r\n",
        body.len()
    );
    if stream.write_all(header.as_bytes()).is_err() {
        return;
    }
    match stream.write_all(body) {
        Ok(()) => {
            sent_bytes.fetch_add(body.len() as u64, Ordering::Relaxed);
        }
        Err(error) => eprintln!("benchmark mirror response write failed for {path}: {error}"),
    }
}

fn sha1(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}

fn deterministic_payload(size: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

fn fixture_routes() -> HashMap<String, Vec<u8>> {
    // Fixed byte sizes and PRNG seeds keep workload and hashes reproducible.
    let client = deterministic_payload(4 * 1024 * 1024, 0x4343_4c31);
    let library = deterministic_payload(2 * 1024 * 1024, 0x4343_4c32);
    let object = deterministic_payload(3 * 1024 * 1024, 0x4343_4c33);
    let object_hash = sha1(&object);
    let index = serde_json::to_vec(&json!({
        "objects": { "textures/benchmark.bin": { "hash": object_hash, "size": object.len() } }
    }))
    .expect("asset index JSON");
    let index_hash = sha1(&index);
    let client_url = "https://launcher.mojang.com/client.jar";
    let library_url = "https://libraries.minecraft.net/example/benchmark/1.0/benchmark.jar";
    let index_url = "https://piston-data.mojang.com/assets/index.json";
    let metadata = serde_json::to_vec(&json!({
        "id": "p7-fixture-1",
        "downloads": { "client": { "url": client_url, "sha1": sha1(&client), "size": client.len() } },
        "libraries": [{"name":"example:benchmark:1.0","downloads":{"artifact":{
            "path":"example/benchmark/1.0/benchmark.jar","url":library_url,"sha1":sha1(&library),"size":library.len()
        }}}],
        "assetIndex": {"id":"p7-fixture-assets","url":index_url,"sha1":index_hash,"size":index.len()}
    })).expect("version metadata JSON");
    let manifest = serde_json::to_vec(&json!({ "versions": [{
        "id":"p7-fixture-1", "url":"https://piston-meta.mojang.com/version.json", "sha1":sha1(&metadata)
    }]})).expect("version manifest JSON");

    let mut routes = HashMap::new();
    routes.insert("/mc/game/version_manifest_v2.json".into(), manifest);
    routes.insert("/version.json".into(), metadata);
    routes.insert("/client.jar".into(), client);
    routes.insert(
        "/libraries/example/benchmark/1.0/benchmark.jar".into(),
        library,
    );
    routes.insert("/assets/index.json".into(), index);
    routes.insert(
        format!("/assets/{}/{}", &object_hash[..2], object_hash),
        object,
    );
    routes
}

fn directory_size(path: &Path) -> std::io::Result<(u64, u64)> {
    let mut bytes = 0;
    let mut files = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            let (child_bytes, child_files) = directory_size(&entry.path())?;
            bytes += child_bytes;
            files += child_files;
        } else if metadata.is_file() {
            bytes += metadata.len();
            files += 1;
        }
    }
    Ok((bytes, files))
}

#[cfg(target_os = "linux")]
fn peak_memory() -> serde_json::Value {
    let result = fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                let rest = line.strip_prefix("VmHWM:")?.trim();
                let kib = rest.strip_suffix("kB")?.trim().parse::<u64>().ok()?;
                Some(kib * 1024)
            })
        });
    match result {
        Some(bytes) => {
            json!({ "available": true, "peak_rss_bytes": bytes, "source": "/proc/self/status VmHWM" })
        }
        None => json!({ "available": false, "reason": "could not read /proc/self/status VmHWM" }),
    }
}

#[cfg(target_os = "macos")]
fn peak_memory() -> serde_json::Value {
    #[repr(C)]
    struct RawRusage([i64; 18]);
    unsafe extern "C" {
        fn getrusage(who: i32, usage: *mut RawRusage) -> i32;
    }
    let mut usage = RawRusage([0; 18]);
    // macOS `struct rusage` starts with two 16-byte timeval fields and reports
    // ru_maxrss in bytes. The backing array reserves the full 144-byte record.
    let result = unsafe { getrusage(0, &mut usage) };
    if result == 0 && usage.0[4] >= 0 {
        json!({ "available": true, "peak_rss_bytes": usage.0[4], "source": "getrusage(RUSAGE_SELF).ru_maxrss" })
    } else {
        json!({ "available": false, "reason": "getrusage(RUSAGE_SELF) failed" })
    }
}

#[cfg(windows)]
fn peak_memory() -> serde_json::Value {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { zeroed() };
    counters.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let result = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    if result != 0 {
        json!({ "available": true, "peak_rss_bytes": counters.PeakWorkingSetSize, "source": "GetProcessMemoryInfo.PeakWorkingSetSize" })
    } else {
        json!({ "available": false, "reason": "GetProcessMemoryInfo failed" })
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn peak_memory() -> serde_json::Value {
    json!({ "available": false, "reason": "peak RSS source is not implemented for this platform" })
}

fn temp_root() -> PathBuf {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    env::temp_dir().join(format!("ccl-p7-benchmark-{}-{id}", std::process::id()))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" => {
                output = Some(PathBuf::from(args.next().ok_or("--output requires path")?))
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    let output = output.ok_or("usage: p7_benchmark --output PATH")?;
    let root = temp_root();
    fs::create_dir_all(&root)?;
    let _cleanup = Cleanup(root.clone());
    let routes = fixture_routes();
    let mut fixture_entries: Vec<_> = routes.iter().collect();
    fixture_entries.sort_by_key(|(path, _)| *path);
    let mut fixture_digest = Sha256::new();
    for (path, body) in fixture_entries {
        fixture_digest.update(path.as_bytes());
        fixture_digest.update([0]);
        fixture_digest.update(body);
    }
    let fixture_sha256 = format!("{:x}", fixture_digest.finalize());
    let server = Server::start(routes);
    let config = DownloadConfig {
        version_list_source: SourceMode::Official,
        file_source: SourceMode::Mirror,
        mirror_root: server.root(),
        max_parallel: 4,
        ..DownloadConfig::default()
    };
    let service = InstallService::new(config)?.with_manifest_url(format!(
        "{}/mc/game/version_manifest_v2.json",
        server.root()
    ));
    let game = root.join("game");
    let options = InstallOptions::for_current_platform(&game);
    let before_bytes = server.sent_bytes.load(Ordering::Relaxed);
    let before_requests = server.requests.load(Ordering::Relaxed);
    let cold_started = Instant::now();
    let cold = service.install("p7-fixture-1", &options, None).await?;
    let cold_ms = cold_started.elapsed().as_secs_f64() * 1000.0;
    let cold_bytes = server.sent_bytes.load(Ordering::Relaxed) - before_bytes;
    let cold_requests = server.requests.load(Ordering::Relaxed) - before_requests;

    let before_bytes = server.sent_bytes.load(Ordering::Relaxed);
    let before_requests = server.requests.load(Ordering::Relaxed);
    let warm_started = Instant::now();
    let warm = service.install("p7-fixture-1", &options, None).await?;
    let warm_ms = warm_started.elapsed().as_secs_f64() * 1000.0;
    let warm_bytes = server.sent_bytes.load(Ordering::Relaxed) - before_bytes;
    let warm_requests = server.requests.load(Ordering::Relaxed) - before_requests;
    let (logical_disk_bytes, disk_files) = directory_size(&game)?;
    if cold.downloaded_files == 0 || warm.reused_files < 4 || warm.downloaded_files != 0 {
        return Err(format!(
            "fixture did not exercise cold/warm cache: cold={cold:?}, warm={warm:?}"
        )
        .into());
    }
    let report = json!({
        "schema_version": 1,
        "benchmark": "vanilla_install_local_mirror_cold_warm",
        "fixture": "fixed 9 MiB client/library/asset objects, seeded xorshift payloads, locally served version metadata",
        "fixture_sha256": fixture_sha256,
        "fixture_payload_bytes": 9 * 1024 * 1024,
        "fixture_hashes_verified": true,
        "cold": {
            "elapsed_ms": cold_ms,
            "downloaded_files": cold.downloaded_files,
            "reused_files": cold.reused_files,
            "http_requests": cold_requests,
            "transferred_bytes": cold_bytes
        },
        "warm": {
            "elapsed_ms": warm_ms,
            "downloaded_files": warm.downloaded_files,
            "reused_files": warm.reused_files,
            "http_requests": warm_requests,
            "transferred_bytes": warm_bytes
        },
        "final_disk": {"logical_bytes": logical_disk_bytes, "files": disk_files},
        "peak_memory": peak_memory(),
        "runner_os": env::consts::OS,
        "runner_arch": env::consts::ARCH,
        "commit": env::var("GITHUB_SHA").unwrap_or_else(|_| "local".into()),
        "not_measured": ["public network throughput", "Minecraft runtime launch/readiness", "update executable replacement transaction", "allocated disk blocks"]
    });
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, serde_json::to_vec_pretty(&report)?)?;
    println!(
        "cold install {cold_ms:.1} ms ({cold_bytes} bytes); warm reuse {warm_ms:.1} ms ({})",
        warm.reused_files
    );
    Ok(())
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
