//! Fixed-key HKCU snapshot support for restoring NSIS uninstall metadata.
//!
//! Every operation is limited to the two product keys below. Export files live
//! in the updater's private state directory and are validated before import.

use super::{GuiResult, digest, error, io, is_hex};
use std::os::windows::ffi::OsStrExt;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const MAX_EXPORT: u64 = 1024 * 1024;
const KEYS: [(&str, &str); 2] = [
    (
        "launcher_uninstall",
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\CialloCraftLauncher",
    ),
    (
        "publisher_product",
        r"HKCU\Software\WinstonMeursault\CialloCraftLauncher",
    ),
];

pub type RegistrySnapshot = BTreeMap<String, Option<String>>;

fn export_path(state: &Path, token: &str, id: &str) -> PathBuf {
    state.join(format!("registry-{token}-{id}.reg"))
}

fn reg_exe() -> GuiResult<PathBuf> {
    let root = std::env::var_os("SystemRoot")
        .ok_or_else(|| error("Windows system directory is unavailable"))?;
    let executable = PathBuf::from(root).join("System32").join("reg.exe");
    if !executable.is_file() {
        return Err(error("Windows registry tool is unavailable"));
    }
    Ok(executable)
}

fn run_reg(args: &[&str]) -> GuiResult<Output> {
    Command::new(reg_exe()?).args(args).output().map_err(io)
}

fn key_exists(key: &str) -> GuiResult<bool> {
    let relative = key
        .strip_prefix("HKCU\\")
        .ok_or_else(|| error("Unsupported registry root"))?;
    let wide: Vec<u16> = std::ffi::OsStr::new(relative)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut handle = std::ptr::null_mut();
    // The native API distinguishes a genuinely missing key from access or
    // registry errors; reg.exe's localized text does not.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut handle,
        )
    };
    match status {
        ERROR_SUCCESS => {
            unsafe { RegCloseKey(handle) };
            Ok(true)
        }
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(false),
        _ => Err(error("Could not query current-user update registry key")),
    }
}

type Hkey = *mut std::ffi::c_void;
const HKEY_CURRENT_USER: Hkey = (-2_147_483_647isize) as Hkey;
const KEY_QUERY_VALUE: u32 = 0x0001;
const ERROR_SUCCESS: i32 = 0;
const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PATH_NOT_FOUND: i32 = 3;

#[link(name = "Advapi32")]
#[allow(non_snake_case)]
unsafe extern "system" {
    fn RegOpenKeyExW(
        hkey: Hkey,
        sub_key: *const u16,
        options: u32,
        desired_access: u32,
        result: *mut Hkey,
    ) -> i32;
    fn RegCloseKey(hkey: Hkey) -> i32;
}

/// Export the two known product keys and return their checksum bindings.
/// `None` records a key that did not exist before the installer ran.
pub fn capture(state: &Path, token: &str) -> GuiResult<RegistrySnapshot> {
    if !is_hex(token, 32) {
        return Err(error("Invalid registry snapshot token"));
    }
    let result = (|| {
        let mut result = RegistrySnapshot::new();
        for (id, key) in KEYS {
            let path = export_path(state, token, id);
            if !key_exists(key)? {
                let _ = fs::remove_file(&path);
                result.insert(key.to_string(), None);
                continue;
            }
            let output = Command::new(reg_exe()?)
                .arg("export")
                .arg(key)
                .arg(&path)
                .arg("/y")
                .output()
                .map_err(io)?;
            if !output.status.success() {
                let _ = fs::remove_file(&path);
                return Err(error("Could not capture current-user update registry key"));
            }
            let metadata = fs::symlink_metadata(&path).map_err(io)?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > MAX_EXPORT
            {
                let _ = fs::remove_file(&path);
                return Err(error("Registry export exceeds the updater size limit"));
            }
            let bytes = fs::read(&path).map_err(io)?;
            validate_export(&bytes, key)?;
            result.insert(key.to_string(), Some(digest(&bytes)));
        }
        Ok(result)
    })();
    if result.is_err() {
        let _ = cleanup(state, token);
    }
    result
}

