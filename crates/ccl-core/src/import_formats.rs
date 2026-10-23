//! Import converters for CurseForge export archives and MultiMC/Prism exports.
//!
//! Both formats are normalized to the launcher's transactional Modrinth pack installer.

use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use thiserror::Error;
use zip::ZipArchive;

use crate::{
    TaskContext,
    config::DownloadConfig,
    download::{DownloadManager, DownloadRequest},
    instances::InstanceMetadata,
    modpack_format::{self, Manifest},
    modpacks::{ImportOptions, ModpackError, ModpackService},
};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum ImportFormatError {
    #[error("unsupported or invalid import archive: {0}")]
    Invalid(String),
    #[error("archive I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("archive ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("archive JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("CurseForge API returned HTTP {0}")]
    Http(u16),
    #[error("operation cancelled")]
    Cancelled,
    #[error(transparent)]
    Pack(#[from] ModpackError),
    #[error(transparent)]
    Download(#[from] crate::download::DownloadError),
}
impl ImportFormatError {
    /// Whether the importer stopped because its caller cancelled the operation.
    pub fn is_cancelled(&self) -> bool {
        match self {
            Self::Cancelled | Self::Download(crate::download::DownloadError::Cancelled) => true,
            Self::Pack(error) => error.is_cancelled(),
            _ => false,
        }
    }
}

#[derive(Clone, Default)]
pub struct ImportOptionsExt {
    pub include_optional: bool,
    pub replace_modified: bool,
    /// CurseForge API key provided by user configuration. Never persisted by this service.
    pub curseforge_api_key: Option<String>,
    /// Exact extra hosts trusted for file downloads.
    pub allowed_hosts: Vec<String>,
}
impl std::fmt::Debug for ImportOptionsExt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportOptionsExt")
            .field("include_optional", &self.include_optional)
            .field("replace_modified", &self.replace_modified)
            .field(
                "curseforge_api_key",
                &self.curseforge_api_key.as_ref().map(|_| "[redacted]"),
            )
            .field("allowed_hosts", &self.allowed_hosts)
            .finish()
    }
}

#[derive(Clone)]
pub struct ImportFormatService {
    root: PathBuf,
    config: DownloadConfig,
    curseforge_api: String,
    #[cfg(test)]
    prepared: bool,
}

impl ImportFormatService {
    pub fn new(root: PathBuf, config: DownloadConfig) -> Self {
        Self {
            root,
            config,
            curseforge_api: "https://api.curseforge.com/v1".into(),
            #[cfg(test)]
            prepared: false,
        }
    }

    /// Use the transaction fixture mode without downloading a Minecraft runtime.
    #[cfg(test)]
    pub(crate) fn new_prepared(root: PathBuf, config: DownloadConfig) -> Self {
        Self {
            root,
            config,
            curseforge_api: "https://api.curseforge.com/v1".into(),
            prepared: true,
        }
    }

