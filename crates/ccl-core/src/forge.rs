//! Forge and NeoForge installer metadata and processor execution.

use crate::{
    TaskContext, TaskEvent,
    config::{DownloadConfig, SourceMode},
    download::{DownloadManager, DownloadRequest},
    install::{InstallOptions, InstallService},
    loaders::{LoaderError, LoaderKind, LoaderRequest, LoaderVersion},
    metadata::VersionMetadata,
    source::{self, DownloadKind},
};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::future::Future;
use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const FORGE_MAVEN: &str = "https://maven.minecraftforge.net";
const NEO_MAVEN: &str = "https://maven.neoforged.net/releases";
static NEXT_EMBEDDED_TEMP: AtomicU64 = AtomicU64::new(0);

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
                if kind == LoaderKind::NeoForge
                    && neoforge_minecraft_version(&version).as_deref() != Some(minecraft)
                {
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
        sort_releases(&mut values);
        return Ok(values);
    }

    // BMCLAPI provides dedicated loader lists when Maven metadata is
    // unavailable. Only query the selected mirror when lists are not forced
    // to official sources.
    if config.version_list_source != SourceMode::Official {
        let fallback = match kind {
            LoaderKind::Forge => bmcl_forge_versions(config, minecraft, context, &http).await,
            LoaderKind::NeoForge => bmcl_neoforge_versions(config, minecraft, context, &http).await,
            LoaderKind::Fabric => unreachable!("Fabric is rejected above"),
        };
        match fallback {
            Ok(mut values) if !values.is_empty() => {
                sort_releases(&mut values);
                return Ok(values);
            }
            Ok(_) => errors.push(format!(
                "BMCLAPI {} list returned no installer releases for Minecraft {minecraft}",
                kind_label(kind)
            )),
            Err(LoaderError::Cancelled) => return Err(LoaderError::Cancelled),
            Err(e) => errors.push(format!("BMCLAPI {} list: {e}", kind_label(kind))),
        }
    }
    Err(LoaderError::Unsupported(format!(
        "loader versions unavailable: {}",
        errors.join("; ")
    )))
}

#[derive(Deserialize)]
struct BmclNeoForgeVersion {
    version: String,
    mcversion: String,
}

#[derive(Deserialize)]
struct BmclForgeVersion {
    branch: Option<String>,
    mcversion: String,
    version: String,
    #[serde(default)]
    files: Vec<BmclForgeFile>,
}

#[derive(Deserialize)]
struct BmclForgeFile {
    format: String,
    category: String,
}

async fn bmcl_forge_versions(
    config: &DownloadConfig,
    minecraft: &str,
    context: &Option<TaskContext>,
    http: &reqwest::Client,
) -> Result<Vec<LoaderVersion>, LoaderError> {
    let lookup_version = if minecraft == "1.7.10-pre4" {
        "1.7.10_pre4"
    } else {
        minecraft
    };
    let url = format!(
        "{}/forge/minecraft/{lookup_version}",
        config.mirror_root.trim_end_matches('/')
    );
    let fetch = async {
        http.get(&url)
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<BmclForgeVersion>>()
            .await
    };
    let entries = tokio::select! {
        result = fetch => result?,
        _ = wait_cancel(context) => return Err(LoaderError::Cancelled),
    };
    Ok(map_bmcl_forge_versions(entries, minecraft))
}

fn map_bmcl_forge_versions(entries: Vec<BmclForgeVersion>, minecraft: &str) -> Vec<LoaderVersion> {
    entries
        .into_iter()
        .filter(|entry| entry.mcversion == minecraft)
        .filter(|entry| {
            entry
                .files
                .iter()
                .any(|file| file.category == "installer" && file.format == "jar")
        })
        .filter_map(|entry| {
            safe_segment(&entry.version).ok()?;
            let branch = entry.branch.filter(|branch| !branch.is_empty());
            if branch
                .as_deref()
                .is_some_and(|branch| safe_segment(branch).is_err())
            {
                return None;
            }
            let version = branch.as_ref().map_or_else(
                || entry.version.clone(),
                |branch| format!("{}-{branch}", entry.version),
            );
            let lower = version.to_ascii_lowercase();
            Some(LoaderVersion {
                stable: branch.is_none()
                    && !["beta", "alpha", "rc", "pre", "snapshot"]
                        .iter()
                        .any(|marker| lower.contains(marker)),
                version,
            })
        })
        .collect()
}

