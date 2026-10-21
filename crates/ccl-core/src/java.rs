//! Java runtime discovery and inspection.

use serde::Serialize;
use std::{
    collections::HashSet,
    env,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const JAVA_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Serialize)]
pub struct JavaRuntime {
    pub executable: PathBuf,
    pub major: u32,
    pub architecture: String,
    pub version: String,
}

#[derive(Debug, thiserror::Error)]
pub enum JavaError {
    #[error("Java executable was not found at {0}")]
    NotFound(PathBuf),
    #[error("could not run Java at {path}: {source}")]
    Probe {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Java at {path} returned an unsuccessful status: {detail}")]
    ProbeFailed { path: PathBuf, detail: String },
    #[error("Java probe at {0} timed out after 5 seconds")]
    Timeout(PathBuf),
    #[error("could not determine Java version from output of {0}")]
    InvalidOutput(PathBuf),
    #[error("Java at {path} has major version {actual}; version {required} is required")]
    VersionMismatch {
        path: PathBuf,
        actual: u32,
        required: u32,
    },
    #[error("Java at {path} has architecture {actual}; host architecture {host} is required")]
    ArchitectureMismatch {
        path: PathBuf,
        actual: String,
        host: String,
    },
    #[error("no installed Java runtime matching major version {0} was found")]
    NoCompatibleRuntime(u32),
}

/// Selects a runtime. An explicit executable or Java home is authoritative: a
/// bad explicit selection is reported instead of silently choosing another JVM.
pub fn detect_java(explicit: Option<&Path>, required_major: u32) -> Result<JavaRuntime, JavaError> {
    if let Some(path) = explicit {
        let runtime = inspect_java(&resolve_executable(path))?;
        let runtime = require_major(runtime, required_major)?;
        return require_host_architecture(runtime);
    }
    for path in discover_java() {
        if let Ok(runtime) = inspect_java(&path)
            && runtime.major == required_major
            && architectures_match(&runtime.architecture, env::consts::ARCH)
        {
            return Ok(runtime);
        }
    }
    Err(JavaError::NoCompatibleRuntime(required_major))
}

/// Runs Java's own property report and returns its runtime metadata.
pub fn inspect_java(path: &Path) -> Result<JavaRuntime, JavaError> {
    let executable = resolve_executable(path);
    if !executable.is_file() {
        return Err(JavaError::NotFound(executable));
    }
    let canonical = std::fs::canonicalize(&executable).unwrap_or(executable);
    let mut child = Command::new(&canonical)
        .args(["-XshowSettings:properties", "-version"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| JavaError::Probe {
            path: canonical.clone(),
            source,
        })?;
    let started = Instant::now();
    loop {
        match child.try_wait().map_err(|source| JavaError::Probe {
            path: canonical.clone(),
            source,
        })? {
            Some(_) => break,
            None if started.elapsed() >= JAVA_PROBE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(JavaError::Timeout(canonical));
            }
            None => thread::sleep(Duration::from_millis(50)),
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|source| JavaError::Probe {
            path: canonical.clone(),
            source,
        })?;
    let report = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(JavaError::ProbeFailed {
            path: canonical,
            detail: report.trim().to_owned(),
        });
    }
    parse_java_properties(&report, canonical)
}

/// Lists likely Java executables, preserving discovery order and removing duplicates.
pub fn discover_java() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(home) = env::var_os("JAVA_HOME") {
        candidates.push(resolve_executable(&PathBuf::from(home)));
    }
    if let Some(path) = find_on_path("java") {
        candidates.push(path);
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("/usr/libexec/java_home").arg("-V").output() {
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            for line in text.lines() {
                if let Some(home) = mac_java_home_from_line(line) {
                    candidates.push(resolve_executable(Path::new(&home)));
                }
            }
        }
        add_java_dirs(
            &mut candidates,
            Path::new("/Library/Java/JavaVirtualMachines"),
        );
        if let Some(home) = env::var_os("HOME") {
            add_java_dirs(
                &mut candidates,
                &PathBuf::from(home).join("Library/Java/JavaVirtualMachines"),
            );
        }
    }
    #[cfg(target_os = "linux")]
    add_java_dirs(&mut candidates, Path::new("/usr/lib/jvm"));
    #[cfg(target_os = "windows")]
    {
        for root in [
            env::var_os("ProgramFiles").map(PathBuf::from),
            env::var_os("ProgramFiles(x86)").map(PathBuf::from),
        ]
        .into_iter()
        .flatten()
        {
            add_java_dirs(&mut candidates, &root.join("Java"));
            add_java_dirs(&mut candidates, &root.join("Eclipse Adoptium"));
        }
    }

    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter_map(|path| {
            if !path.is_file() {
                return None;
            }
            let canonical = std::fs::canonicalize(&path).unwrap_or(path);
            seen.insert(canonical.clone()).then_some(canonical)
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn mac_java_home_from_line(line: &str) -> Option<String> {
    let start = line.find('/')?;
    let home = line[start..]
        .trim()
        .trim_matches('"')
        .trim_end_matches('"')
        .trim();
    (!home.is_empty()).then(|| home.to_owned())
}

fn require_major(runtime: JavaRuntime, required: u32) -> Result<JavaRuntime, JavaError> {
    if runtime.major != required {
        return Err(JavaError::VersionMismatch {
            path: runtime.executable,
            actual: runtime.major,
            required,
        });
    }
    Ok(runtime)
}

fn require_host_architecture(runtime: JavaRuntime) -> Result<JavaRuntime, JavaError> {
    if !architectures_match(&runtime.architecture, env::consts::ARCH) {
        return Err(JavaError::ArchitectureMismatch {
            path: runtime.executable,
            actual: runtime.architecture,
            host: env::consts::ARCH.to_owned(),
        });
    }
    Ok(runtime)
}

fn architectures_match(java: &str, host: &str) -> bool {
    fn family(value: &str) -> Option<&'static str> {
        match value.to_ascii_lowercase().as_str() {
            "amd64" | "x86_64" => Some("x64"),
            "arm64" | "aarch64" => Some("arm64"),
            "i386" | "i686" | "x86" => Some("x86"),
            _ => None,
        }
    }
    family(java).is_some() && family(java) == family(host)
}

fn resolve_executable(path: &Path) -> PathBuf {
    if path.is_dir() {
        java_in_home(path)
    } else {
        path.to_path_buf()
    }
}

fn java_in_home(home: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    let name = "java.exe";
    #[cfg(not(target_os = "windows"))]
    let name = "java";
    home.join("bin").join(name)
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let executable = if cfg!(target_os = "windows") && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(&executable))
        .find(|p| p.is_file())
}

fn add_java_dirs(out: &mut Vec<PathBuf>, root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if dir.is_dir() {
            out.push(resolve_executable(&dir));
            #[cfg(target_os = "macos")]
            out.push(resolve_executable(&dir.join("Contents/Home")));
        }
    }
}

