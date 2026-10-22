//! Forge and NeoForge installer metadata and processor execution.

use crate::{
    TaskContext, TaskEvent,
    config::DownloadConfig,
    download::{DownloadManager, DownloadRequest},
    install::{InstallOptions, InstallService},
    loaders::{LoaderError, LoaderKind, LoaderRequest, LoaderVersion},
    metadata::VersionMetadata,
    source::{self, DownloadKind},
};
use serde::Deserialize;
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Stdio,
};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const FORGE_MAVEN: &str = "https://maven.minecraftforge.net";
const NEO_MAVEN: &str = "https://maven.neoforged.net/releases";

/// List installable versions for a Forge family and Minecraft release.
pub async fn versions(
    config: &DownloadConfig,
    kind: LoaderKind,
    minecraft: &str,
) -> Result<Vec<LoaderVersion>, LoaderError> {
    versions_with_context(config, kind, minecraft, &None).await
}

async fn versions_with_context(
    config: &DownloadConfig,
    kind: LoaderKind,
    minecraft: &str,
    context: &Option<TaskContext>,
) -> Result<Vec<LoaderVersion>, LoaderError> {
    safe_segment(minecraft)?;
    let root = maven_root(kind)?;
    let (path, prefix) = match kind {
        LoaderKind::Forge => (
            "net/minecraftforge/forge/maven-metadata.xml",
            format!("{minecraft}-"),
        ),
        LoaderKind::NeoForge => ("net/neoforged/neoforge/maven-metadata.xml", String::new()),
        LoaderKind::Fabric => {
            return Err(LoaderError::Unsupported(
                "Forge version request cannot use Fabric".into(),
            ));
        }
    };
    let mut client_builder = reqwest::Client::builder()
        .user_agent(concat!("CialloCraftLauncher/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(30));
    if let Some(proxy) = &config.http_proxy {
        client_builder = client_builder.proxy(reqwest::Proxy::all(proxy)?);
    }
    let http = client_builder.build()?;
    let url = format!("{root}/{path}");
    let mut errors = Vec::new();
    for candidate in maven_candidates(&url, DownloadKind::VersionList, config) {
        let result = async {
            http.get(&candidate)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await
        };
        let xml = tokio::select! {
            result = result => match result {
                Ok(text) => text,
                Err(e) => { errors.push(format!("{candidate}: {e}")); continue; }
            },
            _ = wait_cancel(context) => return Err(LoaderError::Cancelled),
        };
        let neo_prefix = if kind == LoaderKind::NeoForge {
            format!("{}.", minecraft.strip_prefix("1.").unwrap_or(minecraft))
        } else {
            String::new()
        };
        let mut values = xml
            .split("<version>")
            .skip(1)
            .filter_map(|s| s.split_once("</version>").map(|(v, _)| v.trim().to_owned()))
            .filter_map(|v| {
                let version = if prefix.is_empty() {
                    v
                } else {
                    v.strip_prefix(&prefix)?.to_owned()
                };
                if safe_segment(&version).is_err() {
                    return None;
                }
                if !neo_prefix.is_empty() && !version.starts_with(&neo_prefix) {
                    return None;
                }
                let lower = version.to_ascii_lowercase();
                (!version.is_empty()).then_some(LoaderVersion {
                    stable: !["beta", "alpha", "rc", "pre", "snapshot"]
                        .iter()
                        .any(|marker| lower.contains(marker)),
                    version,
                })
            })
            .collect::<Vec<_>>();
        if values.is_empty() {
            errors.push(format!(
                "{candidate}: no versions found for Minecraft {minecraft}"
            ));
            continue;
        }
        values.reverse();
        return Ok(values);
    }
    Err(LoaderError::Unsupported(format!(
        "loader versions unavailable: {}",
        errors.join("; ")
    )))
}

/// Install a supported modern Forge/NeoForge profile and return its version id.
pub async fn install(
    config: &DownloadConfig,
    request: &LoaderRequest,
    options: &InstallOptions,
    context: Option<TaskContext>,
) -> Result<String, LoaderError> {
    safe_segment(&request.minecraft)?;
    safe_segment(&request.version)?;
    check(&context)?;
    if request.kind == LoaderKind::Fabric {
        return Err(LoaderError::Unsupported(
            "Forge installer cannot install Fabric".into(),
        ));
    }
    let choices = versions_with_context(config, request.kind, &request.minecraft, &context).await?;
    if !choices.iter().any(|v| v.version == request.version) {
        return Err(LoaderError::Unsupported(format!(
            "{} {} is not published for Minecraft {}",
            kind_label(request.kind),
            request.version,
            request.minecraft
        )));
    }
    // Populate the vanilla client and libraries expected by processor inputs.
    InstallService::new(config.clone())?
        .install(&request.minecraft, options, context.clone())
        .await?;
    let base = maven_root(request.kind)?;
    let coordinate = if request.kind == LoaderKind::Forge {
        format!("{}-{}", request.minecraft, request.version)
    } else {
        request.version.clone()
    };
    let artifact = if request.kind == LoaderKind::Forge {
        "forge"
    } else {
        "neoforge"
    };
    let installer_url = format!(
        "{base}/net/{}/{artifact}/{coordinate}/{artifact}-{coordinate}-installer.jar",
        if request.kind == LoaderKind::Forge {
            "minecraftforge"
        } else {
            "neoforged"
        }
    );
    let installer = options
        .game_directory
        .join(".ccl")
        .join("installers")
        .join(format!("{}-{coordinate}-installer.jar", artifact));
    let downloader = DownloadManager::new(config)?;
    let expected_sha1 = fetch_sha1(config, &format!("{installer_url}.sha1"), &context).await?;
    let mut download = DownloadRequest::new(
        maven_candidates(&installer_url, DownloadKind::File, config),
        &installer,
    )
    .with_sha1(expected_sha1);
    if let Some(ctx) = context.clone() {
        download = download.with_task_context(ctx);
    }
    downloader.download(download).await?;
    let archive = zip::ZipArchive::new(fs::File::open(&installer).map_err(|e| io(&installer, e))?)
        .map_err(|e| LoaderError::Unsupported(format!("invalid installer archive: {e}")))?;
    let mut archive = archive;
    let profile = read_json::<InstallProfile>(&mut archive, "install_profile.json")?;
    if profile.spec.is_some_and(|spec| spec != 1) {
        return Err(LoaderError::Unsupported(format!(
            "unsupported installer profile spec {}",
            profile.spec.unwrap()
        )));
    }
    if profile.minecraft.as_deref() != Some(request.minecraft.as_str()) {
        return Err(LoaderError::Unsupported(
            "installer metadata targets a different Minecraft version".into(),
        ));
    }
    if profile.processors.is_empty() {
        return Err(LoaderError::Unsupported("installer profile contains no client processors; legacy profile schemas are unsupported".into()));
    }
    let version_path = profile.json.as_deref().ok_or_else(|| {
        LoaderError::Unsupported("installer has no version.json reference".into())
    })?;
    safe_zip_entry(version_path)?;
    let mut version: VersionMetadata = read_json(&mut archive, version_path)?;
    let version_id = version
        .id
        .clone()
        .ok_or_else(|| LoaderError::Unsupported("modded version metadata has no id".into()))?;
    safe_segment(&version_id)?;
    if !version_id.contains(&request.version) {
        return Err(LoaderError::Unsupported(format!(
            "installer metadata version ID {version_id} does not match requested loader {}",
            request.version
        )));
    }
    if version.inherits_from.as_deref() != Some(request.minecraft.as_str()) {
        return Err(LoaderError::Unsupported(
            "modded version does not inherit from the requested Minecraft version".into(),
        ));
    }
    if version.main_class.as_deref().is_none_or(str::is_empty) {
        return Err(LoaderError::Unsupported(
            "modded version metadata has no mainClass".into(),
        ));
    }
    let lib_root = options.game_directory.join("libraries");
    fs::create_dir_all(&lib_root).map_err(|e| io(&lib_root, e))?;
    download_profile_libraries(
        &downloader,
        config,
        &lib_root,
        base,
        &profile.libraries,
        &context,
    )
    .await?;
    let vars = prepare_data(
        &mut archive,
        &profile,
        options,
        &installer,
        version_path,
        &lib_root,
        &version_id,
    )?;
    let vanilla_meta_path = options
        .game_directory
        .join("versions")
        .join(&request.minecraft)
        .join(format!("{}.json", request.minecraft));
    let vanilla_meta: VersionMetadata = serde_json::from_slice(
        &fs::read(&vanilla_meta_path).map_err(|e| io(&vanilla_meta_path, e))?,
    )
    .map_err(|e| {
        LoaderError::Unsupported(format!(
            "invalid vanilla metadata for processor runtime: {e}"
        ))
    })?;
    let java_info = vanilla_meta.java_version.as_ref();
    let major = java_info.and_then(|j| j.major_version).unwrap_or(17);
    let java = if let Some(component) = java_info.and_then(|j| j.component.as_deref()) {
        crate::runtime::ensure_runtime(
            &options.game_directory,
            component,
            major,
            config,
            context.clone(),
        )
        .await
        .map_err(|e| {
            LoaderError::Unsupported(format!(
                "could not prepare managed Java for installer processors: {e}"
            ))
        })?
    } else {
        crate::java::detect_java(None, major).map_err(|e| {
            LoaderError::Unsupported(format!(
                "Java {major} is required to execute installer processors: {e}"
            ))
        })?
    };
    for processor in profile.processors.iter().filter(|p| {
        p.sides
            .as_ref()
            .is_none_or(|s| s.iter().any(|x| x == "client"))
    }) {
        check(&context)?;
        let jar = artifact_path(
            &lib_root,
            processor.jar.as_deref().ok_or_else(|| {
                LoaderError::Unsupported("processor has no executable jar".into())
            })?,
        )?;
        ensure_artifact(
            &mut archive,
            &downloader,
            config,
            &jar,
            processor.jar.as_deref().unwrap(),
            &context,
        )
        .await?;
        let mut cp = Vec::new();
        for coord in &processor.classpath {
            let p = artifact_path(&lib_root, coord)?;
            ensure_artifact(&mut archive, &downloader, config, &p, coord, &context).await?;
            cp.push(p);
        }
        cp.push(jar.clone());
        let main_class = jar_main_class(&jar)?;
        let mut args = vec![
            "-cp".into(),
            std::env::join_paths(&cp)
                .map_err(|e| LoaderError::Unsupported(e.to_string()))?
                .to_string_lossy()
                .into_owned(),
            main_class,
        ];
        for arg in &processor.args {
            args.push(resolve_literal(arg, &vars, &lib_root)?);
        }
        constrain_processor_outputs(&mut args, &options.game_directory)?;
        let output_hashes = resolve_outputs(
            &processor.outputs,
            &vars,
            &lib_root,
            &options.game_directory,
        )?;
        if !output_hashes.is_empty()
            && output_hashes
                .iter()
                .all(|(path, hash)| file_hash(path).as_deref() == Some(hash))
        {
            continue;
        }
        if let Some(c) = &context {
            c.emit(TaskEvent::Log {
                stream: "installer".into(),
                line: format!(
                    "Running {}",
                    jar.file_name().unwrap_or_default().to_string_lossy()
                ),
            })
            .map_err(|_| LoaderError::Cancelled)?;
        }
        let mut child = Command::new(&java.executable)
            .args(&args)
            .current_dir(&options.game_directory)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                LoaderError::Unsupported(format!("could not start installer processor: {e}"))
            })?;
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let stdout_task = tokio::spawn(stream_process_output(stdout, "stdout", context.clone()));
        let stderr_task = tokio::spawn(stream_process_output(stderr, "stderr", context.clone()));
        let status = tokio::select! { status = child.wait() => status.map_err(|e| LoaderError::Unsupported(format!("installer processor failed: {e}")))?, _ = wait_cancel(&context) => { let _ = child.kill().await; return Err(LoaderError::Cancelled); } };
        let _ = stdout_task.await;
        let _ = stderr_task.await;
        if !status.success() {
            return Err(LoaderError::Unsupported(format!(
                "installer processor exited with {status}"
            )));
        }
        for (path, expected) in output_hashes {
            let actual = file_hash(&path).ok_or_else(|| {
                LoaderError::Unsupported(format!(
                    "processor did not create output {}",
                    path.display()
                ))
            })?;
            if actual != expected {
                return Err(LoaderError::Unsupported(format!(
                    "processor output hash mismatch at {}",
                    path.display()
                )));
            }
        }
    }
    let version_dir = options.game_directory.join("versions").join(&version_id);
    fs::create_dir_all(&version_dir).map_err(|e| io(&version_dir, e))?;
    crate::loaders::install_libraries(
        config,
        &options.game_directory,
        &mut version,
        options,
        &context,
    )
    .await?;
    let json_path = version_dir.join(format!("{version_id}.json"));
    let encoded =
        serde_json::to_vec_pretty(&version).map_err(|e| LoaderError::Unsupported(e.to_string()))?;
    atomic_write(&json_path, &encoded)?;
    Ok(version_id)
}

