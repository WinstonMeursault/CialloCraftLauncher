//! OptiFine version discovery and installer transformation.

use std::{
    fs,
    path::Path,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    TaskContext,
    config::DownloadConfig,
    download::{DownloadManager, DownloadRequest},
    install::{InstallOptions, InstallService},
    java,
    loaders::{LoaderError, LoaderKind, LoaderRequest, LoaderVersion},
    source::{self, DownloadKind},
};

static NEXT_INSTALLER: AtomicU64 = AtomicU64::new(0);

#[derive(serde::Deserialize)]
struct RemoteVersion {
    #[serde(default)]
    mcversion: String,
    #[serde(rename = "type", default)]
    edition: String,
    #[serde(default)]
    patch: Option<String>,
}

/// Lists OptiFine builds published by the configured BMCLAPI-compatible mirror.
pub async fn versions(
    c: &DownloadConfig,
    minecraft: &str,
) -> Result<Vec<LoaderVersion>, LoaderError> {
    if c.offline {
        return Err(LoaderError::Unsupported(
            "OptiFine version discovery is unavailable in offline mode".into(),
        ));
    }
    Ok(map_remote_versions(
        fetch_remote_versions(c, minecraft).await?,
        minecraft,
    ))
}

async fn fetch_remote_versions(
    c: &DownloadConfig,
    minecraft: &str,
) -> Result<Vec<RemoteVersion>, LoaderError> {
    let lookup = lookup_minecraft(minecraft);
    let url = format!("{}/optifine/{lookup}", c.mirror_root.trim_end_matches('/'));
    let client = client(c)?;
    Ok(client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<RemoteVersion>>()
        .await?)
}

fn map_remote_versions(response: Vec<RemoteVersion>, minecraft: &str) -> Vec<LoaderVersion> {
    response
        .into_iter()
        .filter(|entry| entry.mcversion == minecraft)
        .filter_map(|entry| {
            let patch = entry.patch?;
            safe_component(&entry.edition).ok()?;
            safe_component(&patch).ok()?;
            Some(LoaderVersion {
                stable: !patch.starts_with("pre") && !patch.starts_with("alpha"),
                version: format!("{}_{}", entry.edition, patch),
            })
        })
        .collect()
}

