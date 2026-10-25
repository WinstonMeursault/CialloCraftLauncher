use super::*;
use flate2::{Compression, write::GzEncoder};
use std::{
    collections::BTreeMap,
    io::Cursor,
    time::{SystemTime, UNIX_EPOCH},
};

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ccl-gui-transaction-test-{unique}"));
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn tar_gz(entries: &[(&str, tar::EntryType, &[u8], Option<&str>)]) -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut archive = tar::Builder::new(encoder);
    for (path, kind, contents, link) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(*kind);
        header.set_size(contents.len() as u64);
        let mode = if kind.is_dir() || path.ends_with("/ccl-gui") {
            0o755
        } else {
            0o644
        };
        header.set_mode(mode);
        if let Some(link) = link {
            header.set_link_name(link).unwrap();
        }
        // Encode deliberately unsafe paths directly; Builder::append_data would
        // reject them before the updater's archive validation is exercised.
        let raw = header.as_mut_bytes();
        assert!(path.len() < 100);
        raw[..100].fill(0);
        raw[..path.len()].copy_from_slice(path.as_bytes());
        header.set_cksum();
        archive.append(&header, Cursor::new(*contents)).unwrap();
    }
    archive.into_inner().unwrap().finish().unwrap()
}

fn regular() -> tar::EntryType {
    tar::EntryType::Regular
}

fn dir() -> tar::EntryType {
    tar::EntryType::Directory
}

fn extract_fails(entries: &[(&str, tar::EntryType, &[u8], Option<&str>)]) {
    let temp = Temp::new();
    let destination = temp.0.join("candidate.app");
    assert!(extract_app(&tar_gz(entries), &destination).is_err());
}

#[test]
fn extracts_a_complete_application_bundle() {
    let temp = Temp::new();
    let destination = temp.0.join("candidate.app");
    let bytes = tar_gz(&[
        ("CialloCraftLauncher.app/", dir(), b"", None),
        ("CialloCraftLauncher.app/Contents/", dir(), b"", None),
        ("CialloCraftLauncher.app/Contents/MacOS/", dir(), b"", None),
        (
            "CialloCraftLauncher.app/Contents/MacOS/ccl-gui",
            regular(),
            b"launcher binary",
            None,
        ),
        (
            "CialloCraftLauncher.app/Contents/Info.plist",
            regular(),
            b"plist",
            None,
        ),
    ]);

    extract_app(&bytes, &destination).unwrap();
    assert_eq!(
        fs::read(destination.join("Contents/MacOS/ccl-gui")).unwrap(),
        b"launcher binary"
    );
    assert_eq!(
        fs::read(destination.join("Contents/Info.plist")).unwrap(),
        b"plist"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(destination.join("Contents/MacOS/ccl-gui"))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);
    }
}

#[test]
fn rejects_unsafe_and_incomplete_application_archives() {
    extract_fails(&[(
        "CialloCraftLauncher.app/../outside",
        regular(),
        b"escape",
        None,
    )]);
    extract_fails(&[(
        "CialloCraftLauncher.app/Contents/link",
        tar::EntryType::Symlink,
        b"",
        Some("../../outside"),
    )]);
    extract_fails(&[
        (
            "CialloCraftLauncher.app/Contents/MacOS/ccl-gui",
            regular(),
            b"first",
            None,
        ),
        (
            "CialloCraftLauncher.app/Contents/MacOS/ccl-gui",
            regular(),
            b"second",
            None,
        ),
        (
            "CialloCraftLauncher.app/Contents/Info.plist",
            regular(),
            b"plist",
            None,
        ),
    ]);
    extract_fails(&[
        (
            "First.app/Contents/MacOS/ccl-gui",
            regular(),
            b"binary",
            None,
        ),
        ("Second.app/Contents/Info.plist", regular(), b"plist", None),
    ]);
    extract_fails(&[(
        "CialloCraftLauncher.app/Contents/Info.plist",
        regular(),
        b"plist only",
        None,
    )]);
}