fn parse_java_properties(report: &str, executable: PathBuf) -> Result<JavaRuntime, JavaError> {
    let mut version = None;
    let mut specification = None;
    let mut architecture = None;
    for line in report.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_owned();
        match key.trim() {
            "java.version" => version = Some(value),
            "java.specification.version" => specification = Some(value),
            "os.arch" => architecture = Some(value),
            _ => {}
        }
    }
    let version = version.ok_or_else(|| JavaError::InvalidOutput(executable.clone()))?;
    let major = specification
        .as_deref()
        .and_then(parse_major)
        .or_else(|| parse_major(&version))
        .ok_or_else(|| JavaError::InvalidOutput(executable.clone()))?;
    Ok(JavaRuntime {
        executable,
        major,
        architecture: architecture.unwrap_or_else(|| "unknown".into()),
        version,
    })
}

fn parse_major(value: &str) -> Option<u32> {
    let first = value.strip_prefix("1.").unwrap_or(value);
    first.split(['.', '_', '+', '-']).next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_legacy_and_modern_versions() {
        for (version, spec, expected) in [
            ("1.8.0_402", "1.8", 8),
            ("17.0.12", "17", 17),
            ("25-ea", "25", 25),
        ] {
            let report = format!(
                "    java.version = {version}\n    java.specification.version = {spec}\n    os.arch = aarch64\n"
            );
            let runtime = parse_java_properties(&report, PathBuf::from("java")).unwrap();
            assert_eq!(runtime.major, expected);
            assert_eq!(runtime.version, version);
            assert_eq!(runtime.architecture, "aarch64");
        }
    }

    #[test]
    fn recognizes_java_architecture_aliases() {
        for (java, host) in [
            ("amd64", "x86_64"),
            ("x86_64", "amd64"),
            ("arm64", "aarch64"),
            ("aarch64", "arm64"),
            ("i386", "x86"),
            ("i686", "x86"),
            ("x86", "i686"),
        ] {
            assert!(architectures_match(java, host), "{java} vs {host}");
        }
        assert!(!architectures_match("aarch64", "x86_64"));
        assert!(!architectures_match("unknown", "x86_64"));
    }

    #[cfg(unix)]
    #[test]
    fn accepts_executable_path_with_spaces() {
        use std::{fs, os::unix::fs::PermissionsExt};
        let path = std::env::temp_dir().join(format!("java path {}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        let java = path.join("java fake");
        fs::write(&java, "#!/bin/sh\nprintf '%s\\n' '    java.version = 17.0.1' '    java.specification.version = 17' '    os.arch = x86_64' >&2\n").unwrap();
        fs::set_permissions(&java, fs::Permissions::from_mode(0o755)).unwrap();
        let runtime = inspect_java(&java).unwrap();
        assert_eq!(runtime.executable, fs::canonicalize(&java).unwrap());
        let _ = fs::remove_dir_all(path);
    }
}
