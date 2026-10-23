//! Account metadata and protected credential storage.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountMetadata {
    pub id: String,
    pub username: String,
    pub uuid: String,
    #[serde(default)]
    pub xuid: Option<String>,
    pub account_type: String,
    #[serde(default)]
    pub oauth_client_id: Option<String>,
    pub last_used_unix: Option<u64>,
}

/// Secrets are deliberately opaque and do not implement Debug or Serialize.
pub struct AccountSecrets {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) expires_at_unix: u64,
}

impl AccountSecrets {
    pub fn new(access_token: String, refresh_token: String, expires_at_unix: u64) -> Self {
        Self {
            access_token,
            refresh_token,
            expires_at_unix,
        }
    }
    pub fn access_token(&self) -> &str {
        &self.access_token
    }
    pub fn refresh_token(&self) -> &str {
        &self.refresh_token
    }
    pub fn expires_at_unix(&self) -> u64 {
        self.expires_at_unix
    }
}

/// Implementations must protect values using the operating system credential store.
pub trait SecretStore: Send + Sync {
    fn put(&self, account_id: &str, secrets: &AccountSecrets) -> Result<(), AccountError>;
    fn get(&self, account_id: &str) -> Result<Option<AccountSecrets>, AccountError>;
    fn remove(&self, account_id: &str) -> Result<(), AccountError>;
}

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("account storage I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("account metadata is invalid")]
    Invalid,
    #[error("secure credential storage is unavailable: {0}")]
    SecureStorage(String),
    #[error("account metadata is corrupt")]
    Corrupt,
    #[error("account operation was cancelled")]
    Cancelled,
    #[error("credentials were removed, but account metadata could not be updated: {source}")]
    CredentialRemovedMetadataWrite {
        #[source]
        source: Box<AccountError>,
    },
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountIndex {
    accounts: Vec<AccountMetadata>,
    selected: Option<String>,
}

/// File-backed nonsecret metadata paired with an injected OS secure store.
pub struct AccountStore<S: SecretStore> {
    path: PathBuf,
    secrets: Arc<S>,
    lock: Mutex<()>,
}

/// Per-account advisory lock held during credential refresh.
pub struct AccountRefreshLock {
    file: std::fs::File,
    account_index: PathBuf,
    account_id: String,
}
impl AccountRefreshLock {
    pub fn unlock(self) -> Result<(), AccountError> {
        fs2::FileExt::unlock(&self.file)?;
        Ok(())
    }
}

impl<S: SecretStore> AccountStore<S> {
    pub fn new(path: impl Into<PathBuf>, secrets: Arc<S>) -> Self {
        Self {
            path: path.into(),
            secrets,
            lock: Mutex::new(()),
        }
    }
    fn read(&self) -> Result<AccountIndex, AccountError> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| AccountError::Corrupt),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AccountIndex::default()),
            Err(e) => Err(e.into()),
        }
    }
    fn write(&self, index: &AccountIndex) -> Result<(), AccountError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let (tmp, mut f) = create_temp_near(&self.path)?;
        let _cleanup = TempCleanup(tmp.clone());
        let bytes = serde_json::to_vec_pretty(index).map_err(|_| AccountError::Corrupt)?;
        use std::io::Write;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(&bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
    pub fn list(&self) -> Result<Vec<AccountMetadata>, AccountError> {
        let _g = self.lock.lock().unwrap();
        Ok(self.read()?.accounts)
    }
    pub fn selected(&self) -> Result<Option<String>, AccountError> {
        let _g = self.lock.lock().unwrap();
        Ok(self.read()?.selected)
    }
    pub fn select(&self, id: &str) -> Result<(), AccountError> {
        let _g = self.lock.lock().unwrap();
        let _file_lock = self.file_lock()?;
        let mut i = self.read()?;
        if !i.accounts.iter().any(|a| a.id == id) {
            return Err(AccountError::Invalid);
        }
        i.selected = Some(id.into());
        self.write(&i)
    }
    pub fn upsert(
        &self,
        metadata: AccountMetadata,
        secrets: AccountSecrets,
    ) -> Result<(), AccountError> {
        let refresh_lock = self.lock_refresh(&metadata.id, None)?;
        self.upsert_with_refresh_lock(metadata, secrets, &refresh_lock)
    }
    pub fn upsert_cancellable(
        &self,
        metadata: AccountMetadata,
        secrets: AccountSecrets,
        cancel: &crate::task::CancellationToken,
    ) -> Result<(), AccountError> {
        let refresh_lock = self.lock_refresh(&metadata.id, Some(cancel))?;
        self.upsert_with_refresh_lock(metadata, secrets, &refresh_lock)
    }
    pub(crate) fn upsert_with_refresh_lock(
        &self,
        metadata: AccountMetadata,
        secrets: AccountSecrets,
        refresh_lock: &AccountRefreshLock,
    ) -> Result<(), AccountError> {
        validate(&metadata)?;
        if refresh_lock.account_index != self.path || refresh_lock.account_id != metadata.id {
            return Err(AccountError::Invalid);
        }
        let _g = self.lock.lock().unwrap();
        let _file_lock = self.file_lock()?;
        let mut i = self.read()?;
        if let Some(old) = i.accounts.iter().find(|a| a.id == metadata.id)
            && old.uuid != metadata.uuid
        {
            return Err(AccountError::Invalid);
        }
        self.secrets.put(&metadata.id, &secrets)?;
        if let Some(a) = i.accounts.iter_mut().find(|a| a.id == metadata.id) {
            *a = metadata;
        } else {
            i.accounts.push(metadata);
        }
        self.write(&i)
    }
    pub fn credentials(&self, id: &str) -> Result<Option<AccountSecrets>, AccountError> {
        let _g = self.lock.lock().unwrap();
        if !self.read()?.accounts.iter().any(|a| a.id == id) {
            return Ok(None);
        }
        self.secrets.get(id)
    }
    fn file_lock(&self) -> Result<std::fs::File, AccountError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock_path = self.path.with_extension("lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        fs2::FileExt::lock_exclusive(&file)?;
        Ok(file)
    }
    /// Attempts a cross-process exclusive lock for one account's refresh rotation.
    /// Returns None while another process owns the lock.
    pub fn try_lock_refresh(
        &self,
        account_id: &str,
    ) -> Result<Option<AccountRefreshLock>, AccountError> {
        if account_id.is_empty() {
            return Err(AccountError::Invalid);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        use sha1::{Digest, Sha1};
        let digest = Sha1::digest(account_id.as_bytes());
        let name = format!(
            "{}.refresh.{}.lock",
            self.path.file_name().unwrap_or_default().to_string_lossy(),
            hex_lower(&digest)
        );
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path.with_file_name(name))?;
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Some(AccountRefreshLock {
                file,
                account_index: self.path.clone(),
                account_id: account_id.to_owned(),
            })),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
            {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }
    fn lock_refresh(
        &self,
        account_id: &str,
        cancel: Option<&crate::task::CancellationToken>,
    ) -> Result<AccountRefreshLock, AccountError> {
        loop {
            if cancel.is_some_and(crate::task::CancellationToken::is_cancelled) {
                return Err(AccountError::Cancelled);
            }
            if let Some(lock) = self.try_lock_refresh(account_id)? {
                return Ok(lock);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    pub fn remove(&self, id: &str) -> Result<(), AccountError> {
        self.remove_inner(id, None)
    }
    pub fn remove_cancellable(
        &self,
        id: &str,
        cancel: &crate::task::CancellationToken,
    ) -> Result<(), AccountError> {
        self.remove_inner(id, Some(cancel))
    }
    fn remove_inner(
        &self,
        id: &str,
        cancel: Option<&crate::task::CancellationToken>,
    ) -> Result<(), AccountError> {
        let _refresh_lock = self.lock_refresh(id, cancel)?;
        let _g = self.lock.lock().unwrap();
        let _file_lock = self.file_lock()?;
        let mut i = self.read()?;
        self.secrets.remove(id)?;
        i.accounts.retain(|a| a.id != id);
        if i.selected.as_deref() == Some(id) {
            i.selected = None;
        }
        self.write(&i)
            .map_err(|source| AccountError::CredentialRemovedMetadataWrite {
                source: Box::new(source),
            })
    }
}
fn validate(a: &AccountMetadata) -> Result<(), AccountError> {
    if a.id.is_empty()
        || a.id.len() > 128
        || a.id.contains('/')
        || a.id.contains('\\')
        || a.username.is_empty()
        || a.uuid.is_empty()
    {
        Err(AccountError::Invalid)
    } else {
        Ok(())
    }
}

/// In-memory store for deterministic tests and embedding applications.
#[derive(Default)]
pub struct MemorySecretStore(Mutex<BTreeMap<String, (String, String, u64)>>);
impl SecretStore for MemorySecretStore {
    fn put(&self, id: &str, s: &AccountSecrets) -> Result<(), AccountError> {
        self.0.lock().unwrap().insert(
            id.into(),
            (
                s.access_token.clone(),
                s.refresh_token.clone(),
                s.expires_at_unix,
            ),
        );
        Ok(())
    }
    fn get(&self, id: &str) -> Result<Option<AccountSecrets>, AccountError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(id)
            .map(|(a, r, e)| AccountSecrets::new(a.clone(), r.clone(), *e)))
    }
    fn remove(&self, id: &str) -> Result<(), AccountError> {
        self.0.lock().unwrap().remove(id);
        Ok(())
    }
}

/// Native credential vault adapter. Requires the workspace `keyring` dependency.
pub struct OsSecretStore {
    service: String,
}

/// A provider API key scoped to a launcher configuration and kept only in the OS vault.
/// It is deliberately separate from account tokens and from the JSON configuration.
pub struct OsProviderKeyStore {
    service: String,
}

impl OsProviderKeyStore {
    pub fn for_config_file(path: &std::path::Path) -> Result<Self, AccountError> {
        let absolute = canonicalize_future_path(path)?;
        use sha1::{Digest, Sha1};
        let hash = Sha1::digest(absolute.to_string_lossy().as_bytes());
        Ok(Self {
            service: format!("CialloCraftLauncher/{}/provider-keys", hex_lower(&hash)),
        })
    }

    fn entry(&self) -> Result<keyring::Entry, AccountError> {
        keyring::Entry::new(&self.service, "curseforge")
            .map_err(|_| AccountError::SecureStorage("cannot access OS credential vault".into()))
    }

    pub fn set_curseforge(&self, key: &str) -> Result<(), AccountError> {
        if key.is_empty() || key.len() > 1024 || !key.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(AccountError::Invalid);
        }
        self.entry()?.set_password(key).map_err(|_| {
            AccountError::SecureStorage("could not save key in OS credential vault".into())
        })
    }

    pub fn get_curseforge(&self) -> Result<Option<String>, AccountError> {
        match self.entry()?.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(AccountError::SecureStorage(
                "could not read key from OS credential vault".into(),
            )),
        }
    }

    pub fn remove_curseforge(&self) -> Result<(), AccountError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(AccountError::SecureStorage(
                "could not remove key from OS credential vault".into(),
            )),
        }
    }
}