async fn bmcl_neoforge_versions(
    config: &DownloadConfig,
    minecraft: &str,
    context: &Option<TaskContext>,
    http: &reqwest::Client,
) -> Result<Vec<LoaderVersion>, LoaderError> {
    let url = format!(
        "{}/neoforge/list/{minecraft}",
        config.mirror_root.trim_end_matches('/')
    );
    let fetch = async {
        http.get(&url)
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<BmclNeoForgeVersion>>()
            .await
    };
    let entries = tokio::select! {
        result = fetch => result?,
        _ = wait_cancel(context) => return Err(LoaderError::Cancelled),
    };
    Ok(map_bmcl_neoforge_versions(entries, minecraft))
}

fn map_bmcl_neoforge_versions(
    entries: Vec<BmclNeoForgeVersion>,
    minecraft: &str,
) -> Vec<LoaderVersion> {
    entries
        .into_iter()
        .filter(|entry| entry.mcversion == minecraft)
        .filter_map(|entry| {
            safe_segment(&entry.version).ok()?;
            let lower = entry.version.to_ascii_lowercase();
            Some(LoaderVersion {
                stable: !["beta", "alpha", "rc", "pre", "snapshot"]
                    .iter()
                    .any(|marker| lower.contains(marker)),
                version: entry.version,
            })
        })
        .collect()
}