/// Restore the captured product keys. All backup files and key bindings are
/// validated before deleting either key, so corrupt state cannot partially
/// erase the current installer metadata.
pub fn restore(state: &Path, token: &str, snapshot: &RegistrySnapshot) -> GuiResult<()> {
    if !is_hex(token, 32) {
        return Err(error("Invalid registry snapshot binding"));
    }
    // Old journals and unit fixtures do not have registry snapshots. Keep this
    // a no-op so they can never cause edits to the host's HKCU registry.
    if snapshot.is_empty() {
        return Ok(());
    }
    if snapshot.len() != KEYS.len() {
        return Err(error("Invalid registry snapshot binding"));
    }
    let mut imports = Vec::new();
    for (id, key) in KEYS {
        let value = snapshot
            .get(key)
            .ok_or_else(|| error("Registry snapshot contains an unknown key set"))?;
        if let Some(expected_hash) = value {
            if !is_hex(expected_hash, 64) {
                return Err(error("Invalid registry export checksum"));
            }
            let path = export_path(state, token, id);
            let metadata = fs::symlink_metadata(&path).map_err(io)?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > MAX_EXPORT
            {
                return Err(error("Invalid bounded registry export"));
            }
            let bytes = fs::read(&path).map_err(io)?;
            if digest(&bytes) != expected_hash.as_str() {
                return Err(error("Registry export checksum mismatch"));
            }
            validate_export(&bytes, key)?;
            imports.push((key, path));
        }
    }
    if snapshot
        .keys()
        .any(|key| !KEYS.iter().any(|(_, allowed)| key.as_str() == *allowed))
    {
        return Err(error("Registry snapshot contains an unsupported key"));
    }

    for (_, key) in KEYS {
        let output = run_reg(&["delete", key, "/f"])?;
        if !output.status.success() && key_exists(key)? {
            return Err(error("Could not remove candidate update registry key"));
        }
    }
    for (key, path) in imports {
        if !Command::new(reg_exe()?)
            .arg("import")
            .arg(&path)
            .output()
            .map_err(io)?
            .status
            .success()
        {
            return Err(error("Could not restore previous update registry key"));
        }
        if !key_exists(key)? {
            return Err(error("Restored update registry key is missing"));
        }
    }
    Ok(())
}

/// Remove only the two token-scoped export files after their journal is retired.
pub fn cleanup(state: &Path, token: &str) -> GuiResult<()> {
    if !is_hex(token, 32) {
        return Err(error("Invalid registry snapshot token"));
    }
    for (id, _) in KEYS {
        let path = export_path(state, token, id);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                fs::remove_file(path).map_err(io)?;
            }
            Ok(_) => return Err(error("Invalid registry export cleanup path")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(e)),
        }
    }
    Ok(())
}

fn validate_export(bytes: &[u8], key: &str) -> GuiResult<()> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_EXPORT {
        return Err(error("Invalid bounded registry export"));
    }
    let encoded = bytes
        .strip_prefix(&[0xff, 0xfe])
        .ok_or_else(|| error("Registry export has an invalid encoding"))?;
    if encoded.len() % 2 != 0 {
        return Err(error("Registry export has an invalid encoding"));
    }
    let text = String::from_utf16(
        &encoded
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .map_err(|_| error("Registry export has an invalid encoding"))?;
    let allowed = key
        .to_ascii_lowercase()
        .replace("hkcu", "hkey_current_user");
    let mut found_key = false;
    for line in text.lines().map(str::trim) {
        if !line.starts_with('[') || !line.ends_with(']') {
            continue;
        }
        let section = &line[1..line.len() - 1];
        let section = section.to_ascii_lowercase();
        if section != allowed && !section.starts_with(&format!("{allowed}\\")) {
            return Err(error("Registry export contains an unexpected key"));
        }
        found_key = true;
    }
    if !found_key {
        return Err(error("Registry export does not contain its bound key"));
    }
    Ok(())
}