#[derive(Deserialize)]
struct InstallProfile {
    spec: Option<u32>,
    minecraft: Option<String>,
    json: Option<String>,
    #[serde(default)]
    data: HashMap<String, serde_json::Value>,
    #[serde(default)]
    processors: Vec<Processor>,
    #[serde(default)]
    libraries: Vec<ProfileLibrary>,
}

fn constrain_processor_outputs(args: &mut [String], root: &Path) -> Result<(), LoaderError> {
    for index in 0..args.len().saturating_sub(1) {
        if args[index] != "--to" && args[index] != "--output" {
            continue;
        }
        let path = PathBuf::from(&args[index + 1]);
        if path.is_absolute() {
            if !path.starts_with(root) {
                return Err(LoaderError::Unsupported(format!(
                    "processor output escapes staging directory: {}",
                    path.display()
                )));
            }
        } else {
            if path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Err(LoaderError::Unsupported(format!(
                    "unsafe relative processor output: {}",
                    path.display()
                )));
            }
            args[index + 1] = root.join(path).to_string_lossy().into_owned();
        }
    }
    Ok(())
}
#[derive(Deserialize)]
struct ProfileLibrary {
    name: String,
    #[serde(default)]
    downloads: Option<ProfileLibraryDownloads>,
}
#[derive(Deserialize)]
struct ProfileLibraryDownloads {
    artifact: Option<ProfileArtifact>,
}
#[derive(Deserialize)]
struct ProfileArtifact {
    path: Option<String>,
    url: Option<String>,
    sha1: Option<String>,
    size: Option<u64>,
}
#[derive(Deserialize)]
struct Processor {
    sides: Option<Vec<String>>,
    jar: Option<String>,
    #[serde(default)]
    classpath: Vec<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    outputs: HashMap<String, String>,
}
fn maven_root(k: LoaderKind) -> Result<&'static str, LoaderError> {
    match k {
        LoaderKind::Forge => Ok(FORGE_MAVEN),
        LoaderKind::NeoForge => Ok(NEO_MAVEN),
        _ => Err(LoaderError::Unsupported("unsupported loader kind".into())),
    }
}
fn kind_label(k: LoaderKind) -> &'static str {
    if k == LoaderKind::Forge {
        "Forge"
    } else {
        "NeoForge"
    }
}
fn safe_segment(s: &str) -> Result<(), LoaderError> {
    if s.is_empty()
        || s == "."
        || s == ".."
        || s.contains('/')
        || s.contains('\\')
        || s.chars().any(char::is_control)
    {
        Err(LoaderError::Unsupported(format!(
            "unsafe version segment {s:?}"
        )))
    } else {
        Ok(())
    }
}
fn safe_zip_entry(s: &str) -> Result<String, LoaderError> {
    let normalized = s.trim_start_matches('/');
    let p = Path::new(normalized);
    if normalized.is_empty() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        Err(LoaderError::Unsupported(format!(
            "unsafe installer entry {s:?}"
        )))
    } else {
        Ok(normalized.to_owned())
    }
}
fn read_json<T: for<'de> Deserialize<'de>>(
    z: &mut zip::ZipArchive<fs::File>,
    name: &str,
) -> Result<T, LoaderError> {
    let normalized_name = safe_zip_entry(name)?;
    let mut f = z
        .by_name(&normalized_name)
        .map_err(|e| LoaderError::Unsupported(format!("installer missing {name}: {e}")))?;
    if f.size() > 64 * 1024 * 1024 {
        return Err(LoaderError::Unsupported(format!(
            "installer metadata entry {name} exceeds the size limit"
        )));
    }
    let mut b = Vec::new();
    f.read_to_end(&mut b)
        .map_err(|e| LoaderError::Unsupported(e.to_string()))?;
    serde_json::from_slice(&b).map_err(|e| LoaderError::Unsupported(format!("invalid {name}: {e}")))
}