/// Convert a NeoForge release coordinate into the exact Minecraft release
/// described by NeoForge's published versioning scheme.
fn neoforge_minecraft_version(version: &str) -> Option<String> {
    let (release, mc_suffix) = version
        .split_once('+')
        .map_or((version, None), |(release, suffix)| (release, Some(suffix)));
    let segments: Vec<_> = release.split('.').collect();
    let parse_segment = |segment: &str| -> Option<u64> {
        let digits: String = segment.chars().take_while(char::is_ascii_digit).collect();
        (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
    };
    let result = match segments.as_slice() {
        // Snapshot coordinates encode the MC snapshot as the second segment.
        ["0", snapshot, ..] if !snapshot.is_empty() => (*snapshot).to_owned(),
        [major, minor, patch, ..] => {
            let major = parse_segment(major)?;
            let minor = parse_segment(minor)?;
            let patch = parse_segment(patch)?;
            if major >= 26 {
                if segments.len() < 4 {
                    return None;
                }
                if patch == 0 {
                    format!("{major}.{minor}")
                } else {
                    format!("{major}.{minor}.{patch}")
                }
            } else if minor == 0 {
                format!("1.{major}")
            } else {
                format!("1.{major}.{minor}")
            }
        }
        _ => return None,
    };
    let result = match mc_suffix {
        Some(suffix) if !suffix.is_empty() => format!("{result}-{suffix}"),
        Some(_) => return None,
        None => result,
    };
    Some(result)
}

fn sort_releases(values: &mut Vec<LoaderVersion>) {
    fn parts(version: &str) -> Vec<u64> {
        version
            .split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.parse().ok())
            .collect()
    }
    values.sort_by(|a, b| {
        parts(&b.version)
            .cmp(&parts(&a.version))
            .then_with(|| b.stable.cmp(&a.stable))
            .then_with(|| b.version.cmp(&a.version))
    });
    values.dedup_by(|a, b| a.version == b.version);
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
    let checksum_url = format!("{installer_url}.sha1");
    let integrity_record = installer_integrity_record_path(&installer);
    let expected_sha1 = expected_installer_sha1(
        &installer,
        &integrity_record,
        &installer_url,
        &context,
        || fetch_sha1(config, &checksum_url, &context),
    )
    .await?;
    let mut download = DownloadRequest::new(
        maven_candidates(&installer_url, DownloadKind::File, config),
        &installer,
    )
    .with_sha1(expected_sha1.clone());
    if let Some(ctx) = context.clone() {
        download = download.with_task_context(ctx);
    }
    downloader.download(download).await?;
    write_installer_integrity_record(
        &installer,
        &integrity_record,
        &installer_url,
        &expected_sha1,
    )?;
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
    if !installer_version_matches(
        request.kind,
        &request.minecraft,
        &request.version,
        &version_id,
    ) {
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
        let processor_args = processor
            .args
            .iter()
            .map(|arg| resolve_literal(arg, &vars, &lib_root))
            .collect::<Result<Vec<_>, _>>()?;
        let output_hashes = resolve_outputs(
            &processor.outputs,
            &vars,
            &lib_root,
            &options.game_directory,
        )?;
        if is_client_mojmaps_task(
            processor.jar.as_deref(),
            &processor_args,
            &request.minecraft,
        ) {
            download_client_mappings(MojmapsDownload {
                config,
                downloader: &downloader,
                vanilla: &vanilla_meta,
                vars: &vars,
                args: &processor_args,
                output_hashes: &output_hashes,
                game_directory: &options.game_directory,
                context: &context,
            })
            .await?;
            continue;
        }
        let jar = artifact_path(
            &lib_root,
            processor.jar.as_deref().ok_or_else(|| {
                LoaderError::Unsupported("processor has no executable jar".into())
            })?,
        )?;
        ensure_artifact(
            &mut archive,
            EnsureArtifact {
                downloader: &downloader,
                config,
                library_root: &lib_root,
                path: &jar,
                coordinate: processor.jar.as_deref().unwrap(),
                libraries: &profile.libraries,
                context: &context,
            },
        )
        .await?;
        let mut cp = Vec::new();
        for coord in &processor.classpath {
            let p = artifact_path(&lib_root, coord)?;
            ensure_artifact(
                &mut archive,
                EnsureArtifact {
                    downloader: &downloader,
                    config,
                    library_root: &lib_root,
                    path: &p,
                    coordinate: coord,
                    libraries: &profile.libraries,
                    context: &context,
                },
            )
            .await?;
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
        args.extend(processor_args);
        constrain_processor_outputs(&mut args, &options.game_directory)?;
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
        let mut command = Command::new(&java.executable);
        // The launcher handles terminal interruption before stopping Java, just
        // as it does for game launches, so cancellation cannot race a child exit.
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(0x0000_0200); // CREATE_NEW_PROCESS_GROUP
        let mut child = command
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

fn installer_version_matches(
    kind: LoaderKind,
    minecraft: &str,
    requested: &str,
    version_id: &str,
) -> bool {
    if kind == LoaderKind::Forge {
        let prefix = format!("{minecraft}-forge-");
        let Some(profile_version) = version_id.strip_prefix(&prefix) else {
            return false;
        };
        requested == profile_version
            || requested
                .strip_prefix(profile_version)
                .is_some_and(|suffix| suffix.starts_with('-') && suffix.len() > 1)
    } else {
        version_id.contains(requested)
    }
}

fn is_client_mojmaps_task(jar: Option<&str>, args: &[String], minecraft: &str) -> bool {
    let Some(coordinate) = jar else {
        return false;
    };
    let parts: Vec<_> = coordinate.split(':').collect();
    if parts.len() < 3
        || parts.len() > 4
        || parts[0] != "net.neoforged.installertools"
        || parts[1] != "installertools"
        || parts[2].is_empty()
        || artifact_path(Path::new("ccl-libraries"), coordinate).is_err()
        || args.len() != 8
        || !args
            .iter()
            .step_by(2)
            .all(|flag| ["--task", "--version", "--side", "--output"].contains(&flag.as_str()))
    {
        return false;
    }
    unique_flag_value(args, "--task") == Some("DOWNLOAD_MOJMAPS")
        && unique_flag_value(args, "--version") == Some(minecraft)
        && unique_flag_value(args, "--side") == Some("client")
        && unique_flag_value(args, "--output").is_some_and(|output| !output.is_empty())
}

fn unique_flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let mut values = args
        .windows(2)
        .filter(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str());
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

struct MojmapsDownload<'a> {
    config: &'a DownloadConfig,
    downloader: &'a DownloadManager,
    vanilla: &'a VersionMetadata,
    vars: &'a HashMap<String, String>,
    args: &'a [String],
    output_hashes: &'a [(PathBuf, String)],
    game_directory: &'a Path,
    context: &'a Option<TaskContext>,
}

async fn download_client_mappings(input: MojmapsDownload<'_>) -> Result<(), LoaderError> {
    let output = unique_flag_value(input.args, "--output").ok_or_else(|| {
        LoaderError::Unsupported("DOWNLOAD_MOJMAPS processor has no unique output path".into())
    })?;
    let output = PathBuf::from(output);
    let expected_path = input.vars.get("MOJMAPS").ok_or_else(|| {
        LoaderError::Unsupported("DOWNLOAD_MOJMAPS processor has no MOJMAPS data mapping".into())
    })?;
    let mapping = input
        .vanilla
        .downloads
        .get("client_mappings")
        .ok_or_else(|| {
            LoaderError::Unsupported("vanilla metadata has no client_mappings download".into())
        })?;
    let url = mapping
        .url
        .as_deref()
        .filter(|url| !url.is_empty())
        .ok_or_else(|| LoaderError::Unsupported("vanilla client_mappings has no URL".into()))?;
    let sha1 = mapping
        .sha1
        .as_deref()
        .ok_or_else(|| LoaderError::Unsupported("vanilla client_mappings has no SHA-1".into()))?;
    if sha1.len() != 40 || !sha1.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(LoaderError::Unsupported(
            "vanilla client_mappings has an invalid SHA-1".into(),
        ));
    }
    validate_mojmaps_target(
        &output,
        expected_path,
        input.game_directory,
        input.output_hashes,
        sha1,
    )?;
    let mut download = DownloadRequest::new(
        source::url_candidates(url, DownloadKind::File, input.config),
        &output,
    )
    .with_sha1(sha1);
    if let Some(size) = mapping.size {
        download = download.with_size(size);
    }
    if let Some(context) = input.context {
        download = download.with_task_context(context.clone());
    }
    input.downloader.download(download).await?;
    Ok(())
}

fn validate_mojmaps_target(
    output: &Path,
    expected_path: &str,
    game_directory: &Path,
    output_hashes: &[(PathBuf, String)],
    metadata_sha1: &str,
) -> Result<(), LoaderError> {
    if output != Path::new(expected_path)
        || !output.is_absolute()
        || !output.starts_with(game_directory)
    {
        return Err(LoaderError::Unsupported(format!(
            "DOWNLOAD_MOJMAPS output is not the staged MOJMAPS artifact: {}",
            output.display()
        )));
    }
    if output_hashes
        .iter()
        .any(|(path, expected)| path == output && !expected.eq_ignore_ascii_case(metadata_sha1))
    {
        return Err(LoaderError::Unsupported(
            "installer MOJMAPS output hash disagrees with vanilla metadata".into(),
        ));
    }
    Ok(())
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
struct EnsureArtifact<'a> {
    downloader: &'a DownloadManager,
    config: &'a DownloadConfig,
    library_root: &'a Path,
    path: &'a Path,
    coordinate: &'a str,
    libraries: &'a [ProfileLibrary],
    context: &'a Option<TaskContext>,
}

async fn ensure_artifact(
    z: &mut zip::ZipArchive<fs::File>,
    input: EnsureArtifact<'_>,
) -> Result<(), LoaderError> {
    let EnsureArtifact {
        downloader,
        config,
        library_root,
        path,
        coordinate,
        libraries,
        context,
    } = input;
    let rel = artifact_relative(coordinate)?;
    let embedded = format!("maven/{rel}");
    let root = if coordinate.starts_with("net.neoforged") {
        NEO_MAVEN
    } else {
        FORGE_MAVEN
    };
    let descriptor = profile_artifact_for_coordinate(libraries, coordinate)?;
    let (url, integrity, size) = if let Some(descriptor) = descriptor {
        let descriptor_rel = match descriptor.path.as_deref() {
            Some(path) => safe_zip_entry(path)?,
            None => rel.clone(),
        };
        if descriptor_rel != rel || library_root.join(&descriptor_rel) != path {
            return Err(LoaderError::Unsupported(format!(
                "profile library path does not match processor coordinate {coordinate}"
            )));
        }
        let integrity = profile_artifact_integrity(descriptor)?;
        let url = descriptor
            .url
            .as_deref()
            .filter(|url| !url.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{root}/{rel}"));
        (url, integrity, descriptor.size)
    } else {
        (format!("{root}/{rel}"), None, None)
    };

    if let Some(integrity) = &integrity
        && file_matches_integrity(path, integrity)
    {
        return Ok(());
    }
    if extract_embedded_artifact(z, &embedded, path, integrity.as_ref(), size)? {
        return Ok(());
    }

    let expected = match integrity {
        Some(integrity) => integrity.sha1.clone(),
        None => fetch_sha1(config, &format!("{url}.sha1"), context).await?,
    };
    if file_matches_integrity(
        path,
        &ArtifactIntegrity {
            sha1: expected.clone(),
            size,
        },
    ) {
        return Ok(());
    }
    let mut req = DownloadRequest::new(maven_candidates(&url, DownloadKind::File, config), path)
        .with_sha1(expected);
    if let Some(size) = size {
        req = req.with_size(size);
    }
    if let Some(context) = context.clone() {
        req = req.with_task_context(context)
    }
    downloader.download(req).await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArtifactIntegrity {
    sha1: String,
    size: Option<u64>,
}

fn profile_artifact_for_coordinate<'a>(
    libraries: &'a [ProfileLibrary],
    coord: &str,
) -> Result<Option<&'a ProfileArtifact>, LoaderError> {
    let mut matches = libraries.iter().filter(|library| library.name == coord);
    let Some(library) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(LoaderError::Unsupported(format!(
            "installer contains duplicate profile library coordinate {coord}"
        )));
    }
    Ok(library
        .downloads
        .as_ref()
        .and_then(|downloads| downloads.artifact.as_ref()))
}