    /// Override the file metadata service for a controlled mirror or service fixture.
    #[cfg(test)]
    pub fn with_curseforge_api(mut self, url: impl Into<String>) -> Self {
        self.curseforge_api = url.into().trim_end_matches('/').into();
        self
    }
    /// Identify standard Modrinth archives independently of their filename extension.
    pub fn is_modrinth(path: &Path) -> Result<bool, ImportFormatError> {
        let mut zip = ZipArchive::new(fs::File::open(path)?)?;
        Ok(zip.by_name("modrinth.index.json").is_ok())
    }
    /// Inspect an archive without network access or installation.
    pub fn inspect(path: &Path) -> Result<serde_json::Value, ImportFormatError> {
        if Self::is_modrinth(path)? {
            return Ok(serde_json::to_value(ModpackService::inspect(path)?)?);
        }
        let kind = classify(path)?;
        let (mut zip, prefix) = checked_archive(path)?;
        let (format, file) = match kind {
            ArchiveKind::CurseForge => ("curseforge", "manifest.json"),
            ArchiveKind::MultiMc => ("multimc-prism", "mmc-pack.json"),
        };
        let raw: serde_json::Value =
            serde_json::from_slice(&read_json_entry(&mut zip, &format!("{prefix}{file}"))?)?;
        Ok(serde_json::json!({"format":format,"manifest":raw}))
    }
    /// Convert and install a CurseForge, MultiMC, or Prism export archive into an instance.
    pub async fn import(
        &self,
        id: &str,
        archive: &Path,
        options: &ImportOptionsExt,
        context: Option<TaskContext>,
    ) -> Result<InstanceMetadata, ImportFormatError> {
        check_cancelled(context.as_ref())?;
        if Self::is_modrinth(archive)? {
            return Ok(ModpackService::new(self.root.clone(), self.config.clone())?
                .import(
                    id,
                    archive,
                    &ImportOptions {
                        include_optional: options.include_optional,
                        replace_modified: options.replace_modified,
                        allowed_hosts: options.allowed_hosts.clone(),
                    },
                    context,
                )
                .await?);
        }
        let temp = std::env::temp_dir().join(format!(
            "ccl-convert-{}-{}.mrpack",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let result = async {
            let kind = classify(archive)?;
            match kind {
                ArchiveKind::CurseForge => {
                    convert_curseforge(
                        archive,
                        &temp,
                        &self.config,
                        options,
                        &self.curseforge_api,
                        context.clone(),
                    )
                    .await?
                }
                ArchiveKind::MultiMc => convert_multimc(archive, &temp)?,
            }
            #[cfg(test)]
            let service = if self.prepared {
                ModpackService::new_prepared(self.root.clone(), self.config.clone())?
            } else {
                ModpackService::new(self.root.clone(), self.config.clone())?
            };
            #[cfg(not(test))]
            let service = ModpackService::new(self.root.clone(), self.config.clone())?;
            service
                .import(
                    id,
                    &temp,
                    &ImportOptions {
                        include_optional: options.include_optional,
                        replace_modified: options.replace_modified,
                        allowed_hosts: options.allowed_hosts.clone(),
                    },
                    context,
                )
                .await
                .map_err(ImportFormatError::from)
        }
        .await;
        let _ = fs::remove_file(temp);
        result
    }
}

#[derive(Clone, Copy)]
enum ArchiveKind {
    CurseForge,
    MultiMc,
}
fn validate_archive_name(name: &str) -> Result<(), ImportFormatError> {
    let invalid = || ImportFormatError::Invalid(format!("unsafe archive path: {name}"));
    if name.is_empty()
        || name.starts_with('/')
        || name.contains(['\\', ':'])
        || name
            .chars()
            .any(|c| c.is_control() || "<>\"|?*".contains(c))
    {
        return Err(invalid());
    }
    for part in name.split('/') {
        if part.is_empty()
            || matches!(part, "." | "..")
            || part.ends_with('.')
            || part.ends_with(' ')
        {
            return Err(invalid());
        }
        let stem = part
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(invalid());
        }
    }
    Ok(())
}
fn checked_archive(path: &Path) -> Result<(ZipArchive<fs::File>, String), ImportFormatError> {
    let mut zip = ZipArchive::new(fs::File::open(path)?)?;
    validate_archive_entry_count(zip.len())?;
    let mut total = 0u64;
    let mut names = std::collections::BTreeMap::new();
    let mut roots = Vec::new();
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        let name = entry.name()?.into_owned();
        let clean = if entry.is_dir() {
            name.strip_suffix('/').ok_or_else(|| {
                ImportFormatError::Invalid(format!(
                    "directory entry lacks a trailing slash: {name}"
                ))
            })?
        } else {
            name.as_str()
        };
        validate_archive_name(clean)?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(ImportFormatError::Invalid(format!(
                "symlink entry {name} is forbidden"
            )));
        }
        add_archive_size(&mut total, entry.size(), &name)?;
        if names
            .insert(clean.to_ascii_lowercase(), entry.is_dir())
            .is_some()
        {
            return Err(ImportFormatError::Invalid(format!(
                "duplicate or case-colliding entry {name}"
            )));
        }
        if clean == "manifest.json" || clean == "mmc-pack.json" {
            roots.push(String::new());
        } else if clean.ends_with("/mmc-pack.json") || clean.ends_with("/manifest.json") {
            let prefix = clean.rsplit_once('/').unwrap().0;
            if !prefix.contains('/') {
                roots.push(format!("{prefix}/"));
            }
        }
    }
    for name in names.keys() {
        let mut parent = name.as_str();
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            if names.get(prefix) == Some(&false) {
                return Err(ImportFormatError::Invalid(format!(
                    "file/directory collision at {prefix}"
                )));
            }
            parent = prefix;
        }
    }
    roots.sort();
    roots.dedup();
    if roots.iter().any(String::is_empty) {
        roots = vec![String::new()];
    }
    if roots.len() != 1 {
        return Err(ImportFormatError::Invalid(
            "expected exactly one manifest.json or mmc-pack.json export root".into(),
        ));
    }
    Ok((zip, roots.remove(0)))
}
fn validate_archive_entry_count(count: usize) -> Result<(), ImportFormatError> {
    if count > 100_000 {
        Err(ImportFormatError::Invalid(
            "archive exceeds 100000 entries".into(),
        ))
    } else {
        Ok(())
    }
}
fn add_archive_size(total: &mut u64, size: u64, name: &str) -> Result<(), ImportFormatError> {
    if size > 128 * 1024 * 1024 {
        return Err(ImportFormatError::Invalid(format!(
            "entry {name} exceeds 128 MiB"
        )));
    }
    *total = total
        .checked_add(size)
        .ok_or_else(|| ImportFormatError::Invalid("archive size overflow".into()))?;
    if *total > 512 * 1024 * 1024 {
        return Err(ImportFormatError::Invalid("archive exceeds 512 MiB".into()));
    }
    Ok(())
}
fn read_json_entry(
    zip: &mut ZipArchive<fs::File>,
    name: &str,
) -> Result<Vec<u8>, ImportFormatError> {
    let entry = zip.by_name(name)?;
    if entry.size() > 2 * 1024 * 1024 {
        return Err(ImportFormatError::Invalid("manifest exceeds 2 MiB".into()));
    }
    let mut bytes = Vec::new();
    entry.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(ImportFormatError::Invalid("manifest exceeds 2 MiB".into()));
    }
    Ok(bytes)
}
fn check_cancelled(context: Option<&TaskContext>) -> Result<(), ImportFormatError> {
    if context.is_some_and(|ctx| ctx.cancellation.is_cancelled()) {
        Err(ImportFormatError::Cancelled)
    } else {
        Ok(())
    }
}
async fn wait_cancelled(context: &TaskContext) {
    loop {
        if context.cancellation.is_cancelled() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
async fn cancellable_send(
    request: reqwest::RequestBuilder,
    context: Option<&TaskContext>,
) -> Result<reqwest::Response, ImportFormatError> {
    let send = request.send();
    let result = if let Some(context) = context {
        tokio::select! {
            result = send => result,
            _ = wait_cancelled(context) => return Err(ImportFormatError::Cancelled),
        }
    } else {
        send.await
    };
    result.map_err(|e| ImportFormatError::Invalid(e.to_string()))
}
async fn read_response_limited(
    mut response: reqwest::Response,
    limit: usize,
    context: Option<&TaskContext>,
) -> Result<Vec<u8>, ImportFormatError> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(ImportFormatError::Invalid(format!(
            "CurseForge response exceeds {limit} bytes"
        )));
    }
    let mut bytes = Vec::new();
    loop {
        check_cancelled(context)?;
        let chunk = if let Some(context) = context {
            tokio::select! {
                chunk = response.chunk() => chunk,
                _ = wait_cancelled(context) => return Err(ImportFormatError::Cancelled),
            }
        } else {
            response.chunk().await
        }
        .map_err(|e| ImportFormatError::Invalid(e.to_string()))?;
        let Some(chunk) = chunk else { break };
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(ImportFormatError::Invalid(format!(
                "CurseForge response exceeds {limit} bytes"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn classify(path: &Path) -> Result<ArchiveKind, ImportFormatError> {
    let (mut zip, prefix) = checked_archive(path)?;
    let cf = zip.by_name(&format!("{prefix}manifest.json")).is_ok();
    let mmc = zip.by_name(&format!("{prefix}mmc-pack.json")).is_ok();
    match (cf, mmc) {
        (true, false) => Ok(ArchiveKind::CurseForge),
        (false, true) => Ok(ArchiveKind::MultiMc),
        _ => Err(ImportFormatError::Invalid(
            "ambiguous or unsupported archive manifests".into(),
        )),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfManifest {
    #[serde(default)]
    minecraft: CfMinecraft,
    name: Option<String>,
    version: Option<String>,
    #[serde(default)]
    files: Vec<CfFile>,
    #[serde(default)]
    overrides: String,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfMinecraft {
    version: Option<String>,
    mod_loaders: Vec<CfLoader>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfLoader {
    id: String,
    #[serde(default)]
    primary: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfFile {
    #[serde(default = "required_default")]
    required: bool,
    #[serde(rename = "projectID", alias = "projectId")]
    project_id: u64,
    #[serde(rename = "fileID", alias = "fileId")]
    file_id: u64,
}
fn required_default() -> bool {
    true
}
#[derive(Deserialize)]
struct CfFileResponse {
    data: CfFileInfo,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfFileInfo {
    download_url: Option<String>,
    file_name: String,
    file_length: u64,
    #[serde(default)]
    hashes: Vec<CfHash>,
}
#[derive(Deserialize)]
struct CfHash {
    value: String,
    algo: u32,
}

async fn convert_curseforge(
    input: &Path,
    output: &Path,
    config: &DownloadConfig,
    options: &ImportOptionsExt,
    api: &str,
    context: Option<TaskContext>,
) -> Result<(), ImportFormatError> {
    check_cancelled(context.as_ref())?;
    let (mut zip, prefix) = checked_archive(input)?;
    let raw = read_json_entry(&mut zip, &format!("{prefix}manifest.json"))?;
    let source: CfManifest = serde_json::from_slice(&raw)?;
    let has_files = source
        .files
        .iter()
        .any(|file| file.required || options.include_optional);
    if has_files && config.offline {
        return Err(ImportFormatError::Invalid(
            "CurseForge file resolution requires network access; disable network offline mode"
                .into(),
        ));
    }
    let key = if has_files {
        options
            .curseforge_api_key
            .as_deref()
            .filter(|x| !x.trim().is_empty())
            .ok_or_else(|| {
                ImportFormatError::Invalid(
                    "CurseForge file resolution requires a user-provided API key (CCL_CURSEFORGE_API_KEY)".into(),
                )
            })?
    } else {
        ""
    };
    let minecraft = source.minecraft.version.ok_or_else(|| {
        ImportFormatError::Invalid("CurseForge manifest has no Minecraft version".into())
    })?;
    let loader = source
        .minecraft
        .mod_loaders
        .iter()
        .find(|x| x.primary)
        .or_else(|| source.minecraft.mod_loaders.first())
        .map(|x| x.id.clone());
    let mut dependencies = BTreeMap::from([("minecraft".into(), minecraft)]);
    if let Some(id) = loader {
        let (key, value) = parse_cf_loader(&id)?;
        dependencies.insert(key.into(), value.into());
    }
    let mut builder = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(60))
        // A provider key must never follow a metadata redirect to a mirror or other host.
        .redirect(reqwest::redirect::Policy::none());
    if let Some(proxy) = config.http_proxy.as_deref() {
        builder = builder.proxy(
            reqwest::Proxy::all(proxy).map_err(|e| ImportFormatError::Invalid(e.to_string()))?,
        );
    }
    let client = builder
        .build()
        .map_err(|e| ImportFormatError::Invalid(e.to_string()))?;
    let downloader =
        DownloadManager::new(config).map_err(|e| ImportFormatError::Invalid(e.to_string()))?;
    let mut download_hosts = options.allowed_hosts.clone();
    download_hosts.extend(
        [
            "edge.forgecdn.net",
            "mediafilez.forgecdn.net",
            "mediafiles.forgecdn.net",
            "media.forgecdn.net",
            "curseforge.com",
            "www.curseforge.com",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    let overrides_dir = if source.overrides.is_empty() {
        "overrides"
    } else {
        source.overrides.as_str()
    };
    modpack_format::validate_content_path(&format!("config/{overrides_dir}"))
        .map_err(|e| ImportFormatError::Invalid(e.to_string()))?;
    let mut overrides = read_overrides(&mut zip, &format!("{prefix}{overrides_dir}"))?;
    let mut total = overrides.values().map(|v| v.len() as u64).sum::<u64>();
    for f in source.files {
        if !f.required && !options.include_optional {
            continue;
        }
        check_cancelled(context.as_ref())?;
        let mut api_key = reqwest::header::HeaderValue::from_str(key).map_err(|_| {
            ImportFormatError::Invalid("CurseForge API key is not a valid HTTP header".into())
        })?;
        api_key.set_sensitive(true);
        let response = cancellable_send(
            client
                .get(format!("{api}/mods/{}/files/{}", f.project_id, f.file_id))
                .header("x-api-key", api_key),
            context.as_ref(),
        )
        .await?;
        if !response.status().is_success() {
            return Err(ImportFormatError::Http(response.status().as_u16()));
        }
        let detail: CfFileResponse = serde_json::from_slice(
            &read_response_limited(response, 2 * 1024 * 1024, context.as_ref()).await?,
        )?;
        let url=detail.data.download_url.ok_or_else(||ImportFormatError::Invalid(format!("CurseForge file {} is not directly downloadable; download it manually and add it to the instance",detail.data.file_name)))?;
        modpack_format::validate_download_url(&url, &download_hosts)
            .map_err(|e| ImportFormatError::Invalid(e.to_string()))?;
        if detail.data.file_length > 128 * 1024 * 1024 {
            return Err(ImportFormatError::Invalid(format!(
                "CurseForge file {} exceeds the 128 MiB import limit",
                detail.data.file_name
            )));
        }
        total = total
            .checked_add(detail.data.file_length)
            .ok_or_else(|| ImportFormatError::Invalid("import size overflow".into()))?;
        if total > 512 * 1024 * 1024 {
            return Err(ImportFormatError::Invalid(
                "import exceeds 512 MiB total".into(),
            ));
        }
        let sha1 = detail
            .data
            .hashes
            .iter()
            .find(|h| h.algo == 1)
            .map(|h| h.value.clone())
            .ok_or_else(|| {
                ImportFormatError::Invalid(format!(
                    "CurseForge file {} has no SHA-1 metadata",
                    detail.data.file_name
                ))
            })?;
        let download_path = std::env::temp_dir().join(format!(
            "ccl-cf-{}-{}.part",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let mut request = DownloadRequest::new(vec![url.clone()], &download_path)
            .with_sha1(sha1)
            .with_size(detail.data.file_length)
            .with_allowed_hosts(download_hosts.clone())
            .with_api_key(key, crate::online_content::curseforge_download_hosts())
            .map_err(|e| ImportFormatError::Invalid(e.to_string()))?;
        if let Some(ctx) = &context {
            request = request.with_task_context(ctx.clone());
        }
        check_cancelled(context.as_ref())?;
        let download_result = downloader.download(request).await;
        if let Err(error) = download_result {
            let _ = fs::remove_file(&download_path);
            return Err(error.into());
        }
        let bytes = fs::read(&download_path)?;
        let _ = fs::remove_file(&download_path);
        let rel = format!("mods/{}", detail.data.file_name);
        modpack_format::validate_content_path(&rel)
            .map_err(|e| ImportFormatError::Invalid(e.to_string()))?;
        if overrides.insert(rel.clone(), bytes).is_some() {
            return Err(ImportFormatError::Invalid(format!(
                "duplicate CurseForge override path `{rel}`"
            )));
        }
    }
    let manifest = Manifest {
        format_version: 1,
        game: "minecraft".into(),
        version_id: source.version.unwrap_or_else(|| "imported".into()),
        name: source
            .name
            .unwrap_or_else(|| "Imported CurseForge pack".into()),
        summary: None,
        files: Vec::new(),
        dependencies,
    };
    modpack_format::write_pack(output, &manifest, &overrides)
        .map_err(|e| ImportFormatError::Invalid(e.to_string()))?;
    Ok(())
}

fn parse_cf_loader(id: &str) -> Result<(&'static str, &str), ImportFormatError> {
    let prefixes = [
        ("fabric-loader-", "fabric-loader"),
        ("fabric-", "fabric-loader"),
        ("quilt-loader-", "quilt-loader"),
        ("quilt-", "quilt-loader"),
        ("neoforge-", "neoforge"),
        ("forge-", "forge"),
    ];
    let (key, version) = prefixes
        .iter()
        .find_map(|(prefix, key)| id.strip_prefix(prefix).map(|version| (*key, version)))
        .ok_or_else(|| ImportFormatError::Invalid(format!("unsupported CurseForge loader {id}")))?;
    if version.is_empty() {
        return Err(ImportFormatError::Invalid("empty loader version".into()));
    }
    Ok((key, version))
}

fn read_overrides(
    zip: &mut ZipArchive<fs::File>,
    prefix: &str,
) -> Result<BTreeMap<String, Vec<u8>>, ImportFormatError> {
    let prefix = if prefix.is_empty() {
        "overrides".to_string()
    } else {
        prefix.trim_matches('/').to_string()
    };
    let mut result = BTreeMap::new();
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        let entry_name = entry.name()?.into_owned();
        let Some(path) = entry_name.strip_prefix(&(prefix.clone() + "/")) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        modpack_format::validate_content_path(path)
            .map_err(|e| ImportFormatError::Invalid(e.to_string()))?;
        if entry.size() > 128 * 1024 * 1024 {
            return Err(ImportFormatError::Invalid(format!(
                "override `{path}` exceeds size limit"
            )));
        }
        let mut bytes = Vec::new();
        entry.take(128 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 128 * 1024 * 1024 {
            return Err(ImportFormatError::Invalid(
                "entry exceeds actual read limit".into(),
            ));
        }
        result.insert(path.into(), bytes);
    }
    Ok(result)
}

#[derive(Deserialize)]
struct MmcPack {
    components: Vec<MmcComponent>,
}
#[derive(Deserialize)]
struct MmcComponent {
    uid: String,
    version: String,
}
fn convert_multimc(input: &Path, output: &Path) -> Result<(), ImportFormatError> {
    let (mut zip, prefix) = checked_archive(input)?;
    let raw = read_json_entry(&mut zip, &format!("{prefix}mmc-pack.json"))?;
    let pack: MmcPack = serde_json::from_slice(&raw)?;
    let mut mc = None;
    let mut deps: BTreeMap<String, String> = BTreeMap::new();
    for c in pack.components {
        match c.uid.as_str() {
            "net.minecraft" => mc = Some(c.version),
            "net.fabricmc.fabric-loader" => {
                deps.insert("fabric-loader".into(), c.version);
            }
            "org.quiltmc.quilt-loader" => {
                deps.insert("quilt-loader".into(), c.version);
            }
            "net.minecraftforge" => {
                deps.insert("forge".into(), c.version);
            }
            "net.neoforged" => {
                deps.insert("neoforge".into(), c.version);
            }
            "net.fabricmc.intermediary" | "org.lwjgl" | "org.lwjgl3" => {}
            other => {
                return Err(ImportFormatError::Invalid(format!(
                    "unsupported MultiMC/Prism component {other}; custom patches cannot be ignored"
                )));
            }
        }
    }
    deps.insert(
        "minecraft".into(),
        mc.ok_or_else(|| {
            ImportFormatError::Invalid("mmc-pack.json does not specify net.minecraft".into())
        })?,
    );
    if deps.keys().filter(|x| x.as_str() != "minecraft").count() > 1 {
        return Err(ImportFormatError::Invalid(
            "multiple loader components are unsupported".into(),
        ));
    }
    let mut overrides = BTreeMap::new();
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        let name = entry.name()?.into_owned();
        let Some(name) = name.strip_prefix(&prefix) else {
            continue;
        };
        if name.starts_with("patches/") {
            return Err(ImportFormatError::Invalid(
                "custom MultiMC/Prism patches require explicit conversion".into(),
            ));
        }
        let rel = name
            .strip_prefix("minecraft/")
            .or_else(|| name.strip_prefix(".minecraft/"));
        let Some(rel) = rel else { continue };
        if entry.is_dir() {
            continue;
        }
        if !matches!(
            rel.split('/').next().unwrap_or(""),
            "mods"
                | "config"
                | "defaultconfigs"
                | "resourcepacks"
                | "shaderpacks"
                | "datapacks"
                | "saves"
                | "options.txt"
                | "servers.dat"
        ) {
            continue;
        }
        modpack_format::validate_content_path(rel)
            .map_err(|e| ImportFormatError::Invalid(e.to_string()))?;
        if entry.size() > 128 * 1024 * 1024 {
            return Err(ImportFormatError::Invalid(format!(
                "file `{rel}` exceeds size limit"
            )));
        }
        let mut bytes = Vec::new();
        entry.take(128 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 128 * 1024 * 1024 {
            return Err(ImportFormatError::Invalid(
                "entry exceeds actual read limit".into(),
            ));
        }
        overrides.insert(rel.into(), bytes);
    }
    let manifest = Manifest {
        format_version: 1,
        game: "minecraft".into(),
        version_id: "imported".into(),
        name: "Imported MultiMC/Prism pack".into(),
        summary: None,
        files: Vec::new(),
        dependencies: deps,
    };
    modpack_format::write_pack(output, &manifest, &overrides)
        .map_err(|e| ImportFormatError::Invalid(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, sync::atomic::AtomicU64};
    use zip::{ZipWriter, write::SimpleFileOptions};
    static ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn converts_prism_archive_and_keeps_supported_game_content() {
        let id = ID.fetch_add(1, Ordering::Relaxed);
        let input =
            std::env::temp_dir().join(format!("ccl-prism-test-{}-{id}.zip", std::process::id()));
        let output =
            std::env::temp_dir().join(format!("ccl-prism-test-{}-{id}.mrpack", std::process::id()));
        let file = fs::File::create(&input).unwrap();
        let mut zip = ZipWriter::new(file);
        let opts = SimpleFileOptions::default();
        zip.start_file("mmc-pack.json", opts).unwrap();
        zip.write_all(br#"{"components":[{"uid":"net.minecraft","version":"1.20.1"},{"uid":"net.fabricmc.fabric-loader","version":"0.15.11"}]}"#).unwrap();
        zip.start_file("minecraft/mods/example.jar", opts).unwrap();
        zip.write_all(b"fixture mod").unwrap();
        zip.start_file("minecraft/config/example.toml", opts)
            .unwrap();
        zip.write_all(b"enabled=true").unwrap();
        zip.finish().unwrap();
        convert_multimc(&input, &output).unwrap();
        let result = modpack_format::read_pack(&output).unwrap();
        assert_eq!(result.manifest.dependencies["minecraft"], "1.20.1");
        assert_eq!(result.manifest.dependencies["fabric-loader"], "0.15.11");
        assert_eq!(result.overrides["mods/example.jar"], b"fixture mod");
        assert_eq!(result.overrides["config/example.toml"], b"enabled=true");
        let _ = fs::remove_file(input);
        let _ = fs::remove_file(output);
    }

    #[test]
    fn rejects_unsupported_curseforge_loaders_and_accepts_common_loader_ids() {
        assert_eq!(
            parse_cf_loader("fabric-0.15.11").unwrap(),
            ("fabric-loader", "0.15.11")
        );
        assert_eq!(
            parse_cf_loader("forge-47.2.0").unwrap(),
            ("forge", "47.2.0")
        );
        assert!(parse_cf_loader("unknown-1").is_err());
    }
}

#[cfg(test)]
#[path = "import_formats_tests.rs"]
mod regression_tests;