async fn stream_process_output<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    stream: &'static str,
    context: Option<TaskContext>,
) {
    let mut buffer = [0u8; 4096];
    loop {
        let count = match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(count) => count,
        };
        if let Some(context) = &context {
            let line = String::from_utf8_lossy(&buffer[..count]).into_owned();
            let _ = context.emit(TaskEvent::Log {
                stream: stream.to_owned(),
                line,
            });
        }
    }
}
fn prepare_data(
    z: &mut zip::ZipArchive<fs::File>,
    p: &InstallProfile,
    o: &InstallOptions,
    installer: &Path,
    _version_path: &str,
    lib: &Path,
    version_id: &str,
) -> Result<HashMap<String, String>, LoaderError> {
    let mut vars = HashMap::new();
    let temp = o
        .game_directory
        .join(".ccl")
        .join("forge-data")
        .join(version_id);
    fs::create_dir_all(&temp).map_err(|e| io(&temp, e))?;
    for (key, value) in &p.data {
        let value = value
            .get("client")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                LoaderError::Unsupported(format!("processor data {key} has no client value"))
            })?;
        let parsed = if value.starts_with('[') && value.ends_with(']') {
            artifact_path(lib, &value[1..value.len() - 1])?
                .to_string_lossy()
                .into_owned()
        } else if value.starts_with('\'') && value.ends_with('\'') {
            value[1..value.len() - 1].into()
        } else {
            let name = safe_zip_entry(value)?;
            let out = temp.join(format!("data-{}", vars.len()));
            let mut entry = z.by_name(&name).map_err(|e| {
                LoaderError::Unsupported(format!("processor data entry {value} unavailable: {e}"))
            })?;
            if entry.size() > 1024 * 1024 * 1024 {
                return Err(LoaderError::Unsupported(format!(
                    "processor data entry {value} exceeds the extraction size limit"
                )));
            }
            let mut file = fs::File::create(&out).map_err(|e| io(&out, e))?;
            std::io::copy(&mut entry, &mut file).map_err(|e| io(&out, e))?;
            out.to_string_lossy().into_owned()
        };
        vars.insert(key.clone(), parsed);
    }
    vars.insert("SIDE".into(), "client".into());
    vars.insert(
        "MINECRAFT_VERSION".into(),
        p.minecraft.clone().unwrap_or_default(),
    );
    vars.insert(
        "ROOT".into(),
        o.game_directory.to_string_lossy().into_owned(),
    );
    vars.insert("LIBRARY_DIR".into(), lib.to_string_lossy().into_owned());
    vars.insert("INSTALLER".into(), installer.to_string_lossy().into_owned());
    let mc = o
        .game_directory
        .join("versions")
        .join(p.minecraft.as_deref().unwrap_or(""))
        .join(format!("{}.jar", p.minecraft.as_deref().unwrap_or("")));
    let disposable = temp.join("minecraft.jar");
    fs::copy(&mc, &disposable).map_err(|e| io(&mc, e))?;
    vars.insert(
        "MINECRAFT_JAR".into(),
        disposable.to_string_lossy().into_owned(),
    );
    Ok(vars)
}
fn resolve_literal(
    s: &str,
    vars: &HashMap<String, String>,
    lib: &Path,
) -> Result<String, LoaderError> {
    if s.starts_with('{') && s.ends_with('}') {
        return vars.get(&s[1..s.len() - 1]).cloned().ok_or_else(|| {
            LoaderError::Unsupported(format!("unsupported processor variable {s}"))
        });
    }
    if s.starts_with('\'') && s.ends_with('\'') {
        return Ok(s[1..s.len() - 1].into());
    }
    if s.starts_with('[') && s.ends_with(']') {
        return Ok(artifact_path(lib, &s[1..s.len() - 1])?
            .to_string_lossy()
            .into_owned());
    }
    let mut v = s.to_owned();
    for (k, x) in vars {
        v = v.replace(&format!("{{{k}}}"), x);
    }
    if v.contains('{') || v.contains('}') {
        return Err(LoaderError::Unsupported(format!(
            "unresolved processor token in {s}"
        )));
    }
    Ok(v)
}
fn resolve_outputs(
    outputs: &HashMap<String, String>,
    vars: &HashMap<String, String>,
    lib: &Path,
    root: &Path,
) -> Result<Vec<(PathBuf, String)>, LoaderError> {
    outputs
        .iter()
        .map(|(k, v)| {
            let path = resolve_literal(k, vars, lib)?;
            let hash = resolve_literal(v, vars, lib)?;
            if hash.len() != 40 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err(LoaderError::Unsupported(format!(
                    "invalid declared output hash {hash}"
                )));
            }
            let p = PathBuf::from(path);
            if !p.is_absolute() || !p.starts_with(root) {
                return Err(LoaderError::Unsupported(format!(
                    "processor output escapes staging directory: {}",
                    p.display()
                )));
            }
            Ok((p, hash.to_ascii_lowercase()))
        })
        .collect()
}
fn artifact_path(lib: &Path, coord: &str) -> Result<PathBuf, LoaderError> {
    let (coord, ext) = coord.split_once('@').unwrap_or((coord, "jar"));
    let v: Vec<_> = coord.split(':').collect();
    if !(3..=4).contains(&v.len())
        || v.iter()
            .any(|x| x.is_empty() || x.contains('/') || x.contains('\\'))
    {
        return Err(LoaderError::Unsupported(format!(
            "unsupported artifact coordinate {coord}"
        )));
    }
    for x in &v {
        safe_segment(x)?;
    }
    safe_segment(ext)?;
    let mut rel = PathBuf::new();
    for c in v[0].split('.') {
        safe_segment(c)?;
        rel.push(c)
    }
    rel.push(v[1]);
    rel.push(v[2]);
    let name = format!(
        "{}-{}{}.{}",
        v[1],
        v[2],
        if v.len() == 4 {
            format!("-{}", v[3])
        } else {
            String::new()
        },
        ext
    );
    rel.push(name);
    Ok(lib.join(rel))
}
async fn ensure_artifact(
    z: &mut zip::ZipArchive<fs::File>,
    d: &DownloadManager,
    c: &DownloadConfig,
    path: &Path,
    coord: &str,
    ctx: &Option<TaskContext>,
) -> Result<(), LoaderError> {
    let rel = artifact_relative(coord)?;
    let embedded = format!("maven/{rel}");
    if let Ok(mut src) = z.by_name(&embedded) {
        if path.is_file() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| io(parent, e))?
        }
        let mut out = fs::File::create(path).map_err(|e| io(path, e))?;
        std::io::copy(&mut src, &mut out).map_err(|e| io(path, e))?;
        return Ok(());
    }
    let root = if coord.starts_with("net.neoforged") {
        NEO_MAVEN
    } else {
        FORGE_MAVEN
    };
    let url = format!("{root}/{rel}");
    let expected = fetch_sha1(c, &format!("{url}.sha1"), ctx).await?;
    if file_hash(path).as_deref() == Some(expected.as_str()) {
        return Ok(());
    }
    let mut req = DownloadRequest::new(maven_candidates(&url, DownloadKind::File, c), path)
        .with_sha1(expected);
    if let Some(context) = ctx.clone() {
        req = req.with_task_context(context)
    }
    d.download(req).await?;
    Ok(())
}