fn profile_artifact_integrity(
    descriptor: &ProfileArtifact,
) -> Result<Option<ArtifactIntegrity>, LoaderError> {
    let Some(sha1) = descriptor.sha1.as_deref() else {
        return Ok(None);
    };
    if sha1.len() != 40 || !sha1.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(LoaderError::Unsupported(
            "profile artifact descriptor has an invalid SHA-1".into(),
        ));
    }
    Ok(Some(ArtifactIntegrity {
        sha1: sha1.to_ascii_lowercase(),
        size: descriptor.size,
    }))
}

fn file_matches_integrity(path: &Path, expected: &ArtifactIntegrity) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    metadata.file_type().is_file()
        && expected.size.is_none_or(|size| metadata.len() == size)
        && file_hash(path).as_deref() == Some(expected.sha1.as_str())
}

fn extract_embedded_artifact(
    archive: &mut zip::ZipArchive<fs::File>,
    embedded: &str,
    destination: &Path,
    expected: Option<&ArtifactIntegrity>,
    expected_size: Option<u64>,
) -> Result<bool, LoaderError> {
    let mut source = match archive.by_name(embedded) {
        Ok(source) => source,
        Err(_) => return Ok(false),
    };
    if source.size() > 1024 * 1024 * 1024
        || expected_size
            .or_else(|| expected.and_then(|integrity| integrity.size))
            .is_some_and(|size| source.size() != size)
    {
        return Ok(false);
    }
    let parent = destination.parent().ok_or_else(|| {
        LoaderError::Unsupported("embedded artifact destination has no parent".into())
    })?;
    fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    let file_name = destination
        .file_name()
        .ok_or_else(|| LoaderError::Unsupported("embedded artifact has no file name".into()))?
        .to_string_lossy();
    let temporary = parent.join(format!(
        ".{file_name}.{}.{}.embedded.tmp",
        std::process::id(),
        NEXT_EMBEDDED_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let copy_result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| io(&temporary, e))?;
        std::io::copy(&mut source, &mut file).map_err(|e| io(&temporary, e))?;
        file.sync_all().map_err(|e| io(&temporary, e))?;
        Ok::<_, LoaderError>(())
    })();
    if let Err(error) = copy_result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let temp_matches = if let Some(expected) = expected {
        file_matches_integrity(&temporary, expected)
    } else {
        true
    };
    if !temp_matches {
        let _ = fs::remove_file(&temporary);
        return Ok(false);
    }
    let existing_is_same = fs::symlink_metadata(destination)
        .ok()
        .is_some_and(|metadata| {
            metadata.file_type().is_file()
                && file_hash(destination) == file_hash(&temporary)
                && metadata.len() == fs::metadata(&temporary).map(|m| m.len()).unwrap_or(0)
        });
    if expected.is_none() && existing_is_same {
        let _ = fs::remove_file(&temporary);
        return Ok(true);
    }
    if fs::symlink_metadata(destination).is_ok() {
        fs::remove_file(destination).map_err(|e| io(destination, e))?;
    }
    fs::rename(&temporary, destination).map_err(|e| io(destination, e))?;
    Ok(true)
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
        let integrity = ArtifactIntegrity {
            sha1: expected.clone(),
            size: descriptor.and_then(|artifact| artifact.size),
        };
        if file_matches_integrity(&destination, &integrity) {
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

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InstallerIntegrityRecord {
    schema: u32,
    url: String,
    sha1: String,
    size: u64,
}

fn installer_integrity_record_path(installer: &Path) -> PathBuf {
    let name = installer.file_name().unwrap_or_default().to_string_lossy();
    installer.with_file_name(format!("{name}.sha1.json"))
}

async fn expected_installer_sha1<F, Fut>(
    installer: &Path,
    record_path: &Path,
    url: &str,
    context: &Option<TaskContext>,
    fetch: F,
) -> Result<String, LoaderError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<String, LoaderError>>,
{
    match fetch().await {
        Ok(hash) => validate_sha1(&hash),
        Err(error @ LoaderError::Cancelled) => Err(error),
        Err(fetch_error) => match read_installer_integrity_record(installer, record_path, url) {
            Ok(Some(record)) => {
                check(context)?;
                Ok(record)
            }
            Ok(None) => Err(fetch_error),
            Err(cache_error) => Err(LoaderError::Unsupported(format!(
                "{fetch_error}; cached installer integrity record was rejected: {cache_error}"
            ))),
        },
    }
}

fn validate_sha1(value: &str) -> Result<String, LoaderError> {
    if value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(value.to_ascii_lowercase())
    } else {
        Err(LoaderError::Unsupported(
            "installer checksum is not a valid SHA-1".into(),
        ))
    }
}

