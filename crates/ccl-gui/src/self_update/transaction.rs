use super::{GuiManifest, current_identity, digest, error, is_hex};
use crate::{GuiError, GuiResult};
use ccl_core::update::Channel;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "macos", test))]
use std::path::Component;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const MAX_TREE: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;
const HEALTH_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize)]
pub struct TransactionStatus {
    pub phase: String,
    pub version: String,
    pub channel: Channel,
    pub has_backup: bool,
    pub last_error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema: u32,
    target: PathBuf,
    token: String,
    phase: String,
    manifest: Vec<u8>,
    signature: String,
    previous: BTreeMap<String, String>,
    last_error: Option<String>,
    #[serde(default)]
    registry: BTreeMap<String, Option<String>>,
}

/// A packaged installation and private updater state; game data lives elsewhere.
pub struct Installation {
    target: PathBuf,
    state: PathBuf,
}
impl Installation {
    pub fn current(state: &Path) -> GuiResult<Self> {
        #[cfg(not(target_os = "linux"))]
        let executable = std::env::current_exe().map_err(io)?;
        #[cfg(target_os="macos")]
        let target=executable.ancestors().find(|p|p.extension().is_some_and(|s|s=="app")).map(Path::to_path_buf).ok_or_else(||error("Self-update requires an installed .app bundle; this is a development executable"))?;
        #[cfg(target_os = "linux")]
        let target = std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .ok_or_else(|| {
                error(
                    "Self-update requires AppImage. For DEB installations use the package manager",
                )
            })?;
        #[cfg(target_os = "windows")]
        let target = {
            let root = executable
                .parent()
                .ok_or_else(|| error("Installation has no parent"))?
                .to_path_buf();
            if !root.join("uninstall.exe").is_file() {
                return Err(error("Self-update requires the per-user EXE installation"));
            }
            root
        };
        let target = target.canonicalize().map_err(io)?;
        let install = Self::at(target, state.to_path_buf())?;
        #[cfg(target_os = "macos")]
        if executable.canonicalize().map_err(io)? != install.executable() {
            return Err(error("Unexpected executable in application bundle"));
        }
        Ok(install)
    }
    fn at(target: PathBuf, state: PathBuf) -> GuiResult<Self> {
        if !target.is_absolute()
            || !state.is_absolute()
            || target.parent().is_none()
            || state.starts_with(&target)
            || target.starts_with(&state)
        {
            return Err(error(
                "Updater state must be outside the installed application",
            ));
        }
        safe_directory(&state)?;
        Ok(Self { target, state })
    }
    /// Explicit installations are available only to the separate acceptance tool.
    #[cfg(feature = "gui-validation")]
    pub fn validation_at(target: PathBuf, state: PathBuf) -> GuiResult<Self> {
        Self::at(target, state)
    }
    pub fn process_guard(&self) -> GuiResult<File> {
        let file = lock_file(&self.state.join("process.lock"))?;
        file.try_lock_exclusive().map_err(|_| {
            error("Another desktop instance or update helper holds this installation")
        })?;
        Ok(file)
    }
    fn executable(&self) -> PathBuf {
        executable_at(&self.target)
    }
    fn journal_path(&self) -> PathBuf {
        self.state.join("journal.json")
    }
    fn sibling(&self, journal: &Journal, suffix: &str) -> PathBuf {
        self.target
            .parent()
            .expect("validated parent")
            .join(format!(".ccl-gui-{}-{suffix}", journal.token))
    }
    fn backup(&self, journal: &Journal) -> PathBuf {
        self.sibling(journal, "previous")
    }
    fn load(&self) -> GuiResult<Journal> {
        let bytes = bounded_file(&self.journal_path(), 4 * 1024 * 1024)?;
        let journal: Journal = serde_json::from_slice(&bytes)
            .map_err(|_| error("Malformed desktop update journal"))?;
        if journal.schema != 1
            || journal.target != self.target
            || !is_hex(&journal.token, 32)
            || !matches!(
                journal.phase.as_str(),
                "staged"
                    | "replacing"
                    | "launching"
                    | "applied"
                    | "rollback_pending"
                    | "rolled_back"
                    | "recovery_required"
            )
        {
            return Err(error("Desktop journal binding mismatch"));
        }
        self.manifest(&journal)?;
        Ok(journal)
    }
    fn manifest(&self, journal: &Journal) -> GuiResult<GuiManifest> {
        ccl_core::update::verify_release_signature(&journal.manifest, &journal.signature)
            .map_err(|_| error("Journal manifest signature invalid"))?;
        let manifest: GuiManifest = serde_json::from_slice(&journal.manifest)
            .map_err(|_| error("Invalid journal manifest"))?;
        ccl_core::update::verify_release_signature_for_version(
            &journal.manifest,
            &journal.signature,
            &manifest.version,
        )
        .map_err(|_| error("Signed journal version mismatch"))?;
        manifest.validate(
            manifest.channel,
            &current_identity().target,
            &manifest.tag(),
        )?;
        Ok(manifest)
    }
    fn save(&self, journal: &Journal) -> GuiResult<()> {
        let bytes = serde_json::to_vec_pretty(journal)
            .map_err(|_| error("Journal serialization failed"))?;
        let mut temp = tempfile::NamedTempFile::new_in(&self.state).map_err(io)?;
        temp.write_all(&bytes).map_err(io)?;
        temp.as_file().sync_all().map_err(io)?;
        temp.persist(self.journal_path()).map_err(|e| io(e.error))?;
        sync_parent(&self.state)?;
        Ok(())
    }
    pub fn status(&self) -> GuiResult<Option<TransactionStatus>> {
        if !self.journal_path().exists() {
            return Ok(None);
        }
        let journal = self.load()?;
        let manifest = self.manifest(&journal)?;
        Ok(Some(TransactionStatus {
            phase: journal.phase.clone(),
            version: manifest.version,
            channel: manifest.channel,
            has_backup: self.backup(&journal).exists(),
            last_error: journal.last_error,
        }))
    }
    pub fn stage(
        &self,
        manifest: &GuiManifest,
        raw: &[u8],
        signature: &str,
        package: &[u8],
    ) -> GuiResult<()> {
        let _lock = self.transaction_lock()?;
        if self.journal_path().exists() {
            let old = self.load()?;
            if !matches!(old.phase.as_str(), "applied" | "rolled_back") {
                return Err(error(
                    "An interrupted desktop update requires recovery first",
                ));
            }
            remove(&self.sibling(&old, "next"))?;
        }
        ccl_core::update::verify_release_signature(raw, signature)
            .map_err(|_| error("Manifest signature invalid"))?;
        let parsed: GuiManifest =
            serde_json::from_slice(raw).map_err(|_| error("Invalid manifest"))?;
        ccl_core::update::verify_release_signature_for_version(raw, signature, &parsed.version)
            .map_err(|_| error("Signed package version mismatch"))?;
        if serde_json::to_value(&parsed).ok() != serde_json::to_value(manifest).ok() {
            return Err(error("Manifest changed after verification"));
        }
        manifest.validate(
            manifest.channel,
            &current_identity().target,
            &manifest.tag(),
        )?;
        if digest(package) != manifest.asset.sha256 || package.len() as u64 != manifest.asset.size {
            return Err(error("Staged package digest mismatch"));
        }
        // Check write access before the running application agrees to exit.
        let probe = tempfile::NamedTempFile::new_in(
            self.target.parent().expect("validated parent"),
        )
        .map_err(|_| {
            error("Application directory is not writable; install in a user-writable location")
        })?;
        drop(probe);
        let previous = snapshot(&self.target)?;
        let token = digest(
            format!(
                "{}:{}:{:?}",
                std::process::id(),
                manifest.commit,
                std::time::SystemTime::now()
            )
            .as_bytes(),
        )[..32]
            .to_string();
        let journal = Journal {
            schema: 1,
            target: self.target.clone(),
            token: token.clone(),
            phase: "staged".into(),
            manifest: raw.to_vec(),
            signature: signature.into(),
            previous,
            last_error: None,
            #[cfg(target_os = "windows")]
            registry: windows_registry::capture(&self.state, &token)?,
            #[cfg(not(target_os = "windows"))]
            registry: BTreeMap::new(),
        };
        atomic_file(&self.state.join("payload"), package)?;
        let helper = self.state.join(helper_name());
        #[cfg(target_os = "linux")]
        let source = self.target.clone();
        #[cfg(not(target_os = "linux"))]
        let source = std::env::current_exe().map_err(io)?;
        let helper_bytes = bounded_file(&source, 512 * 1024 * 1024)?;
        atomic_file(&helper, &helper_bytes)?;
        executable_permission(&helper)?;
        // Preserve the prior completed journal and its rollback point until this
        // transaction is healthy. A failed staging write must not destroy it.
        if self.journal_path().exists() {
            atomic_file(
                &self.state.join("prior-journal.json"),
                &bounded_file(&self.journal_path(), 4 * 1024 * 1024)?,
            )?;
        } else {
            remove(&self.state.join("prior-journal.json"))?;
        }
        self.save(&journal)
    }
    pub fn prepare(&self, operation: &str) -> GuiResult<()> {
        let _lock = self.transaction_lock()?;
        let mut journal = self.load()?;
        if operation == "rollback" {
            if journal.phase != "applied" || !self.backup(&journal).exists() {
                return Err(error("No completed desktop update has a rollback backup"));
            }
            if snapshot(&self.backup(&journal))? != journal.previous {
                return Err(error("Rollback backup integrity check failed"));
            }
            journal.phase = "rollback_pending".into();
            self.save(&journal)?;
        } else if !matches!(
            journal.phase.as_str(),
            "staged" | "replacing" | "launching" | "rollback_pending" | "recovery_required"
        ) {
            return Err(error("No interrupted desktop update requires recovery"));
        }
        Ok(())
    }
    pub fn launch(&self, operation: &str) -> GuiResult<u32> {
        self.load()?;
        let helper = self.state.join(helper_name());
        if !helper.is_file() {
            return Err(error("Desktop update helper is missing"));
        }
        let child = Command::new(helper)
            .arg("--ccl-gui-update-helper")
            .arg(&self.state)
            .arg(operation)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(io)?;
        Ok(child.id())
    }
    fn transaction_lock(&self) -> GuiResult<File> {
        let file = lock_file(&self.state.join("transaction.lock"))?;
        file.try_lock_exclusive()
            .map_err(|_| error("Another desktop update is in progress"))?;
        Ok(file)
    }
    fn wait_process_exit(&self) -> GuiResult<File> {
        let file = lock_file(&self.state.join("process.lock"))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if file.try_lock_exclusive().is_ok() {
                return Ok(file);
            }
            if Instant::now() > deadline {
                return Err(error(
                    "Running desktop did not exit; no application files changed",
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    fn apply(&self, journal: &mut Journal) -> GuiResult<()> {
        let manifest = self.manifest(journal)?;
        let package = bounded_file(&self.state.join("payload"), super::MAX_PACKAGE)?;
        if digest(&package) != manifest.asset.sha256 || package.len() as u64 != manifest.asset.size
        {
            return Err(error(
                "Downloaded package no longer matches signed manifest",
            ));
        }
        if snapshot(&self.target)? != journal.previous {
            return Err(error("Installed application changed since update staging"));
        }
        let next = self.sibling(journal, "next");
        let backup = self.backup(journal);
        if next.exists() || backup.exists() {
            return Err(error("Desktop staging paths already exist; recover first"));
        }
        #[cfg(target_os = "macos")]
        extract_app(&package, &next)?;
        #[cfg(target_os = "linux")]
        {
            atomic_file(&next, &package)?;
            executable_permission(&next)?;
        }
        #[cfg(target_os = "windows")]
        copy_tree(&self.target, &backup)?;
        journal.phase = "replacing".into();
        self.save(journal)?;
        #[cfg(not(target_os = "windows"))]
        {
            fs::rename(&self.target, &backup).map_err(io)?;
            sync_parent(self.target.parent().expect("validated parent"))?;
            fs::rename(&next, &self.target).map_err(io)?;
        }
        #[cfg(target_os = "windows")]
        {
            let installer = self.state.join("installer.exe");
            atomic_file(&installer, &package)?;
            let mut child = Command::new(&installer)
                .arg("/S")
                .arg("/UPDATE")
                .arg(format!("/D={}", self.target.display()))
                .spawn()
                .map_err(io)?;
            let deadline = Instant::now() + Duration::from_secs(120);
            loop {
                if let Some(status) = child.try_wait().map_err(io)? {
                    if !status.success() {
                        return Err(error("Desktop installer failed"));
                    }
                    break;
                }
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error("Desktop installer timed out"));
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
        journal.phase = "launching".into();
        self.save(journal)
    }
    fn restore(&self, journal: &mut Journal) -> GuiResult<()> {
        let backup = self.backup(journal);
        if !backup.exists() {
            if snapshot(&self.target)? == journal.previous {
                remove(&self.sibling(journal, "next"))?;
                #[cfg(target_os = "windows")]
                windows_registry::restore(&self.state, &journal.token, &journal.registry)?;
                journal.phase = "rolled_back".into();
                return self.save(journal);
            }
            return Err(error(
                "Backup unavailable; retain journal for manual recovery",
            ));
        }
        if snapshot(&backup)? != journal.previous {
            return Err(error("Rollback backup integrity check failed"));
        }
        journal.phase = "rollback_pending".into();
        self.save(journal)?;
        remove(&self.target)?;
        fs::rename(&backup, &self.target).map_err(io)?;
        if snapshot(&self.target)? != journal.previous {
            return Err(error("Restored application integrity check failed"));
        }
        #[cfg(target_os = "windows")]
        windows_registry::restore(&self.state, &journal.token, &journal.registry)?;
        remove(&self.sibling(journal, "next"))?;
        journal.phase = "rolled_back".into();
        self.save(journal)
    }
    fn finish_prior(&self, committed: bool, last_error: Option<String>) -> GuiResult<()> {
        let path = self.state.join("prior-journal.json");
        if !path.exists() {
            return Ok(());
        }
        let bytes = bounded_file(&path, 4 * 1024 * 1024)?;
        let mut prior: Journal = serde_json::from_slice(&bytes)
            .map_err(|_| error("Malformed retained update journal"))?;
        if prior.target != self.target
            || !is_hex(&prior.token, 32)
            || !matches!(prior.phase.as_str(), "applied" | "rolled_back")
        {
            return Err(error("Retained journal binding mismatch"));
        }
        self.manifest(&prior)?;
        if committed {
            remove(&self.backup(&prior))?;
            #[cfg(target_os = "windows")]
            windows_registry::cleanup(&self.state, &prior.token)?;
        } else {
            prior.last_error = last_error;
            self.save(&prior)?;
        }
        remove(&path)
    }
    fn spawn_application(&self, health: Option<&Journal>) -> GuiResult<Child> {
        let mut cmd = Command::new(self.executable());
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // AppImage launchers must derive their own mount path.
        cmd.env_remove("APPIMAGE")
            .env_remove("APPDIR")
            .env_remove("ARGV0");
        if let Some(journal) = health {
            cmd.env("CCL_GUI_UPDATE_HEALTH_STATE", &self.state)
                .env("CCL_GUI_UPDATE_HEALTH_TOKEN", &journal.token);
        } else {
            cmd.env_remove("CCL_GUI_UPDATE_HEALTH_STATE")
                .env_remove("CCL_GUI_UPDATE_HEALTH_TOKEN");
        }
        cmd.spawn().map_err(io)
    }
    fn health_check(&self, journal: &Journal, child: &mut Child) -> GuiResult<()> {
        let marker = self.state.join(format!("healthy-{}", journal.token));
        let deadline = Instant::now() + HEALTH_TIMEOUT;
        loop {
            if marker.exists() {
                let content = bounded_file(&marker, 128)?;
                if content == journal.token.as_bytes() {
                    return Ok(());
                }
                return Err(error("Invalid startup health marker"));
            }
            if child.try_wait().map_err(io)?.is_some() {
                return Err(error(
                    "Updated application exited before its UI became ready",
                ));
            }
            if Instant::now() > deadline {
                return Err(error(
                    "Updated application did not report UI readiness within 60 seconds",
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Handle the private helper invocation before loading Tauri or creating windows.
pub fn helper_entry() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_none_or(|a| a != "--ccl-gui-update-helper") {
        return None;
    }
    Some(if args.len() == 4 {
        helper_run(Path::new(&args[2]), &args[3].to_string_lossy()).map_or(1, |_| 0)
    } else {
        2
    })
}
fn helper_run(state: &Path, operation: &str) -> GuiResult<()> {
    let result = helper_run_inner(state, operation);
    if let Err(failure) = &result {
        // Keep unexpected I/O/locking failures visible on the next startup.
        // Revalidate the signed journal before writing any recovery status.
        let record = (|| -> GuiResult<()> {
            let raw = bounded_file(&state.join("journal.json"), 4 * 1024 * 1024)?;
            let initial: Journal =
                serde_json::from_slice(&raw).map_err(|_| error("Invalid helper journal"))?;
            let install = Installation::at(initial.target, state.to_path_buf())?;
            let _lock = install.transaction_lock()?;
            let mut journal = install.load()?;
            journal.last_error = Some(failure.message.clone());
            if !matches!(journal.phase.as_str(), "applied" | "rolled_back") {
                journal.phase = "recovery_required".into();
            }
            install.save(&journal)
        })();
        let _ = record;
    }
    result
}

fn helper_run_inner(state: &Path, operation: &str) -> GuiResult<()> {
    if !matches!(operation, "apply" | "rollback" | "recover") {
        return Err(error("Invalid helper operation"));
    }
    let bytes = bounded_file(&state.join("journal.json"), 4 * 1024 * 1024)?;
    let initial: Journal =
        serde_json::from_slice(&bytes).map_err(|_| error("Invalid helper journal"))?;
    let install = Installation::at(initial.target, state.to_path_buf())?;
    let _transaction = install.transaction_lock()?;
    let mut journal = install.load()?;
    let process = install.wait_process_exit()?;
    if operation == "apply" && journal.phase == "staged" {
        if let Err(e) = install.apply(&mut journal) {
            journal.last_error = Some(e.message);
            if journal.phase == "staged" {
                journal.phase = "recovery_required".into();
                install.save(&journal)?;
                install.finish_prior(false, journal.last_error.clone())?;
                drop(process);
                install.spawn_application(None)?;
                return Err(error(
                    "Update staging failed; original application retained and restarted",
                ));
            }
            install.restore(&mut journal)?;
            install.finish_prior(false, journal.last_error.clone())?;
            drop(process);
            install.spawn_application(None)?;
            return Err(error("Update failed and previous application was restored"));
        }
        drop(process);
        let mut child = match install.spawn_application(Some(&journal)) {
            Ok(child) => child,
            Err(e) => {
                journal.last_error = Some(e.message);
                let _guard = install.wait_process_exit()?;
                install.restore(&mut journal)?;
                install.finish_prior(false, journal.last_error.clone())?;
                drop(_guard);
                install.spawn_application(None)?;
                return Err(error(
                    "Updated application could not start; previous version restored",
                ));
            }
        };
        match install.health_check(&journal, &mut child) {
            Ok(()) => {
                journal.phase = "applied".into();
                journal.last_error = None;
                install.save(&journal)?;
                install.finish_prior(true, None)?;
            }
            Err(e) => {
                journal.last_error = Some(e.message);
                let _ = child.kill();
                let _ = child.wait();
                let guard = install.wait_process_exit()?;
                install.restore(&mut journal)?;
                install.finish_prior(false, journal.last_error.clone())?;
                drop(guard);
                install.spawn_application(None)?;
            }
        }
    } else {
        // Interrupted replacement always restores the known previous tree; never
        // silently resumes an unacknowledged candidate installation.
        install.restore(&mut journal)?;
        install.finish_prior(false, journal.last_error.clone())?;
        drop(process);
        install.spawn_application(None)?;
    }
    Ok(())
}

pub fn acknowledge_health() -> GuiResult<()> {
    let Some(state) = std::env::var_os("CCL_GUI_UPDATE_HEALTH_STATE") else {
        return Ok(());
    };
    let token =
        std::env::var("CCL_GUI_UPDATE_HEALTH_TOKEN").map_err(|_| error("Health token missing"))?;
    if !is_hex(&token, 32) {
        return Err(error("Invalid health token"));
    }
    let install = Installation::current(Path::new(&state))?;
    let journal = install.load()?;
    let manifest = install.manifest(&journal)?;
    let current = current_identity();
    if journal.token != token
        || journal.phase != "launching"
        || manifest.commit != current.commit
        || manifest.version != current.version
        || manifest.channel != current.channel
    {
        return Err(error("Health identity does not match the staged update"));
    }
    atomic_file(
        &install.state.join(format!("healthy-{token}")),
        token.as_bytes(),
    )
}

fn helper_name() -> &'static str {
    if cfg!(windows) {
        "helper.exe"
    } else if cfg!(target_os = "linux") {
        "helper.AppImage"
    } else {
        "helper"
    }
}
fn executable_at(target: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        target.join("Contents/MacOS/ccl-gui")
    } else if cfg!(windows) {
        target.join("ccl-gui.exe")
    } else {
        target.to_path_buf()
    }
}
fn safe_directory(path: &Path) -> GuiResult<()> {
    fs::create_dir_all(path).map_err(io)?;
    for part in path.ancestors() {
        if fs::symlink_metadata(part)
            .map_err(io)?
            .file_type()
            .is_symlink()
        {
            return Err(error("Updater state path contains a symbolic link"));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io)?;
    }
    Ok(())
}
fn bounded_file(path: &Path, max: u64) -> GuiResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(io)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > max {
        return Err(error("Expected bounded regular updater file"));
    }
    let mut output = Vec::new();
    File::open(path)
        .map_err(io)?
        .take(max + 1)
        .read_to_end(&mut output)
        .map_err(io)?;
    if output.len() as u64 > max {
        return Err(error("Updater file grew past limit"));
    }
    Ok(output)
}
fn atomic_file(path: &Path, bytes: &[u8]) -> GuiResult<()> {
    let mut temp =
        tempfile::NamedTempFile::new_in(path.parent().ok_or_else(|| error("No parent"))?)
            .map_err(io)?;
    temp.write_all(bytes).map_err(io)?;
    temp.as_file().sync_all().map_err(io)?;
    temp.persist(path).map_err(|e| io(e.error))?;
    Ok(())
}
fn lock_file(path: &Path) -> GuiResult<File> {
    if path.exists()
        && fs::symlink_metadata(path)
            .map_err(io)?
            .file_type()
            .is_symlink()
    {
        return Err(error("Updater lock is a symbolic link"));
    }
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(io)
}
fn sync_parent(path: &Path) -> GuiResult<()> {
    #[cfg(unix)]
    File::open(path).map_err(io)?.sync_all().map_err(io)?;
    let _ = path;
    Ok(())
}
fn executable_permission(path: &Path) -> GuiResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(io)?;
    }
    let _ = path;
    Ok(())
}
fn remove(path: &Path) -> GuiResult<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => fs::remove_dir_all(path).map_err(io),
        Ok(_) => fs::remove_file(path).map_err(io),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(e)),
    }
}
fn snapshot(path: &Path) -> GuiResult<BTreeMap<String, String>> {
    fn walk(
        root: &Path,
        path: &Path,
        out: &mut BTreeMap<String, String>,
        size: &mut u64,
    ) -> GuiResult<()> {
        let metadata = fs::symlink_metadata(path).map_err(io)?;
        if metadata.file_type().is_symlink() {
            return Err(error(
                "Application trees containing symbolic links cannot be updated",
            ));
        }
        if out.len() >= MAX_ENTRIES {
            return Err(error("Application tree entry limit exceeded"));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| error("Invalid tree path"))?
            .to_string_lossy()
            .replace('\\', "/");
        if metadata.is_dir() {
            out.insert(relative, "directory".into());
            for entry in fs::read_dir(path).map_err(io)? {
                walk(root, &entry.map_err(io)?.path(), out, size)?;
            }
        } else if metadata.is_file() {
            *size = size
                .checked_add(metadata.len())
                .ok_or_else(|| error("Application size overflow"))?;
            if *size > MAX_TREE {
                return Err(error("Application tree too large"));
            }
            out.insert(relative, digest(&bounded_file(path, MAX_TREE)?));
        } else {
            return Err(error("Unsupported application filesystem entry"));
        }
        Ok(())
    }
    let mut output = BTreeMap::new();
    walk(path, path, &mut output, &mut 0)?;
    Ok(output)
}
#[cfg(target_os = "windows")]
fn copy_tree(source: &Path, destination: &Path) -> GuiResult<()> {
    let _ = snapshot(source)?;
    fs::create_dir(destination).map_err(io)?;
    for entry in fs::read_dir(source).map_err(io)? {
        let entry = entry.map_err(io)?;
        let target = destination.join(entry.file_name());
        if entry.file_type().map_err(io)?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target).map_err(io)?;
        }
    }
    Ok(())
}
#[cfg(any(target_os = "macos", test))]
fn extract_app(bytes: &[u8], destination: &Path) -> GuiResult<()> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    fs::create_dir(destination).map_err(io)?;
    let mut total = 0u64;
    let mut seen = std::collections::BTreeSet::new();
    let mut root = None;
    for (index, entry) in archive.entries().map_err(io)?.enumerate() {
        if index >= MAX_ENTRIES {
            return Err(error("App archive entry limit exceeded"));
        }
        let mut entry = entry.map_err(io)?;
        let path = entry.path().map_err(io)?.into_owned();
        let parts: Vec<_> = path.components().collect();
        if parts.is_empty()
            || parts.len() > 32
            || parts.iter().any(|p| !matches!(p, Component::Normal(_)))
        {
            return Err(error("App archive contains an unsafe path"));
        }
        let Component::Normal(first) = parts[0] else {
            unreachable!()
        };
        if Path::new(first).extension().is_none_or(|e| e != "app") {
            return Err(error("Expected one application bundle"));
        }
        if root.as_ref().is_some_and(|old| old != first) {
            return Err(error("Archive contains multiple application bundles"));
        }
        root = Some(first.to_os_string());
        let relative: PathBuf = parts.iter().skip(1).map(|p| p.as_os_str()).collect();
        if relative.as_os_str().is_empty() {
            if !entry.header().entry_type().is_dir() {
                return Err(error("Application root is not a directory"));
            }
            continue;
        }
        if !seen.insert(relative.clone()) {
            return Err(error("Duplicate application archive path"));
        }
        let target = destination.join(&relative);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            fs::create_dir_all(target).map_err(io)?;
            continue;
        }
        if !kind.is_file() {
            return Err(error(
                "Links and special files are not allowed in update archives",
            ));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| error("Archive size overflow"))?;
        if total > MAX_TREE {
            return Err(error("Expanded application exceeds size limit"));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io)?;
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&target)
            .map_err(io)?;
        std::io::copy(&mut entry, &mut file).map_err(io)?;
        file.sync_all().map_err(io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = entry.header().mode().map_err(io)?;
            fs::set_permissions(
                &target,
                fs::Permissions::from_mode(if mode & 0o111 != 0 { 0o755 } else { 0o644 }),
            )
            .map_err(io)?;
        }
    }
    if !destination.join("Contents/MacOS/ccl-gui").is_file()
        || !destination.join("Contents/Info.plist").is_file()
    {
        return Err(error("Incomplete application bundle"));
    }
    Ok(())
}
fn io(e: std::io::Error) -> GuiError {
    error(format!("Desktop update I/O failed: {e}"))
}

#[cfg(test)]
mod tests;

#[cfg(target_os = "windows")]
mod windows_registry;