#[cfg(test)]
mod provider_key_tests {
    use super::*;

    #[test]
    fn provider_keys_are_scoped_to_config_and_reject_header_unsafe_input() {
        let base = std::env::temp_dir().join(format!("ccl-key-scope-{}", std::process::id()));
        let one = OsProviderKeyStore::for_config_file(&base.join("one/config.json")).unwrap();
        let same =
            OsProviderKeyStore::for_config_file(&base.join("one/../one/config.json")).unwrap();
        let other = OsProviderKeyStore::for_config_file(&base.join("two/config.json")).unwrap();
        assert_eq!(one.service, same.service);
        assert_ne!(one.service, other.service);
        for unsafe_key in ["", "key\nInjected: yes", "key with spaces", "é"] {
            assert!(matches!(
                one.set_curseforge(unsafe_key),
                Err(AccountError::Invalid)
            ));
        }
        assert!(matches!(
            one.set_curseforge(&"x".repeat(1025)),
            Err(AccountError::Invalid)
        ));
    }

    #[test]
    #[ignore = "requires an unlocked OS credential vault"]
    fn provider_key_vault_round_trip_never_uses_a_config_file() {
        let unique = std::env::temp_dir().join(format!(
            "ccl-provider-vault-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = unique.join("config.json");
        let store = OsProviderKeyStore::for_config_file(&config).unwrap();
        assert!(store.get_curseforge().unwrap().is_none());
        store.set_curseforge("temporary-test-key").unwrap();
        assert_eq!(
            store.get_curseforge().unwrap().as_deref(),
            Some("temporary-test-key")
        );
        assert!(!config.exists());
        store.remove_curseforge().unwrap();
        assert!(store.get_curseforge().unwrap().is_none());
    }
}
impl OsSecretStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
    /// Isolate credentials by account-index path to avoid collisions between launcher profiles.
    pub fn for_account_file(path: &std::path::Path) -> Result<Self, AccountError> {
        let absolute = canonicalize_future_path(path)?;
        use sha1::{Digest, Sha1};
        let hash = Sha1::digest(absolute.to_string_lossy().as_bytes());
        Ok(Self::new(format!(
            "CialloCraftLauncher/{}",
            hex_lower(&hash)
        )))
    }
    /// Verifies that the OS vault can write, read, and remove a temporary credential.
    /// This is safe to use as a startup diagnostic and never touches account credentials.
    pub fn probe(&self) -> Result<(), AccountError> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let account = format!("probe-{}-{nonce}", std::process::id());
        let expected = AccountSecrets::new(
            "probe-access-sentinel".into(),
            "probe-refresh-sentinel".into(),
            1,
        );
        if let Err(error) = self.put(&account, &expected) {
            let _ = self.remove(&account);
            return Err(error);
        }
        let result = (|| match self.get(&account)? {
            Some(actual)
                if actual.access_token() == expected.access_token()
                    && actual.refresh_token() == expected.refresh_token()
                    && actual.expires_at_unix() == expected.expires_at_unix() =>
            {
                Ok(())
            }
            _ => Err(AccountError::SecureStorage(
                "credential vault probe did not round-trip".into(),
            )),
        })();
        let cleanup = self.remove(&account);
        result.and(cleanup)
    }
}