fn read_installer_integrity_record(
    installer: &Path,
    record_path: &Path,
    url: &str,
) -> Result<Option<String>, LoaderError> {
    let metadata = match fs::symlink_metadata(record_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io(record_path, error)),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(LoaderError::Unsupported(format!(
            "cached installer integrity record is not a regular file: {}",
            record_path.display()
        )));
    }
    let bytes = fs::read(record_path).map_err(|error| io(record_path, error))?;
    let record: InstallerIntegrityRecord = serde_json::from_slice(&bytes).map_err(|error| {
        LoaderError::Unsupported(format!(
            "invalid cached installer integrity record {}: {error}",
            record_path.display()
        ))
    })?;
    let expected = validate_sha1(&record.sha1)?;
    if record.schema != 1 || record.url != url {
        return Err(LoaderError::Unsupported(
            "cached installer integrity record does not match this installer URL or schema".into(),
        ));
    }
    let installer_metadata =
        fs::symlink_metadata(installer).map_err(|error| io(installer, error))?;
    if !installer_metadata.is_file() || installer_metadata.file_type().is_symlink() {
        return Err(LoaderError::Unsupported(format!(
            "cached installer is not a regular file: {}",
            installer.display()
        )));
    }
    if installer_metadata.len() != record.size
        || file_hash(installer).as_deref() != Some(expected.as_str())
    {
        return Err(LoaderError::Unsupported(format!(
            "cached installer does not match its recorded size and SHA-1: {}",
            installer.display()
        )));
    }
    Ok(Some(expected))
}