/// Installs an OptiFine build by running its embedded patcher against the vanilla client.
pub async fn install(
    c: &DownloadConfig,
    request: &LoaderRequest,
    options: &InstallOptions,
    context: Option<TaskContext>,
) -> Result<String, LoaderError> {
    if request.kind != LoaderKind::OptiFine {
        return Err(LoaderError::Unsupported(
            "OptiFine installer received another loader kind".into(),
        ));
    }
    if c.offline {
        return Err(LoaderError::Unsupported(
            "OptiFine installer metadata is unavailable in offline mode".into(),
        ));
    }
    safe_component(&request.minecraft)?;
    let published = fetch_remote_versions(c, &request.minecraft)
        .await?
        .into_iter()
        .find(|entry| {
            entry.mcversion == request.minecraft
                && entry
                    .patch
                    .as_deref()
                    .is_some_and(|patch| format!("{}_{}", entry.edition, patch) == request.version)
        });
    let Some(published) = published else {
        return Err(LoaderError::Unsupported(format!(
            "OptiFine {} is not published for Minecraft {}",
            request.version, request.minecraft
        )));
    };
    safe_component(&published.edition)?;
    let patch = published
        .patch
        .as_deref()
        .ok_or_else(|| LoaderError::Unsupported("OptiFine build has no patch version".into()))?;
    safe_component(patch)?;
    InstallService::new(c.clone())?
        .install(&request.minecraft, options, context.clone())
        .await?;
    check_cancelled(&context)?;
    let lookup = lookup_minecraft(&request.minecraft);
    let url = format!(
        "{}/optifine/{lookup}/{}/{patch}",
        c.mirror_root.trim_end_matches('/'),
        published.edition
    );
    let installer_dir = options.game_directory.join(".ccl/installers");
    fs::create_dir_all(&installer_dir).map_err(|e| io(&installer_dir, e))?;
    let destination = installer_dir.join(format!(
        "OptiFine_{}_{}_{}_{}.jar",
        request.minecraft,
        request.version,
        std::process::id(),
        NEXT_INSTALLER.fetch_add(1, Ordering::Relaxed)
    ));
    if destination.exists() || fs::symlink_metadata(&destination).is_ok() {
        return Err(LoaderError::Unsupported(
            "refusing to reuse an OptiFine installer cache without an upstream checksum".into(),
        ));
    }
    let candidates = https_candidates(source::url_candidates(&url, DownloadKind::File, c))?;
    let mut download = DownloadRequest::new(candidates, &destination).with_https();
    if let Some(ctx) = context.clone() {
        download = download.with_task_context(ctx);
    }
    DownloadManager::new(c)?.download(download).await?;
    verify_installer(&destination, &request.minecraft, &request.version)?;
    let client_jar = options
        .game_directory
        .join("versions")
        .join(&request.minecraft)
        .join(format!("{}.jar", request.minecraft));
    if !client_jar.is_file() {
        return Err(LoaderError::Unsupported(format!(
            "vanilla client JAR is missing: {}",
            client_jar.display()
        )));
    }
    let installer = destination;
    let version_id = format!("{}-OptiFine-{}", request.minecraft, request.version);
    safe_component(&version_id)?;
    let output = options
        .game_directory
        .join("libraries/optifine/OptiFine")
        .join(format!("{}_{}", request.minecraft, request.version))
        .join(format!(
            "OptiFine-{}_{}.jar",
            request.minecraft, request.version
        ));
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    if fs::symlink_metadata(&output).is_ok() {
        fs::remove_file(&output).map_err(|e| io(&output, e))?;
    }
    let has_patcher = zip_has_entry(&installer, "optifine/Patcher.class")?;
    if has_patcher {
        let vanilla_path = options
            .game_directory
            .join("versions")
            .join(&request.minecraft)
            .join(format!("{}.json", request.minecraft));
        let vanilla: crate::metadata::VersionMetadata =
            serde_json::from_slice(&fs::read(&vanilla_path).map_err(|e| io(&vanilla_path, e))?)
                .map_err(crate::metadata::MetadataError::from)?;
        let java_version = vanilla.java_version.as_ref();
        let major = java_version.and_then(|j| j.major_version).unwrap_or(8);
        let runtime = if let Some(component) = java_version.and_then(|j| j.component.as_deref()) {
            crate::runtime::ensure_runtime(
                &options.game_directory,
                component,
                major,
                c,
                context.clone(),
            )
            .await
            .map_err(|e| {
                LoaderError::Unsupported(format!(
                    "could not prepare Java for OptiFine patcher: {e}"
                ))
            })?
        } else {
            java::detect_java(None, major).map_err(|e| {
                LoaderError::Unsupported(format!(
                    "Java {major} is required for OptiFine patcher: {e}"
                ))
            })?
        };
        let mut command = tokio::process::Command::new(runtime.executable);
        command
            .args([
                "-cp",
                &installer.to_string_lossy(),
                "optifine.Patcher",
                &client_jar.to_string_lossy(),
                &installer.to_string_lossy(),
                &output.to_string_lossy(),
            ])
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let run = command.output();
        tokio::pin!(run);
        let result = loop {
            tokio::select! {
                result=&mut run=>break result.map_err(|e|LoaderError::Unsupported(format!("could not run OptiFine patcher: {e}")))?,
                _=tokio::time::sleep(std::time::Duration::from_millis(100))=>check_cancelled(&context)?,
            }
        };
        if !result.status.success() {
            return Err(LoaderError::Unsupported(format!(
                "OptiFine patcher failed: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            )));
        }
    } else {
        if !zip_has_entry(&installer, "optifine/OptiFineTweaker.class")? {
            return Err(LoaderError::Unsupported(
                "OptiFine archive has neither a known patcher nor a launch tweaker".into(),
            ));
        }
        fs::copy(&installer, &output).map_err(|e| io(&output, e))?;
    }
    check_cancelled(&context)?;
    let profile_path = options
        .game_directory
        .join("versions")
        .join(&request.minecraft)
        .join(format!("{}.json", request.minecraft));
    let vanilla_profile: serde_json::Value =
        serde_json::from_slice(&fs::read(&profile_path).map_err(|e| io(&profile_path, e))?)
            .map_err(crate::metadata::MetadataError::from)?;
    let mut profile_value =
        create_optifine_child_profile(&vanilla_profile, &version_id, &request.minecraft)?;
    let libraries = profile_value["libraries"]
        .as_array_mut()
        .expect("OptiFine child profile creates a libraries array");
    let optifine_relative = output
        .strip_prefix(options.game_directory.join("libraries"))
        .map_err(|_| LoaderError::Unsupported("OptiFine library escaped game directory".into()))?
        .to_string_lossy()
        .replace('\\', "/");
    libraries.push(serde_json::json!({"name": format!("optifine:OptiFine:{}_{}", request.minecraft, request.version), "downloads":{"artifact":{"path":optifine_relative, "sha1":sha1_file(&output)?}}}));
    let mut embedded_launchwrappers =
        extract_embedded_launchwrappers(&installer, &options.game_directory)?;
    if embedded_launchwrappers.is_empty() {
        let launchwrapper = "https://libraries.minecraft.net/net/minecraft/launchwrapper/1.12/launchwrapper-1.12.jar";
        let launchwrapper_hash = fetch_sha1(c, &format!("{launchwrapper}.sha1")).await?;
        let launchwrapper_path = options
            .game_directory
            .join("libraries/net/minecraft/launchwrapper/1.12/launchwrapper-1.12.jar");
        let launchwrapper_candidates =
            https_candidates(source::url_candidates(launchwrapper, DownloadKind::File, c))?;
        let mut launchwrapper_request =
            DownloadRequest::new(launchwrapper_candidates, &launchwrapper_path)
                .with_https()
                .with_sha1(launchwrapper_hash.clone());
        if let Some(ctx) = context.clone() {
            launchwrapper_request = launchwrapper_request.with_task_context(ctx);
        }
        DownloadManager::new(c)?
            .download(launchwrapper_request)
            .await?;
        embedded_launchwrappers.push(serde_json::json!({"name":"net.minecraft:launchwrapper:1.12", "downloads":{"artifact":{"path":"net/minecraft/launchwrapper/1.12/launchwrapper-1.12.jar", "url":launchwrapper, "sha1":launchwrapper_hash}}}));
    }
    libraries.append(&mut embedded_launchwrappers);
    let dir = options.game_directory.join("versions").join(&version_id);
    fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
    let path = dir.join(format!("{version_id}.json"));
    fs::write(
        &path,
        serde_json::to_vec_pretty(&profile_value).map_err(crate::metadata::MetadataError::from)?,
    )
    .map_err(|e| io(&path, e))?;
    Ok(version_id)
}

fn client(c: &DownloadConfig) -> Result<reqwest::Client, LoaderError> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(60))
        .user_agent(concat!("CialloCraftLauncher/", env!("CARGO_PKG_VERSION")));
    if let Some(proxy) = &c.http_proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
    }
    Ok(builder.build()?)
}
fn https_candidates(candidates: Vec<String>) -> Result<Vec<String>, LoaderError> {
    if candidates.is_empty() {
        return Err(LoaderError::Unsupported(
            "no OptiFine download source is configured".into(),
        ));
    }
    let mut result = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let parsed = reqwest::Url::parse(&candidate)
            .map_err(|e| LoaderError::Unsupported(format!("invalid OptiFine download URL: {e}")))?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() {
            return Err(LoaderError::Unsupported(
                "OptiFine downloads require HTTPS endpoints".into(),
            ));
        }
        result.push(candidate);
    }
    Ok(result)
}
fn lookup_minecraft(v: &str) -> &str {
    match v {
        "1.8" => "1.8.0",
        "1.9" => "1.9.0",
        other => other,
    }
}
fn safe_component(s: &str) -> Result<(), LoaderError> {
    if s.is_empty()
        || s == "."
        || s == ".."
        || s.contains(['/', '\\', ':'])
        || s.chars().any(char::is_control)
    {
        Err(LoaderError::Unsupported(format!(
            "unsafe OptiFine version component: {s}"
        )))
    } else {
        Ok(())
    }
}

