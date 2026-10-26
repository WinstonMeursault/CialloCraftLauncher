//! Bounded launcher diagnostics. Payloads, credentials and game output are never logged.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: u64 = 5 * 1024 * 1024;
const FILES: usize = 5;
const READ_BYTES: u64 = 256 * 1024;
static GLOBAL: OnceLock<LauncherLog> = OnceLock::new();

#[derive(Clone)]
pub struct LauncherLog {
    directory: PathBuf,
    lock: Arc<Mutex<bool>>,
}
#[derive(Serialize)]
pub struct LogSnapshot {
    pub text: String,
    pub path: PathBuf,
    pub debug: bool,
}
#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LogPreferences {
    debug: bool,
}

fn regular(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_file() && !m.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(std::io::Error::other(
            "Launcher log path is not a regular file",
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

impl LauncherLog {
    pub fn new(directory: PathBuf) -> std::io::Result<Self> {
        if let Ok(metadata) = fs::symlink_metadata(&directory)
            && (!metadata.is_dir() || metadata.file_type().is_symlink())
        {
            return Err(std::io::Error::other("Launcher log directory is invalid"));
        }
        fs::create_dir_all(&directory)?;
        let prefs = directory.join("preferences.json");
        regular(&prefs)?;
        let debug = fs::metadata(&prefs)
            .ok()
            .filter(|metadata| metadata.len() <= 1024)
            .and_then(|_| fs::read(&prefs).ok())
            .and_then(|bytes| serde_json::from_slice::<LogPreferences>(&bytes).ok())
            .is_some_and(|p| p.debug);
        Ok(Self {
            directory,
            lock: Arc::new(Mutex::new(debug)),
        })
    }

    fn path(&self, index: usize) -> PathBuf {
        self.directory.join(if index == 0 {
            "launcher.log".into()
        } else {
            format!("launcher.{index}.log")
        })
    }

    pub fn record(
        &self,
        level: &str,
        event: &str,
        code: Option<&str>,
        task: Option<u64>,
    ) -> std::io::Result<()> {
        let debug = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if level == "DEBUG" && !*debug {
            return Ok(());
        }
        // Never accept arbitrary messages, URLs, account names or request bodies.
        let token = |s: &str| {
            s.chars()
                .filter(|c| c.is_ascii_alphanumeric() || "_.-".contains(*c))
                .take(64)
                .collect::<String>()
        };
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let line = format!(
            "{time} {} {}{}{}\n",
            token(level),
            token(event),
            code.map(|v| format!(" code={}", token(v)))
                .unwrap_or_default(),
            task.map(|v| format!(" task={v}")).unwrap_or_default()
        );
        let path = self.path(0);
        regular(&path)?;
        if fs::metadata(&path).is_ok_and(|m| m.len() + line.len() as u64 > MAX_BYTES) {
            for i in (1..FILES).rev() {
                let from = self.path(i - 1);
                let to = self.path(i);
                regular(&from)?;
                regular(&to)?;
                if to.exists() {
                    fs::remove_file(&to)?;
                }
                if from.exists() {
                    fs::rename(from, to)?;
                }
            }
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?
            .write_all(line.as_bytes())
    }

    pub fn snapshot(&self, index: usize) -> std::io::Result<LogSnapshot> {
        if index >= FILES {
            return Err(std::io::Error::other("Invalid launcher log selection"));
        }
        let debug = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let path = self.path(index);
        regular(&path)?;
        let text = match fs::File::open(&path) {
            Ok(mut file) => {
                let offset = file.metadata()?.len().saturating_sub(READ_BYTES);
                file.seek(SeekFrom::Start(offset))?;
                let mut bytes = Vec::new();
                file.take(READ_BYTES).read_to_end(&mut bytes)?;
                let text = String::from_utf8_lossy(&bytes);
                let text = if offset > 0 {
                    text.split_once('\n').map_or("", |(_, tail)| tail)
                } else {
                    &text
                };
                text.lines()
                    .rev()
                    .take(1000)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        Ok(LogSnapshot {
            text,
            path,
            debug: *debug,
        })
    }

    pub fn set_debug(&self, enabled: bool) -> std::io::Result<()> {
        let mut debug = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let path = self.directory.join("preferences.json");
        regular(&path)?;
        fs::write(
            path,
            serde_json::to_vec(&LogPreferences { debug: enabled })?,
        )?;
        *debug = enabled;
        Ok(())
    }
}

pub fn initialize(logger: LauncherLog) {
    let _ = GLOBAL.set(logger);
}
pub fn record(level: &str, event: &str, code: Option<&str>, task: Option<u64>) {
    if let Some(log) = GLOBAL.get() {
        let _ = log.record(level, event, code, task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotation_read_bounds_and_debug_preferences() {
        let temp = tempfile::tempdir().unwrap();
        let log = LauncherLog::new(temp.path().join("logs")).unwrap();
        log.record("DEBUG", "navigation", None, None).unwrap();
        assert!(log.snapshot(0).unwrap().text.is_empty());
        log.set_debug(true).unwrap();
        fs::write(log.path(0), vec![b'x'; MAX_BYTES as usize]).unwrap();
        log.record("INFO", "task.failed", Some("invalid_launch_key"), Some(42))
            .unwrap();
        assert!(
            log.snapshot(0)
                .unwrap()
                .text
                .contains("invalid_launch_key task=42")
        );
        assert!(log.snapshot(1).unwrap().text.len() <= READ_BYTES as usize);
        assert!(
            LauncherLog::new(temp.path().join("logs"))
                .unwrap()
                .snapshot(0)
                .unwrap()
                .debug
        );
        assert!(log.snapshot(FILES).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_log_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let log = LauncherLog::new(temp.path().join("logs")).unwrap();
        let outside = temp.path().join("untouched");
        fs::write(&outside, "keep").unwrap();
        std::os::unix::fs::symlink(&outside, log.path(0)).unwrap();
        assert!(log.record("INFO", "startup", None, None).is_err());
        assert!(log.snapshot(0).is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "keep");
    }
}