fn write_installer_integrity_record(
    installer: &Path,
    record_path: &Path,
    url: &str,
    sha1: &str,
) -> Result<(), LoaderError> {
    let sha1 = validate_sha1(sha1)?;
    let metadata = fs::symlink_metadata(installer).map_err(|error| io(installer, error))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || file_hash(installer).as_deref() != Some(sha1.as_str())
    {
        return Err(LoaderError::Unsupported(format!(
            "installer failed integrity verification before recording its checksum: {}",
            installer.display()
        )));
    }
    let record = InstallerIntegrityRecord {
        schema: 1,
        url: url.to_owned(),
        sha1,
        size: metadata.len(),
    };
    let bytes =
        serde_json::to_vec(&record).map_err(|error| LoaderError::Unsupported(error.to_string()))?;
    atomic_write(record_path, &bytes)
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

    fn integrity_fixture(name: &str) -> (PathBuf, PathBuf, PathBuf, String) {
        let root = std::env::temp_dir().join(format!(
            "ccl-installer-integrity-{name}-{}",
            std::process::id()
        ));
        let installer = root.join("neoforge-installer.jar");
        let record = installer_integrity_record_path(&installer);
        fs::create_dir_all(&root).unwrap();
        fs::write(&installer, b"verified installer fixture").unwrap();
        let hash = file_hash(&installer).unwrap();
        (root, installer, record, hash)
    }

    #[tokio::test]
    async fn verified_installer_record_reuses_cache_when_checksum_metadata_is_unavailable() {
        let (root, installer, record, hash) = integrity_fixture("reuse");
        let url = "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.234/neoforge-21.1.234-installer.jar";
        write_installer_integrity_record(&installer, &record, url, &hash).unwrap();
        let actual = expected_installer_sha1(&installer, &record, url, &None, || async {
            Err(LoaderError::Unsupported(
                "checksum endpoint unavailable".into(),
            ))
        })
        .await
        .unwrap();
        assert_eq!(actual, hash);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn installer_cache_rejects_changed_bytes_and_wrong_artifact_url() {
        let (root, installer, record, hash) = integrity_fixture("reject");
        let url = "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.234/neoforge-21.1.234-installer.jar";
        write_installer_integrity_record(&installer, &record, url, &hash).unwrap();
        fs::write(&installer, b"changed installer").unwrap();
        let changed = expected_installer_sha1(&installer, &record, url, &None, || async {
            Err(LoaderError::Unsupported(
                "checksum endpoint unavailable".into(),
            ))
        })
        .await;
        assert!(changed.is_err());

        fs::write(&installer, b"verified installer fixture").unwrap();
        let wrong_url = expected_installer_sha1(
            &installer,
            &record,
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.233/neoforge-21.1.233-installer.jar",
            &None,
            || async { Err(LoaderError::Unsupported("checksum endpoint unavailable".into())) },
        )
        .await;
        assert!(wrong_url.is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn installer_cache_miss_does_not_accept_unverified_bytes() {
        let (root, installer, record, _) = integrity_fixture("miss");
        let result = expected_installer_sha1(
            &installer,
            &record,
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.234/neoforge-21.1.234-installer.jar",
            &None,
            || async { Err(LoaderError::Unsupported("checksum endpoint unavailable".into())) },
        )
        .await;
        assert!(result.is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn neoforge_release_coordinates_map_to_exact_minecraft_versions() {
        for (version, minecraft) in [
            ("21.0.143", "1.21"),
            ("21.1.234", "1.21.1"),
            ("20.0.27", "1.20"),
            ("26.1.0.0-beta", "26.1"),
            ("26.1.1.0", "26.1.1"),
            ("26.1.1.0+1.21.1", "26.1.1-1.21.1"),
            ("0.24w33a.3", "24w33a"),
        ] {
            assert_eq!(
                neoforge_minecraft_version(version).as_deref(),
                Some(minecraft)
            );
        }
        assert_eq!(neoforge_minecraft_version("21.1"), None);
        assert_eq!(neoforge_minecraft_version("26.1.0"), None);
    }

    #[test]
    fn bmcl_neoforge_list_uses_explicit_exact_minecraft_field() {
        let entries = vec![
            BmclNeoForgeVersion {
                version: "21.0.143".into(),
                mcversion: "1.21".into(),
            },
            BmclNeoForgeVersion {
                version: "21.1.234".into(),
                mcversion: "1.21.1".into(),
            },
            BmclNeoForgeVersion {
                version: "21.1.235-beta".into(),
                mcversion: "1.21.1".into(),
            },
            BmclNeoForgeVersion {
                version: "../unsafe".into(),
                mcversion: "1.21.1".into(),
            },
        ];
        let values = map_bmcl_neoforge_versions(entries, "1.21.1");
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].version, "21.1.234");
        assert!(values[0].stable);
        assert!(!values[1].stable);
    }

    #[test]
    fn bmcl_forge_list_filters_exact_minecraft_and_installer_jar_and_keeps_branch() {
        let installer = || BmclForgeFile {
            format: "jar".into(),
            category: "installer".into(),
        };
        let values = map_bmcl_forge_versions(
            vec![
                BmclForgeVersion {
                    branch: None,
                    mcversion: "1.21.1".into(),
                    version: "52.1.0".into(),
                    files: vec![installer()],
                },
                BmclForgeVersion {
                    branch: Some("custom".into()),
                    mcversion: "1.21.1".into(),
                    version: "52.1.0".into(),
                    files: vec![installer()],
                },
                BmclForgeVersion {
                    branch: None,
                    mcversion: "1.21".into(),
                    version: "53.0.0".into(),
                    files: vec![installer()],
                },
                BmclForgeVersion {
                    branch: None,
                    mcversion: "1.21.1".into(),
                    version: "52.2.0".into(),
                    files: vec![BmclForgeFile {
                        format: "jar".into(),
                        category: "universal".into(),
                    }],
                },
                BmclForgeVersion {
                    branch: None,
                    mcversion: "1.21.1".into(),
                    version: "52.3.0".into(),
                    files: vec![BmclForgeFile {
                        format: "zip".into(),
                        category: "installer".into(),
                    }],
                },
            ],
            "1.21.1",
        );
        assert_eq!(values.len(), 2);
        let mut values = values;
        sort_releases(&mut values);
        assert_eq!(values[0].version, "52.1.0");
        assert!(values[0].stable);
        assert_eq!(values[1].version, "52.1.0-custom");
        assert!(!values[1].stable);
    }

    #[test]
    fn forge_profile_version_validation_accepts_only_the_requested_branch_suffix() {
        let id = "1.21.1-forge-52.1.0";
        assert!(installer_version_matches(
            LoaderKind::Forge,
            "1.21.1",
            "52.1.0-custom",
            id
        ));
        assert!(!installer_version_matches(
            LoaderKind::Forge,
            "1.21.1",
            "52.1.0-custom",
            "1.21.1-forge-52.1.0-other"
        ));
        assert!(!installer_version_matches(
            LoaderKind::Forge,
            "1.21.1",
            "52.1.0",
            "1.20.1-forge-52.1.0"
        ));
    }

    #[test]
    fn neo_mojmaps_download_recognition_is_narrow_and_never_handles_sanitize() {
        let jar = "net.neoforged.installertools:installertools:2.1.2";
        let valid = vec![
            "--task".into(),
            "DOWNLOAD_MOJMAPS".into(),
            "--version".into(),
            "1.21.1".into(),
            "--side".into(),
            "client".into(),
            "--output".into(),
            "/game/libraries/mojmap.txt".into(),
        ];
        assert!(is_client_mojmaps_task(Some(jar), &valid, "1.21.1"));

        let mut sanitize = valid.clone();
        sanitize.push("--sanitize".into());
        assert!(!is_client_mojmaps_task(Some(jar), &sanitize, "1.21.1"));
        let mut unknown = valid.clone();
        unknown.extend(["--format".into(), "TSRG".into()]);
        assert!(!is_client_mojmaps_task(Some(jar), &unknown, "1.21.1"));
        let mut sanitize_equals = valid.clone();
        sanitize_equals.extend(["--sanitize=true".into(), "".into()]);
        assert!(!is_client_mojmaps_task(
            Some(jar),
            &sanitize_equals,
            "1.21.1"
        ));
        assert!(!is_client_mojmaps_task(
            Some("net.neoforged.installertools:jarsplitter:2.1.2"),
            &valid,
            "1.21.1"
        ));
        assert!(!is_client_mojmaps_task(Some(jar), &valid, "1.21"));
        let mut duplicate_task = valid.clone();
        duplicate_task.extend(["--task".into(), "DOWNLOAD_MOJMAPS".into()]);
        assert!(!is_client_mojmaps_task(
            Some(jar),
            &duplicate_task,
            "1.21.1"
        ));
    }

    #[test]
    fn neo_mojmaps_output_requires_staging_target_and_matching_declared_hash() {
        let root = std::env::temp_dir().join("ccl-mojmaps-staging");
        let output = root.join("libraries/mojmap.txt");
        let expected = "0123456789012345678901234567890123456789";
        validate_mojmaps_target(
            &output,
            output.to_str().unwrap(),
            &root,
            &[(output.clone(), expected.into())],
            expected,
        )
        .unwrap();
        assert!(
            validate_mojmaps_target(
                &output,
                output.to_str().unwrap(),
                &root,
                &[(output.clone(), "f".repeat(40))],
                expected,
            )
            .is_err()
        );
        assert!(
            validate_mojmaps_target(
                &root.join("../outside.txt"),
                output.to_str().unwrap(),
                &root,
                &[],
                expected,
            )
            .is_err()
        );
    }

    #[test]
    fn release_catalogue_sorts_numeric_versions_without_relying_on_xml_order() {
        let mut values: Vec<_> = ["52.0.9", "52.1.0", "52.0.48", "52.1.0"]
            .into_iter()
            .map(|version| LoaderVersion {
                version: version.into(),
                stable: true,
            })
            .collect();
        sort_releases(&mut values);
        assert_eq!(
            values
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>(),
            vec!["52.1.0", "52.0.48", "52.0.9"]
        );
    }
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
    fn profile_library_hash_avoids_sidecar_and_rejects_corrupt_cached_artifact() {
        let root = std::env::temp_dir().join(format!(
            "ccl-profile-artifact-integrity-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let coord = "net.neoforged.installertools:installertools:2.1.2";
        let bytes = b"profile verified processor artifact";
        let expected_sha1 = format!("{:x}", Sha1::digest(bytes));
        let libraries = vec![ProfileLibrary {
            name: coord.into(),
            downloads: Some(ProfileLibraryDownloads {
                artifact: Some(ProfileArtifact {
                    path: Some("net/neoforged/installertools/installertools/2.1.2/installertools-2.1.2.jar".into()),
                    url: Some("https://maven.neoforged.net/releases/net/neoforged/installertools/installertools/2.1.2/installertools-2.1.2.jar".into()),
                    sha1: Some(expected_sha1.clone()),
                    size: Some(bytes.len() as u64),
                }),
            }),
        }];
        let descriptor = profile_artifact_for_coordinate(&libraries, coord)
            .unwrap()
            .unwrap();
        let integrity = profile_artifact_integrity(descriptor).unwrap().unwrap();
        assert_eq!(integrity.sha1, expected_sha1);
        assert_eq!(integrity.size, Some(bytes.len() as u64));

        let artifact = root.join("installertools-2.1.2.jar");
        fs::write(&artifact, bytes).unwrap();
        assert!(file_matches_integrity(&artifact, &integrity));
        fs::write(&artifact, b"corrupt cached processor").unwrap();
        assert!(!file_matches_integrity(&artifact, &integrity));

        let no_hash = ProfileArtifact {
            path: None,
            url: None,
            sha1: None,
            size: None,
        };
        assert!(profile_artifact_integrity(&no_hash).unwrap().is_none());
        let _ = fs::remove_dir_all(root);
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