fn create_optifine_child_profile(
    vanilla: &serde_json::Value,
    version_id: &str,
    minecraft: &str,
) -> Result<serde_json::Value, LoaderError> {
    let mut profile = serde_json::json!({
        "id": version_id,
        "inheritsFrom": minecraft,
        "jar": minecraft,
        "mainClass": "net.minecraft.launchwrapper.Launch",
        "libraries": []
    });
    if let Some(legacy_args) = vanilla
        .get("minecraftArguments")
        .and_then(serde_json::Value::as_str)
    {
        // Legacy string arguments replace the parent's value during inheritance,
        // so include the parent's arguments once and append OptiFine's tweak.
        profile["minecraftArguments"] =
            format!("{legacy_args} --tweakClass optifine.OptiFineTweaker").into();
    } else if vanilla
        .get("arguments")
        .and_then(|arguments| arguments.get("game"))
        .and_then(serde_json::Value::as_array)
        .is_some()
    {
        // Structured arguments merge with the parent; the child contributes only
        // the tweak so options such as --gameDir occur exactly once.
        profile["arguments"] = serde_json::json!({
            "game": ["--tweakClass", "optifine.OptiFineTweaker"]
        });
    } else {
        return Err(LoaderError::Unsupported(
            "vanilla profile uses an unsupported argument schema for OptiFine".into(),
        ));
    }
    Ok(profile)
}