fn canonicalize_future_path(path: &std::path::Path) -> Result<PathBuf, AccountError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut lexical = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                lexical.pop();
            }
            other => lexical.push(other.as_os_str()),
        }
    }
    if lexical.exists() {
        return fs::canonicalize(lexical).map_err(AccountError::from);
    }
    let mut ancestor = lexical.as_path();
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or_else(|| {
            AccountError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "account path has no existing ancestor",
            ))
        })?;
    }
    let canonical_ancestor = fs::canonicalize(ancestor)?;
    let suffix = lexical
        .strip_prefix(ancestor)
        .map_err(|_| AccountError::Invalid)?;
    Ok(canonical_ancestor.join(suffix))
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
struct TempCleanup(PathBuf);
impl Drop for TempCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn create_temp_near(path: &std::path::Path) -> Result<(PathBuf, fs::File), AccountError> {
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    for _ in 0..32 {
        let nonce = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let tmp = parent.join(format!(
            ".{name}.{}.{}.{}.tmp",
            std::process::id(),
            stamp,
            nonce
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(AccountError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate unique account metadata temp file",
    )))
}
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 15) as usize] as char);
    }
    s
}
impl SecretStore for OsSecretStore {
    fn put(&self, id: &str, s: &AccountSecrets) -> Result<(), AccountError> {
        let entry = keyring::Entry::new(&self.service, id)
            .map_err(|e| AccountError::SecureStorage(e.to_string()))?;
        let val = serde_json::json!({"access":s.access_token,"refresh":s.refresh_token,"expires":s.expires_at_unix});
        entry
            .set_password(&val.to_string())
            .map_err(|e| AccountError::SecureStorage(e.to_string()))
    }
    fn get(&self, id: &str) -> Result<Option<AccountSecrets>, AccountError> {
        let entry = keyring::Entry::new(&self.service, id)
            .map_err(|e| AccountError::SecureStorage(e.to_string()))?;
        match entry.get_password() {
            Ok(s) => {
                let v: serde_json::Value =
                    serde_json::from_str(&s).map_err(|_| AccountError::Corrupt)?;
                Ok(Some(AccountSecrets::new(
                    v["access"].as_str().ok_or(AccountError::Corrupt)?.into(),
                    v["refresh"].as_str().ok_or(AccountError::Corrupt)?.into(),
                    v["expires"].as_u64().ok_or(AccountError::Corrupt)?,
                )))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AccountError::SecureStorage(e.to_string())),
        }
    }
    fn remove(&self, id: &str) -> Result<(), AccountError> {
        let entry = keyring::Entry::new(&self.service, id)
            .map_err(|e| AccountError::SecureStorage(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(AccountError::SecureStorage(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct DeleteFailureStore(MemorySecretStore);
    impl SecretStore for DeleteFailureStore {
        fn put(&self, id: &str, secrets: &AccountSecrets) -> Result<(), AccountError> {
            self.0.put(id, secrets)
        }
        fn get(&self, id: &str) -> Result<Option<AccountSecrets>, AccountError> {
            self.0.get(id)
        }
        fn remove(&self, _id: &str) -> Result<(), AccountError> {
            Err(AccountError::SecureStorage(
                "simulated vault failure".into(),
            ))
        }
    }
    fn meta() -> AccountMetadata {
        AccountMetadata {
            id: "one".into(),
            username: "Player".into(),
            uuid: "uuid-1".into(),
            xuid: Some("xuid".into()),
            account_type: "microsoft".into(),
            oauth_client_id: Some("client".into()),
            last_used_unix: None,
        }
    }
    #[test]
    fn persists_metadata_and_secret_separately() {
        let d = std::env::temp_dir().join(format!("ccl-acct-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let s = Arc::new(MemorySecretStore::default());
        let st = AccountStore::new(d.join("accounts.json"), s.clone());
        st.upsert(
            meta(),
            AccountSecrets::new("a-token".into(), "r-token".into(), 33),
        )
        .unwrap();
        assert_eq!(st.list().unwrap(), vec![meta()]);
        assert!(
            fs::read_to_string(d.join("accounts.json"))
                .unwrap()
                .find("token")
                .is_none()
        );
        assert_eq!(
            st.credentials("one").unwrap().unwrap().refresh_token(),
            "r-token"
        );
        st.select("one").unwrap();
        st.remove("one").unwrap();
        assert_eq!(st.selected().unwrap(), None);
        let _ = fs::remove_dir_all(d);
    }
    #[test]
    fn rejects_identity_replacement() {
        let d = std::env::temp_dir().join(format!("ccl-acct-id-{}", std::process::id()));
        let s = Arc::new(MemorySecretStore::default());
        let st = AccountStore::new(&d, s);
        st.upsert(meta(), AccountSecrets::new("a".into(), "r".into(), 1))
            .unwrap();
        let mut m = meta();
        m.uuid = "other".into();
        assert!(
            st.upsert(m, AccountSecrets::new("b".into(), "r".into(), 2))
                .is_err()
        );
        let _ = fs::remove_file(d);
    }

    #[test]
    fn read_only_queries_do_not_create_the_account_directory() {
        let d = std::env::temp_dir().join(format!(
            "ccl-acct-read-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let s = Arc::new(MemorySecretStore::default());
        let st = AccountStore::new(d.join("nested/accounts.json"), s);
        assert!(st.list().unwrap().is_empty());
        assert_eq!(st.selected().unwrap(), None);
        assert!(st.credentials("missing").unwrap().is_none());
        assert!(!d.exists());
    }

    #[test]
    fn credential_namespace_normalizes_equivalent_future_paths() {
        let base = std::env::temp_dir().join(format!(
            "ccl-keyring-path-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let left = OsSecretStore::for_account_file(&base.join("config/accounts.json")).unwrap();
        let right =
            OsSecretStore::for_account_file(&base.join("config/../config/accounts.json")).unwrap();
        assert_eq!(left.service, right.service);
    }

    #[test]
    fn independent_store_instances_serialize_concurrent_updates() {
        let d = std::env::temp_dir().join(format!(
            "ccl-acct-concurrent-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let path = d.join("accounts.json");
        let secrets = Arc::new(MemorySecretStore::default());
        let barrier = Arc::new(std::sync::Barrier::new(12));
        let mut workers = Vec::new();
        for n in 0..12 {
            let path = path.clone();
            let secrets = secrets.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                let store = AccountStore::new(path, secrets);
                barrier.wait();
                let mut m = meta();
                m.id = format!("account-{n}");
                m.uuid = format!("uuid-{n}");
                store
                    .upsert(
                        m,
                        AccountSecrets::new(format!("access-{n}"), format!("refresh-{n}"), 1),
                    )
                    .unwrap();
            }));
        }
        for w in workers {
            w.join().unwrap();
        }
        let store = AccountStore::new(&path, secrets);
        let all = store.list().unwrap();
        assert_eq!(all.len(), 12);
        for n in 0..12 {
            assert!(
                store
                    .credentials(&format!("account-{n}"))
                    .unwrap()
                    .is_some()
            );
        }
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn failed_vault_delete_preserves_account_metadata() {
        let path = std::env::temp_dir().join(format!(
            "ccl-acct-delete-fail-{}",
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let secrets = Arc::new(DeleteFailureStore(MemorySecretStore::default()));
        let store = AccountStore::new(&path, secrets);
        store
            .upsert(
                meta(),
                AccountSecrets::new("access".into(), "refresh".into(), 1),
            )
            .unwrap();
        assert!(matches!(
            store.remove("one"),
            Err(AccountError::SecureStorage(_))
        ));
        assert_eq!(store.list().unwrap(), vec![meta()]);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn remove_waits_for_inflight_account_refresh_lock() {
        let path = std::env::temp_dir().join(format!(
            "ccl-acct-delete-lock-{}",
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let secrets = Arc::new(MemorySecretStore::default());
        let store = AccountStore::new(&path, secrets.clone());
        store
            .upsert(
                meta(),
                AccountSecrets::new("access".into(), "refresh".into(), 1),
            )
            .unwrap();
        let refresh_lock = store.try_lock_refresh("one").unwrap().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let remove_path = path.clone();
        let worker_secrets = secrets.clone();
        let worker = std::thread::spawn(move || {
            let worker_store = AccountStore::new(remove_path, worker_secrets);
            worker_store.remove("one").unwrap();
            tx.send(()).unwrap();
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
        drop(refresh_lock);
        rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        assert!(store.list().unwrap().is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn login_upsert_waits_for_refresh_and_then_replaces_credentials_without_deadlock() {
        let path = std::env::temp_dir().join(format!(
            "ccl-acct-login-refresh-{}",
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let secrets = Arc::new(MemorySecretStore::default());
        let store = AccountStore::new(&path, secrets.clone());
        store
            .upsert(
                meta(),
                AccountSecrets::new("old-access".into(), "old-refresh".into(), 1),
            )
            .unwrap();
        let held = store.try_lock_refresh("one").unwrap().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_path = path.clone();
        let worker_secrets = secrets.clone();
        let worker = std::thread::spawn(move || {
            let worker_store = AccountStore::new(worker_path, worker_secrets);
            let mut updated = meta();
            updated.username = "NewLogin".into();
            worker_store
                .upsert(
                    updated,
                    AccountSecrets::new("fresh-access".into(), "fresh-refresh".into(), 2),
                )
                .unwrap();
            tx.send(()).unwrap();
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
        drop(held);
        rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        assert_eq!(
            store.credentials("one").unwrap().unwrap().access_token(),
            "fresh-access"
        );
        assert_eq!(store.list().unwrap()[0].username, "NewLogin");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn cancellable_login_upsert_stops_while_waiting_for_refresh_lock() {
        let path = std::env::temp_dir().join(format!(
            "ccl-acct-login-cancel-{}",
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let secrets = Arc::new(MemorySecretStore::default());
        let store = AccountStore::new(&path, secrets.clone());
        store
            .upsert(
                meta(),
                AccountSecrets::new("old-access".into(), "old-refresh".into(), 1),
            )
            .unwrap();
        let held = store.try_lock_refresh("one").unwrap().unwrap();
        let cancellation = crate::task::CancellationToken::default();
        let worker_cancel = cancellation.clone();
        let worker_path = path.clone();
        let worker_secrets = secrets.clone();
        let worker = std::thread::spawn(move || {
            let worker_store = AccountStore::new(worker_path, worker_secrets);
            let mut updated = meta();
            updated.username = "CancelledLogin".into();
            worker_store.upsert_cancellable(
                updated,
                AccountSecrets::new("new-access".into(), "new-refresh".into(), 2),
                &worker_cancel,
            )
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancellation.cancel();
        assert!(matches!(
            worker.join().unwrap(),
            Err(AccountError::Cancelled)
        ));
        drop(held);
        assert_eq!(
            store.credentials("one").unwrap().unwrap().access_token(),
            "old-access"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    #[ignore = "requires an unlocked OS credential vault; run on each supported desktop platform"]
    fn os_secure_store_probe_round_trips_without_account_credentials() {
        OsSecretStore::new("CialloCraftLauncher-SecureStoreProbe")
            .probe()
            .unwrap();
    }
}