async fn download_profile_libraries(
    downloader: &DownloadManager,
    config: &DownloadConfig,
    library_root: &Path,
    default_maven: &str,
    libraries: &[ProfileLibrary],
    context: &Option<TaskContext>,
) -> Result<(), LoaderError> {
    for library in libraries {
        check(context)?;
        let descriptor = library.downloads.as_ref().and_then(|d| d.artifact.as_ref());
        let rel = match descriptor.and_then(|a| a.path.as_deref()) {
            Some(path) => safe_zip_entry(path)?,
            None => artifact_relative(&library.name)?,
        };
        let destination = library_root.join(&rel);
        if !destination.starts_with(library_root) {
            return Err(LoaderError::Unsupported(format!(
                "library path escapes libraries directory: {rel}"
            )));
        }
        let group = library
            .name
            .split(':')
            .next()
            .unwrap_or("")
            .replace('.', "/");
        let url = descriptor.and_then(|a| a.url.clone()).unwrap_or_else(|| {
            format!(
                "{default_maven}/{group}/{}",
                rel.strip_prefix(&format!("{group}/")).unwrap_or(&rel)
            )
        });
        let expected = match descriptor.and_then(|a| a.sha1.as_deref()) {
            Some(hash) if hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()) => {
                hash.to_ascii_lowercase()
            }
            Some(_) => {
                return Err(LoaderError::Unsupported(format!(
                    "invalid SHA-1 for installer library {}",
                    library.name
                )));
            }
            None => fetch_sha1(config, &format!("{url}.sha1"), context).await?,
        };
        if file_hash(&destination).as_deref() == Some(expected.as_str()) {
            continue;
        }
        let urls = source::url_candidates(&url, DownloadKind::File, config);
        let mut request = DownloadRequest::new(urls, &destination).with_sha1(expected);
        if let Some(size) = descriptor.and_then(|a| a.size) {
            request = request.with_size(size);
        }
        if let Some(ctx) = context.clone() {
            request = request.with_task_context(ctx);
        }
        downloader.download(request).await?;
    }
    Ok(())
}
fn artifact_relative(coord: &str) -> Result<String, LoaderError> {
    let path = artifact_path(Path::new(""), coord)?;
    path.strip_prefix(Path::new(""))
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .map_err(|_| LoaderError::Unsupported("invalid artifact path".into()))
}
async fn fetch_sha1(
    c: &DownloadConfig,
    url: &str,
    context: &Option<TaskContext>,
) -> Result<String, LoaderError> {
    let mut client = reqwest::Client::builder()
        .user_agent(concat!("CialloCraftLauncher/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(30));
    if let Some(proxy) = &c.http_proxy {
        client = client.proxy(reqwest::Proxy::all(proxy)?)
    }
    let client = client.build()?;
    let mut errors = Vec::new();
    for candidate in maven_candidates(url, DownloadKind::File, c) {
        let fetch = async {
            client
                .get(&candidate)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await
        };
        let response = tokio::select! {
            response = fetch => response,
            _ = wait_cancel(context) => return Err(LoaderError::Cancelled),
        };
        match response {
            Ok(s) => {
                let hash = s.split_whitespace().next().unwrap_or("").trim();
                if hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Ok(hash.to_ascii_lowercase());
                }
                errors.push(format!("{candidate}: malformed SHA-1"));
            }
            Err(e) => errors.push(format!("{candidate}: {e}")),
        }
    }
    Err(LoaderError::Unsupported(format!(
        "installer integrity metadata unavailable: {}",
        errors.join("; ")
    )))
}
fn maven_candidates(url: &str, kind: DownloadKind, c: &DownloadConfig) -> Vec<String> {
    source::url_candidates(url, kind, c)
}
fn jar_main_class(path: &Path) -> Result<String, LoaderError> {
    let f = fs::File::open(path).map_err(|e| io(path, e))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| LoaderError::Unsupported(e.to_string()))?;
    let mut m = z
        .by_name("META-INF/MANIFEST.MF")
        .map_err(|e| LoaderError::Unsupported(format!("processor jar has no manifest: {e}")))?;
    let mut s = String::new();
    m.read_to_string(&mut s)
        .map_err(|e| LoaderError::Unsupported(e.to_string()))?;
    s.lines()
        .find_map(|l| {
            l.strip_prefix("Main-Class:")
                .map(str::trim)
                .map(str::to_owned)
        })
        .ok_or_else(|| LoaderError::Unsupported("processor JAR has no Main-Class".into()))
}
fn file_hash(path: &Path) -> Option<String> {
    let b = fs::read(path).ok()?;
    Some(format!("{:x}", Sha1::digest(b)))
}
fn atomic_write(path: &Path, b: &[u8]) -> Result<(), LoaderError> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, b).map_err(|e| io(&tmp, e))?;
    fs::rename(&tmp, path).map_err(|e| io(path, e))
}
fn io(p: &Path, e: std::io::Error) -> LoaderError {
    LoaderError::Io {
        path: p.to_path_buf(),
        source: e,
    }
}
fn check(ctx: &Option<TaskContext>) -> Result<(), LoaderError> {
    if ctx.as_ref().is_some_and(|c| c.cancellation.is_cancelled()) {
        Err(LoaderError::Cancelled)
    } else {
        Ok(())
    }
}
async fn wait_cancel(ctx: &Option<TaskContext>) {
    loop {
        if ctx.as_ref().is_some_and(|c| c.cancellation.is_cancelled()) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(75)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, sync::Arc};

    #[test]
    fn processor_profile_fixture_reads_client_side_schema_and_data() {
        let path =
            std::env::temp_dir().join(format!("ccl-forge-profile-{}.jar", std::process::id()));
        let file = fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("install_profile.json", opts).unwrap();
        zip.write_all(br#"{"spec":1,"minecraft":"1.21.1","json":"/version.json","data":{"PATCHED":{"client":"[net.minecraftforge:forge:1.21.1-52.0.0:client]"}},"processors":[{"sides":["client"],"jar":"net.minecraftforge:installertools:1.4.0","classpath":[],"args":["--input","{PATCHED}"],"outputs":{}}]}"#).unwrap();
        zip.start_file("version.json", opts).unwrap();
        zip.write_all(br#"{"id":"1.21.1-forge-52.0.0","inheritsFrom":"1.21.1","libraries":[]}"#)
            .unwrap();
        zip.start_file("data/client.lzma", opts).unwrap();
        zip.write_all(b"fixture").unwrap();
        drop(zip.finish().unwrap());
        let file = fs::File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let profile: InstallProfile = read_json(&mut archive, "install_profile.json").unwrap();
        let version: VersionMetadata =
            read_json(&mut archive, profile.json.as_deref().unwrap()).unwrap();
        assert_eq!(profile.minecraft.as_deref(), Some("1.21.1"));
        assert_eq!(profile.processors[0].sides.as_ref().unwrap(), &["client"]);
        assert_eq!(version.inherits_from.as_deref(), Some("1.21.1"));
        assert_eq!(
            safe_zip_entry("/data/client.lzma").unwrap(),
            "data/client.lzma"
        );
        assert!(safe_zip_entry("../escape").is_err());
        drop(archive);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn artifact_coordinates_preserve_classifier_and_extension() {
        let path = artifact_path(
            Path::new("/tmp/libraries"),
            "net.neoforged:installertools:2.1.0:fatjar@jar",
        )
        .unwrap();
        assert_eq!(
            path,
            Path::new(
                "/tmp/libraries/net/neoforged/installertools/2.1.0/installertools-2.1.0-fatjar.jar"
            )
        );
        assert!(artifact_path(Path::new("/tmp/libraries"), "../../evil:artifact:1").is_err());
    }

    #[test]
    fn processor_output_paths_cannot_escape_staging_root() {
        let root = std::env::temp_dir().join("ccl-staging");
        let mut args = vec!["--output".into(), "../outside.jar".into()];
        assert!(constrain_processor_outputs(&mut args, &root).is_err());
        let mut safe = vec!["--to".into(), "result.jar".into()];
        constrain_processor_outputs(&mut safe, &root).unwrap();
        assert_eq!(PathBuf::from(&safe[1]), root.join("result.jar"));
    }

    #[tokio::test]
    async fn installer_process_wait_is_interruptible() {
        let token = crate::CancellationToken::default();
        let signal = token.clone();
        let context = TaskContext::new(token, Arc::new(|_| {}));
        let wait = tokio::spawn(async move { wait_cancel(&Some(context)).await });
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        signal.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), wait)
            .await
            .unwrap()
            .unwrap();
    }
}