fn extract_embedded_launchwrappers(
    installer: &Path,
    game_directory: &Path,
) -> Result<Vec<serde_json::Value>, LoaderError> {
    const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
    let file = fs::File::open(installer).map_err(|e| io(installer, e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        LoaderError::Unsupported(format!("invalid OptiFine installer archive: {e}"))
    })?;
    let mut embedded = Vec::new();
    if zip_entry_exists(&mut archive, "launchwrapper-2.0.jar")? {
        embedded.push((
            "launchwrapper-2.0.jar".to_owned(),
            "optifine:launchwrapper:2.0".to_owned(),
            "optifine/launchwrapper/2.0/launchwrapper-2.0.jar".to_owned(),
        ));
    }
    if zip_entry_exists(&mut archive, "launchwrapper-of.txt")? {
        let mut version_entry = archive.by_name("launchwrapper-of.txt").map_err(|e| {
            LoaderError::Unsupported(format!("invalid OptiFine launchwrapper version entry: {e}"))
        })?;
        if version_entry.size() == 0 || version_entry.size() > 128 {
            return Err(LoaderError::Unsupported(
                "OptiFine launchwrapper version entry has an invalid size".into(),
            ));
        }
        let mut version_bytes = Vec::with_capacity(version_entry.size() as usize);
        std::io::Read::read_to_end(&mut version_entry, &mut version_bytes).map_err(|e| {
            LoaderError::Unsupported(format!(
                "could not read OptiFine launchwrapper version: {e}"
            ))
        })?;
        drop(version_entry);
        let version = std::str::from_utf8(&version_bytes)
            .map_err(|e| {
                LoaderError::Unsupported(format!(
                    "OptiFine launchwrapper version is not UTF-8: {e}"
                ))
            })?
            .trim();
        safe_component(version)?;
        let entry = format!("launchwrapper-of-{version}.jar");
        if zip_entry_exists(&mut archive, &entry)? {
            embedded.push((
                entry,
                format!("optifine:launchwrapper-of:{version}"),
                format!("optifine/launchwrapper-of/{version}/launchwrapper-of-{version}.jar"),
            ));
        }
    }
    let library_root = game_directory.join("libraries");
    let mut libraries = Vec::with_capacity(embedded.len());
    for (entry_name, coordinate, relative_path) in embedded {
        let destination = library_root.join(&relative_path);
        let mut source = archive.by_name(&entry_name).map_err(|e| {
            LoaderError::Unsupported(format!(
                "invalid embedded OptiFine launchwrapper {entry_name}: {e}"
            ))
        })?;
        if source.size() == 0 || source.size() > MAX_ENTRY_BYTES {
            return Err(LoaderError::Unsupported(format!(
                "embedded OptiFine launchwrapper {entry_name} exceeds the 64 MiB limit or is empty"
            )));
        }
        let mut bytes = Vec::with_capacity(source.size() as usize);
        std::io::Read::read_to_end(&mut source, &mut bytes).map_err(|e| {
            LoaderError::Unsupported(format!(
                "could not read embedded OptiFine launchwrapper {entry_name}: {e}"
            ))
        })?;
        if bytes.len() as u64 != source.size() {
            return Err(LoaderError::Unsupported(format!(
                "embedded OptiFine launchwrapper {entry_name} has an inconsistent size"
            )));
        }
        drop(source);
        validate_launchwrapper_jar(&bytes, &entry_name)?;
        write_embedded_library(&destination, &library_root, &bytes)?;
        libraries.push(serde_json::json!({
            "name": coordinate,
            "downloads": {"artifact": {
                "path": relative_path,
                "sha1": sha1_bytes(&bytes),
                "size": bytes.len()
            }}
        }));
    }
    Ok(libraries)
}