#[cfg(unix)]
#[test]
fn snapshot_rejects_symbolic_links() {
    use std::os::unix::fs::symlink;

    let temp = Temp::new();
    let tree = temp.0.join("application");
    fs::create_dir(&tree).unwrap();
    fs::write(tree.join("real"), b"contents").unwrap();
    symlink(tree.join("real"), tree.join("link")).unwrap();
    assert!(snapshot(&tree).is_err());
}

#[test]
fn bounded_file_rejects_oversized_input() {
    let temp = Temp::new();
    let path = temp.0.join("oversized");
    fs::write(&path, b"12345").unwrap();
    assert!(bounded_file(&path, 4).is_err());
}

fn transaction_fixture() -> (Temp, Installation, Journal, BTreeMap<String, String>) {
    let temp = Temp::new();
    let target = temp.0.join("installed-app");
    let state = temp.0.join("update-state");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("app.bin"), b"previous application").unwrap();
    fs::create_dir(&state).unwrap();
    let install = Installation::at(target.clone(), state).unwrap();
    let previous = snapshot(&target).unwrap();
    let journal = Journal {
        schema: 1,
        target,
        token: "a".repeat(32),
        phase: "replacing".into(),
        manifest: Vec::new(),
        signature: String::new(),
        previous: previous.clone(),
        last_error: None,
        registry: BTreeMap::new(),
    };
    (temp, install, journal, previous)
}

#[test]
fn restore_recovers_missing_target_and_is_idempotent() {
    let (_temp, install, mut journal, previous) = transaction_fixture();
    let backup = install.backup(&journal);
    fs::rename(&journal.target, &backup).unwrap();

    install.restore(&mut journal).unwrap();
    assert_eq!(snapshot(&journal.target).unwrap(), previous);
    assert_eq!(journal.phase, "rolled_back");
    assert!(!backup.exists());

    install.restore(&mut journal).unwrap();
    assert_eq!(snapshot(&journal.target).unwrap(), previous);
    assert_eq!(journal.phase, "rolled_back");
}

#[test]
fn restore_replaces_a_partial_target_with_valid_backup() {
    let (_temp, install, mut journal, previous) = transaction_fixture();
    let backup = install.backup(&journal);
    fs::rename(&journal.target, &backup).unwrap();
    fs::create_dir(&journal.target).unwrap();
    fs::write(journal.target.join("partial.bin"), b"new application").unwrap();

    install.restore(&mut journal).unwrap();
    assert_eq!(snapshot(&journal.target).unwrap(), previous);
}

#[test]
fn restore_rejects_corrupt_backup_without_touching_target() {
    let (_temp, install, mut journal, _) = transaction_fixture();
    let backup = install.backup(&journal);
    fs::create_dir(&backup).unwrap();
    fs::write(backup.join("app.bin"), b"corrupt backup").unwrap();
    let target_before = snapshot(&journal.target).unwrap();

    assert!(install.restore(&mut journal).is_err());
    assert_eq!(snapshot(&journal.target).unwrap(), target_before);
    assert!(backup.exists());
}

#[test]
fn restore_leaves_user_data_outside_application_tree_untouched() {
    let (temp, install, mut journal, _) = transaction_fixture();
    let user_data = temp.0.join("game-data");
    fs::create_dir(&user_data).unwrap();
    fs::write(user_data.join("saves.dat"), b"user save").unwrap();
    let backup = install.backup(&journal);
    fs::rename(&journal.target, &backup).unwrap();

    install.restore(&mut journal).unwrap();
    assert_eq!(fs::read(user_data.join("saves.dat")).unwrap(), b"user save");
}

#[test]
fn snapshot_enforces_tree_size_bound() {
    let temp = Temp::new();
    let tree = temp.0.join("large-tree");
    fs::create_dir(&tree).unwrap();
    let file = tree.join("sparse.bin");
    File::create(&file).unwrap().set_len(MAX_TREE + 1).unwrap();
    assert!(snapshot(&tree).is_err());
}