fn validate_launchwrapper_jar(bytes: &[u8], entry_name: &str) -> Result<(), LoaderError> {
    let cursor = std::io::Cursor::new(bytes);
    let archive = zip::ZipArchive::new(cursor).map_err(|e| {
        LoaderError::Unsupported(format!(
            "embedded OptiFine launchwrapper {entry_name} is not a valid JAR: {e}"
        ))
    })?;
    if !archive
        .file_names()
        .any(|name| name.is_ok_and(|name| name == "net/minecraft/launchwrapper/Launch.class"))
    {
        return Err(LoaderError::Unsupported(format!(
            "embedded OptiFine launchwrapper {entry_name} has no Launch class"
        )));
    }
    Ok(())
}

fn zip_entry_exists(
    archive: &mut zip::ZipArchive<fs::File>,
    name: &str,
) -> Result<bool, LoaderError> {
    match archive.by_name(name) {
        Ok(_) => Ok(true),
        Err(zip::result::ZipError::FileNotFound) => Ok(false),
        Err(error) => Err(LoaderError::Unsupported(format!(
            "could not inspect OptiFine archive entry {name}: {error}"
        ))),
    }
}

fn write_embedded_library(
    destination: &Path,
    root: &Path,
    bytes: &[u8],
) -> Result<(), LoaderError> {
    let relative = destination.strip_prefix(root).map_err(|_| {
        LoaderError::Unsupported("embedded OptiFine library escaped the library directory".into())
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&current)
            && metadata.file_type().is_symlink()
        {
            return Err(LoaderError::Conflict(format!(
                "installation path is a symbolic link: {}",
                current.display()
            )));
        }
    }
    let parent = destination.parent().ok_or_else(|| {
        LoaderError::Unsupported("embedded OptiFine library has no parent directory".into())
    })?;
    fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    if fs::symlink_metadata(destination).is_ok() {
        fs::remove_file(destination).map_err(|e| io(destination, e))?;
    }
    fs::write(destination, bytes).map_err(|e| io(destination, e))
}
fn verify_installer(
    path: &Path,
    minecraft: &str,
    requested_version: &str,
) -> Result<(), LoaderError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| io(path, e))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > 512 * 1024 * 1024
    {
        return Err(LoaderError::Unsupported(
            "OptiFine installer is not a regular file under the 512 MiB size limit".into(),
        ));
    }
    let file = fs::File::open(path).map_err(|e| io(path, e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        LoaderError::Unsupported(format!("invalid OptiFine installer archive: {e}"))
    })?;
    let config_entry = [
        "Config.class",
        "net/optifine/Config.class",
        "notch/net/optifine/Config.class",
    ]
    .into_iter()
    .find(|name| {
        archive
            .file_names()
            .any(|entry| entry.is_ok_and(|entry| entry == *name))
    })
    .ok_or_else(|| {
        LoaderError::Unsupported("OptiFine installer has no recognizable Config.class".into())
    })?;
    let mut entry = archive
        .by_name(config_entry)
        .map_err(|e| LoaderError::Unsupported(format!("invalid OptiFine Config.class: {e}")))?;
    if entry.size() > 16 * 1024 * 1024 {
        return Err(LoaderError::Unsupported(
            "OptiFine Config.class exceeds size limit".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    std::io::Read::read_to_end(&mut entry, &mut bytes).map_err(|e| {
        LoaderError::Unsupported(format!("could not read OptiFine Config.class: {e}"))
    })?;
    let constants = class_string_fields(&bytes)?;
    let field_after = |name: &str| constants.get(name).map(String::as_str);
    let embedded_version = field_after("OF_EDITION")
        .zip(field_after("OF_RELEASE"))
        .map(|(edition, release)| format!("{edition}_{release}"));
    if field_after("MC_VERSION") != Some(minecraft)
        || embedded_version.as_deref() != Some(requested_version)
    {
        return Err(LoaderError::Unsupported("OptiFine installer metadata does not match the requested Minecraft version and release".into()));
    }
    Ok(())
}

fn class_string_fields(
    bytes: &[u8],
) -> Result<std::collections::BTreeMap<String, String>, LoaderError> {
    fn invalid() -> LoaderError {
        LoaderError::Unsupported("invalid or truncated OptiFine Java class".into())
    }
    fn take<'a>(
        bytes: &'a [u8],
        offset: &mut usize,
        count: usize,
    ) -> Result<&'a [u8], LoaderError> {
        let end = offset.checked_add(count).ok_or_else(invalid)?;
        let value = bytes.get(*offset..end).ok_or_else(invalid)?;
        *offset = end;
        Ok(value)
    }
    fn word(bytes: &[u8], offset: &mut usize) -> Result<u16, LoaderError> {
        let b = take(bytes, offset, 2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    if bytes.get(..4) != Some(&[0xCA, 0xFE, 0xBA, 0xBE]) {
        return Err(invalid());
    }
    let mut offset = 8;
    let count = word(bytes, &mut offset)? as usize;
    let mut text = std::collections::BTreeMap::new();
    let mut references = std::collections::BTreeMap::new();
    let mut index = 1;
    while index < count {
        let tag = take(bytes, &mut offset, 1)?[0];
        match tag {
            1 => {
                let n = word(bytes, &mut offset)? as usize;
                text.insert(
                    index,
                    String::from_utf8_lossy(take(bytes, &mut offset, n)?).into_owned(),
                );
            }
            8 => {
                references.insert(index, word(bytes, &mut offset)? as usize);
            }
            3 | 4 => {
                take(bytes, &mut offset, 4)?;
            }
            5 | 6 => {
                take(bytes, &mut offset, 8)?;
                index += 1;
            }
            7 | 16 | 19 | 20 => {
                take(bytes, &mut offset, 2)?;
            }
            9 | 10 | 11 | 12 | 17 | 18 => {
                take(bytes, &mut offset, 4)?;
            }
            15 => {
                take(bytes, &mut offset, 3)?;
            }
            _ => return Err(invalid()),
        }
        index += 1;
    }
    take(bytes, &mut offset, 6)?;
    let interfaces = word(bytes, &mut offset)? as usize;
    take(bytes, &mut offset, interfaces * 2)?;
    let count = word(bytes, &mut offset)?;
    let mut fields = std::collections::BTreeMap::new();
    for _ in 0..count {
        take(bytes, &mut offset, 2)?;
        let name = word(bytes, &mut offset)? as usize;
        take(bytes, &mut offset, 2)?;
        let attrs = word(bytes, &mut offset)?;
        for _ in 0..attrs {
            let attribute = word(bytes, &mut offset)? as usize;
            let length = take(bytes, &mut offset, 4)?;
            let length = u32::from_be_bytes(length.try_into().unwrap()) as usize;
            let data = take(bytes, &mut offset, length)?;
            if text.get(&attribute).is_some_and(|s| s == "ConstantValue") && data.len() == 2 {
                let index = u16::from_be_bytes([data[0], data[1]]) as usize;
                if let Some(value) = references.get(&index).and_then(|i| text.get(i))
                    && let Some(name) = text.get(&name)
                {
                    fields.insert(name.clone(), value.clone());
                }
            }
        }
    }
    Ok(fields)
}
fn zip_has_entry(path: &Path, entry: &str) -> Result<bool, LoaderError> {
    let file = fs::File::open(path).map_err(|e| io(path, e))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| LoaderError::Unsupported(format!("invalid OptiFine installer: {e}")))?;
    Ok(zip.by_name(entry).is_ok())
}
async fn fetch_sha1(c: &DownloadConfig, url: &str) -> Result<String, LoaderError> {
    let client = client(c)?;
    let mut failures = Vec::new();
    for candidate in https_candidates(source::url_candidates(url, DownloadKind::File, c))? {
        match client.get(&candidate).send().await {
            Ok(response) => match response.error_for_status() {
                Ok(response) => match response.text().await {
                    Ok(body) => {
                        let hash = body.split_whitespace().next().unwrap_or_default();
                        if hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                            return Ok(hash.to_ascii_lowercase());
                        }
                        failures.push(format!("{candidate}: invalid SHA-1"));
                    }
                    Err(error) => failures.push(format!("{candidate}: {error}")),
                },
                Err(error) => failures.push(format!("{candidate}: {error}")),
            },
            Err(error) => failures.push(format!("{candidate}: {error}")),
        }
    }
    Err(LoaderError::Unsupported(format!(
        "OptiFine dependency checksum unavailable: {}",
        failures.join("; ")
    )))
}
fn sha1_file(path: &Path) -> Result<String, LoaderError> {
    use sha1::{Digest, Sha1};
    let mut file = fs::File::open(path).map_err(|e| io(path, e))?;
    let mut hash = Sha1::new();
    std::io::copy(&mut file, &mut hash).map_err(|e| io(path, e))?;
    Ok(format!("{:x}", hash.finalize()))
}
fn sha1_bytes(bytes: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    format!("{:x}", Sha1::digest(bytes))
}
fn check(context: &Option<TaskContext>) -> Result<(), LoaderError> {
    if context
        .as_ref()
        .is_some_and(|ctx| ctx.cancellation.is_cancelled())
    {
        Err(LoaderError::Cancelled)
    } else {
        Ok(())
    }
}
fn check_cancelled(context: &Option<TaskContext>) -> Result<(), LoaderError> {
    check(context)
}
fn io(path: &Path, source: std::io::Error) -> LoaderError {
    LoaderError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_optifine_fixture_versions_and_filters_other_minecraft_builds() {
        let versions = map_remote_versions(
            vec![
                RemoteVersion {
                    mcversion: "1.20.1".into(),
                    edition: "HD_U".into(),
                    patch: Some("I6".into()),
                },
                RemoteVersion {
                    mcversion: "1.20.1".into(),
                    edition: "HD_U".into(),
                    patch: Some("pre1".into()),
                },
                RemoteVersion {
                    mcversion: "1.19.4".into(),
                    edition: "HD_U".into(),
                    patch: Some("I4".into()),
                },
                RemoteVersion {
                    mcversion: "1.20.1".into(),
                    edition: "../bad".into(),
                    patch: Some("x".into()),
                },
            ],
            "1.20.1",
        );
        assert_eq!(
            versions,
            vec![
                LoaderVersion {
                    version: "HD_U_I6".into(),
                    stable: true
                },
                LoaderVersion {
                    version: "HD_U_pre1".into(),
                    stable: false
                },
            ]
        );
    }

    #[test]
    fn parses_real_constant_value_field_references_independent_of_pool_order() {
        let strings = [
            "I6",
            "OF_EDITION",
            "1.20.1",
            "OF_RELEASE",
            "HD_U",
            "MC_VERSION",
            "ConstantValue",
            "Ljava/lang/String;",
        ];
        let mut class = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 52, 0, 12];
        for value in strings {
            class.push(1);
            class.extend_from_slice(&(value.len() as u16).to_be_bytes());
            class.extend_from_slice(value.as_bytes());
        }
        for index in [1u16, 3, 5] {
            class.push(8);
            class.extend_from_slice(&index.to_be_bytes());
        }
        class.extend_from_slice(&[0, 1, 0, 1, 0, 1, 0, 0, 0, 3]);
        for (name, value) in [(6u16, 10u16), (2, 11), (4, 9)] {
            class.extend_from_slice(&[0, 25]);
            class.extend_from_slice(&name.to_be_bytes());
            class.extend_from_slice(&8u16.to_be_bytes());
            class.extend_from_slice(&[0, 1, 0, 7, 0, 0, 0, 2]);
            class.extend_from_slice(&value.to_be_bytes());
        }
        let fields = class_string_fields(&class).unwrap();
        assert_eq!(fields["MC_VERSION"], "1.20.1");
        assert_eq!(fields["OF_EDITION"], "HD_U");
        assert_eq!(fields["OF_RELEASE"], "I6");
        assert!(class_string_fields(&class[..class.len() - 1]).is_err());
    }

    #[test]
    fn optifine_profile_inherits_client_download_from_vanilla_owner() {
        let minecraft = "1.20.1";
        let base = crate::metadata::VersionMetadata::parse(
            r#"{"id":"1.20.1","downloads":{"client":{"sha1":"abc","size":12}},"arguments":{"game":["--gameDir","${game_directory}","--version","${version_name}"]}}"#,
        )
        .unwrap();
        let base_value = serde_json::to_value(&base).unwrap();
        let child_value =
            create_optifine_child_profile(&base_value, "1.20.1-OptiFine-HD_U_I6", minecraft)
                .unwrap();
        assert!(child_value["downloads"].get("client").is_none());
        assert_eq!(child_value["jar"], minecraft);
        assert_eq!(
            child_value["arguments"]["game"],
            serde_json::json!(["--tweakClass", "optifine.OptiFineTweaker"])
        );

        let child = crate::metadata::VersionMetadata::parse(&child_value.to_string()).unwrap();
        let versions = std::collections::HashMap::from([
            (base.id.clone().unwrap(), base),
            (child.id.clone().unwrap(), child),
        ]);
        let resolved =
            crate::metadata::resolve_inheritance("1.20.1-OptiFine-HD_U_I6", &versions).unwrap();
        assert_eq!(
            resolved.client().and_then(|client| client.sha1.as_deref()),
            Some("abc")
        );
        assert_eq!(resolved.extra["jar"], minecraft);
        let Some(crate::metadata::Arguments::Structured(arguments)) = resolved.arguments else {
            panic!("modern OptiFine child should preserve structured parent arguments");
        };
        let game_arguments = arguments
            .game
            .iter()
            .filter_map(|argument| match argument {
                crate::metadata::Argument::Plain(value) => Some(value.as_str()),
                crate::metadata::Argument::Conditional(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            game_arguments
                .iter()
                .filter(|arg| **arg == "--gameDir")
                .count(),
            1
        );
        assert_eq!(
            game_arguments
                .iter()
                .filter(|arg| **arg == "--tweakClass")
                .count(),
            1
        );
    }

    #[test]
    fn legacy_optifine_profile_replaces_legacy_arguments_without_losing_parent_options() {
        let base = crate::metadata::VersionMetadata::parse(
            r#"{"id":"1.8.9","minecraftArguments":"--username ${auth_player_name} --gameDir ${game_directory} --version ${version_name}"}"#,
        )
        .unwrap();
        let child_value = create_optifine_child_profile(
            &serde_json::to_value(&base).unwrap(),
            "1.8.9-OptiFine-HD_U_I7",
            "1.8.9",
        )
        .unwrap();
        let legacy = child_value["minecraftArguments"].as_str().unwrap();
        assert_eq!(legacy.matches("--gameDir").count(), 1);
        assert_eq!(legacy.matches("--tweakClass").count(), 1);
        assert!(legacy.starts_with("--username ${auth_player_name}"));
    }

    #[test]
    fn extracts_bounded_embedded_launchwrapper_variants_with_sha1_metadata() {
        use std::io::Write;

        let root = std::env::temp_dir().join(format!(
            "ccl-optifine-launchwrapper-{}-{}",
            std::process::id(),
            NEXT_INSTALLER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let nested = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let mut nested = nested;
        nested
            .start_file(
                "net/minecraft/launchwrapper/Launch.class",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        nested.write_all(b"fixture launchwrapper class").unwrap();
        let nested_jar = nested.finish().unwrap().into_inner();

        let installer = root.join("OptiFine.jar");
        let file = fs::File::create(&installer).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file("launchwrapper-of.txt", options).unwrap();
        archive.write_all(b"2.3\n").unwrap();
        archive
            .start_file("launchwrapper-of-2.3.jar", options)
            .unwrap();
        archive.write_all(&nested_jar).unwrap();
        archive
            .start_file("launchwrapper-2.0.jar", options)
            .unwrap();
        archive.write_all(&nested_jar).unwrap();
        archive.finish().unwrap();

        let libraries = extract_embedded_launchwrappers(&installer, &root).unwrap();
        assert_eq!(libraries.len(), 2);
        let versioned =
            root.join("libraries/optifine/launchwrapper-of/2.3/launchwrapper-of-2.3.jar");
        let stable = root.join("libraries/optifine/launchwrapper/2.0/launchwrapper-2.0.jar");
        assert_eq!(fs::read(&versioned).unwrap(), nested_jar);
        assert_eq!(fs::read(&stable).unwrap(), nested_jar);
        assert_eq!(libraries[0]["name"], "optifine:launchwrapper:2.0");
        assert_eq!(libraries[1]["name"], "optifine:launchwrapper-of:2.3");
        assert_eq!(
            libraries[0]["downloads"]["artifact"]["sha1"],
            sha1_bytes(&nested_jar)
        );
        assert_eq!(
            libraries[1]["downloads"]["artifact"]["size"],
            nested_jar.len()
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_unsafe_embedded_launchwrapper_version_before_creating_library_paths() {
        use std::io::Write;

        let root = std::env::temp_dir().join(format!(
            "ccl-optifine-launchwrapper-invalid-{}-{}",
            std::process::id(),
            NEXT_INSTALLER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let installer = root.join("OptiFine.jar");
        let file = fs::File::create(&installer).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file(
                "launchwrapper-of.txt",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"../outside").unwrap();
        archive.finish().unwrap();
        assert!(extract_embedded_launchwrappers(&installer, &root).is_err());
        assert!(!root.join("libraries").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
