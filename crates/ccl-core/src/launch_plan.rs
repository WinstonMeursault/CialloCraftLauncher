//! Pure construction of the Java command line for a Minecraft version.

use crate::{
    config::{MAX_LAUNCH_MEMORY_MB, MIN_LAUNCH_MEMORY_MB},
    metadata::{Argument, Arguments, RuleContext, VersionMetadata, library_allowed},
};
use md5::{Digest, Md5};
use serde::{Serialize, Serializer, ser::SerializeStruct};
use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PlanError {
    #[error("invalid offline username (expected 1 to 16 ASCII letters, digits, or underscores)")]
    InvalidUsername,
    #[error("invalid authenticated identity")]
    InvalidIdentity,
    #[error("invalid version identifier")]
    InvalidVersion,
    #[error(
        "maximum memory must be between {min} and {max} MiB",
        min = MIN_LAUNCH_MEMORY_MB,
        max = MAX_LAUNCH_MEMORY_MB
    )]
    InvalidMemory,
    #[error("version metadata is missing {0}")]
    Missing(&'static str),
    #[error("required path does not exist: {0}")]
    MissingPath(PathBuf),
    #[error("unsafe or invalid library path: {0}")]
    InvalidLibraryPath(String),
    #[error("unsupported placeholder: {0}")]
    UnknownPlaceholder(String),
    #[error("invalid path {0}: {1}")]
    Path(String, #[source] std::io::Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchPlatform {
    pub os: String,
    pub arch: String,
    pub os_version: String,
}
impl LaunchPlatform {
    pub fn current() -> Self {
        let raw = env::consts::OS;
        let os = match raw {
            "macos" => "osx",
            "windows" => "windows",
            "linux" => "linux",
            other => other,
        }
        .to_owned();
        let arch = match env::consts::ARCH {
            "x86_64" => "x86_64",
            "aarch64" => "aarch64",
            "x86" => "x86",
            a => a,
        }
        .to_owned();
        let os_version = current_os_version();
        Self {
            os,
            arch,
            os_version,
        }
    }
}

fn current_os_version() -> String {
    let mut command = if cfg!(target_os = "windows") {
        let mut command = std::process::Command::new("cmd.exe");
        command.args(["/C", "ver"]);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = std::process::Command::new("/usr/bin/sw_vers");
        command.arg("-productVersion");
        command
    } else {
        let mut command = std::process::Command::new("uname");
        command.arg("-r");
        command
    };
    let Ok(output) = command.output() else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    if cfg!(target_os = "windows") {
        regex::Regex::new(r"\d+\.\d+\.\d+(?:\.\d+)?")
            .unwrap()
            .find(&text)
            .map_or_else(String::new, |m| m.as_str().to_owned())
    } else {
        text.trim().to_owned()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfflineAccount {
    pub username: String,
    pub uuid: String,
}
impl OfflineAccount {
    pub fn new(username: &str) -> Result<Self, PlanError> {
        if username.is_empty()
            || username.len() > 16
            || !username
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(PlanError::InvalidUsername);
        }
        let digest = Md5::digest(format!("OfflinePlayer:{username}"));
        let mut bytes: [u8; 16] = digest.into();
        bytes[6] = (bytes[6] & 0x0f) | 0x30;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let uuid = format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        );
        Ok(Self {
            username: username.to_owned(),
            uuid,
        })
    }
}

#[derive(Clone, Debug)]
pub struct PlanOptions {
    pub game_directory: PathBuf,
    pub version: String,
    pub natives_directory: PathBuf,
    pub java_executable: PathBuf,
    pub max_memory_mb: u32,
    pub account: OfflineAccount,
    pub platform: LaunchPlatform,
    pub logging_argument: Option<String>,
}
/// Authenticated profile. Tokens are deliberately excluded from Debug and Serialize.
#[derive(Clone)]
pub struct MicrosoftIdentity {
    pub username: String,
    pub uuid: String,
    pub xuid: String,
    pub client_id: String,
    access_token: String,
}
impl MicrosoftIdentity {
    pub fn new(
        username: String,
        uuid: String,
        xuid: String,
        client_id: String,
        access_token: String,
    ) -> Result<Self, PlanError> {
        OfflineAccount::new(&username)?;
        let uuid = uuid.replace('-', "");
        if uuid.len() != 32
            || !uuid.bytes().all(|b| b.is_ascii_hexdigit())
            || !xuid.bytes().all(|b| b.is_ascii_digit())
            || client_id.is_empty()
            || access_token.is_empty()
            || access_token.chars().any(char::is_control)
        {
            return Err(PlanError::InvalidIdentity);
        }
        Ok(Self {
            username,
            uuid,
            xuid,
            client_id,
            access_token,
        })
    }
}
impl std::fmt::Debug for MicrosoftIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MicrosoftIdentity")
            .field("username", &self.username)
            .field("uuid", &self.uuid)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct LaunchPlan {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    sensitive_values: Vec<String>,
}
impl LaunchPlan {
    pub fn new(executable: PathBuf, arguments: Vec<String>, working_directory: PathBuf) -> Self {
        Self {
            executable,
            arguments,
            working_directory,
            sensitive_values: Vec::new(),
        }
    }
    pub fn redact(&self, text: &str) -> String {
        self.sensitive_values
            .iter()
            .fold(text.to_owned(), |text, secret| {
                text.replace(secret, "<redacted>")
            })
    }
    pub(crate) fn sensitive_values(&self) -> Vec<String> {
        self.sensitive_values.clone()
    }
}
impl Serialize for LaunchPlan {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("LaunchPlan", 3)?;
        state.serialize_field("executable", &self.executable)?;
        state.serialize_field(
            "arguments",
            &self
                .arguments
                .iter()
                .map(|arg| self.redact(arg))
                .collect::<Vec<_>>(),
        )?;
        state.serialize_field("working_directory", &self.working_directory)?;
        state.end()
    }
}
impl std::fmt::Debug for LaunchPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchPlan")
            .field("executable", &self.executable)
            .field(
                "arguments",
                &self
                    .arguments
                    .iter()
                    .map(|arg| self.redact(arg))
                    .collect::<Vec<_>>(),
            )
            .field("working_directory", &self.working_directory)
            .finish()
    }
}

pub fn build_authenticated_launch_plan(
    metadata: &VersionMetadata,
    options: &PlanOptions,
    identity: &MicrosoftIdentity,
) -> Result<LaunchPlan, PlanError> {
    build_plan(metadata, options, Some(identity))
}

pub fn build_launch_plan(
    metadata: &VersionMetadata,
    options: &PlanOptions,
) -> Result<LaunchPlan, PlanError> {
    build_plan(metadata, options, None)
}

fn build_plan(
    metadata: &VersionMetadata,
    options: &PlanOptions,
    identity: Option<&MicrosoftIdentity>,
) -> Result<LaunchPlan, PlanError> {
    if options.version.is_empty()
        || options.version == "."
        || options.version == ".."
        || options.version.contains(['/', '\\', ':'])
        || options.version.chars().any(char::is_control)
    {
        return Err(PlanError::InvalidVersion);
    }
    if !(MIN_LAUNCH_MEMORY_MB..=MAX_LAUNCH_MEMORY_MB).contains(&options.max_memory_mb) {
        return Err(PlanError::InvalidMemory);
    }
    let main = metadata
        .main_class
        .as_deref()
        .filter(|x| !x.is_empty())
        .ok_or(PlanError::Missing("mainClass"))?;
    let game = absolute(&options.game_directory)?;
    let natives = absolute(&options.natives_directory)?;
    let executable = absolute(&options.java_executable)?;
    check_exists(&game)?;
    check_exists(&natives)?;
    check_exists(&executable)?;
    let mut classpath = Vec::new();
    for lib in &metadata.libraries {
        let features = HashMap::new();
        let ctx = RuleContext {
            os: &options.platform.os,
            os_version: &options.platform.os_version,
            arch: &options.platform.arch,
            features: &features,
        };
        if !library_allowed(&lib.rules, &ctx) {
            continue;
        }
        let Some(artifact) = &lib.downloads.artifact else {
            continue;
        };
        let p = resolve_artifact(&game, artifact.path.as_deref(), &lib.name)?;
        check_exists(&p)?;
        classpath.push(path_text(&p));
    }
    metadata
        .client()
        .ok_or(PlanError::Missing("downloads.client"))?;
    let client_version = metadata
        .extra
        .get("jar")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&options.version);
    if client_version.is_empty()
        || client_version == "."
        || client_version == ".."
        || client_version.contains(['/', '\\', ':'])
        || client_version.chars().any(char::is_control)
    {
        return Err(PlanError::InvalidVersion);
    }
    let client_path = game
        .join("versions")
        .join(client_version)
        .join(format!("{client_version}.jar"));
    let client_path = client_path
        .canonicalize()
        .map_err(|e| PlanError::Path(client_path.display().to_string(), e))?;
    check_exists(&client_path)?;
    classpath.push(path_text(&client_path));
    let cp = classpath.join(if options.platform.os == "windows" {
        ";"
    } else {
        ":"
    });
    let mut values = placeholders(metadata, options, &game, &natives, &cp);
    values.insert("primary_jar".into(), path_text(&client_path));
    values.insert(
        "file_separator".into(),
        if options.platform.os == "windows" {
            "\\"
        } else {
            "/"
        }
        .into(),
    );
    if let Some(identity) = identity {
        values.insert("auth_player_name".into(), identity.username.clone());
        values.insert("auth_uuid".into(), identity.uuid.clone());
        values.insert("auth_access_token".into(), identity.access_token.clone());
        values.insert(
            "auth_session".into(),
            format!("token:{}:{}", identity.access_token, identity.uuid),
        );
        values.insert("auth_xuid".into(), identity.xuid.clone());
        values.insert("clientid".into(), identity.client_id.clone());
        values.insert("user_type".into(), "msa".into());
    }
    let mut args = Vec::new();
    if options.platform.os == "osx" {
        args.push("-XstartOnFirstThread".into());
    }
    if let Some(log) = &options.logging_argument {
        args.push(expand(log, &values)?);
    }
    let mut game_args = Vec::new();
    if let Some(Arguments::Structured(a)) = &metadata.arguments {
        let ctx_features = HashMap::new();
        let ctx = RuleContext {
            os: &options.platform.os,
            os_version: &options.platform.os_version,
            arch: &options.platform.arch,
            features: &ctx_features,
        };
        for arg in a.jvm.iter().chain(a.default_user_jvm.iter()) {
            append_argument(arg, &ctx, &values, options.max_memory_mb, &mut args)?;
        }
        if !args.iter().any(|a| a.starts_with("-Djava.library.path=")) {
            args.push(format!(
                "-Djava.library.path={}",
                values["natives_directory"]
            ));
        }
        if !args.iter().any(|a| a == "-cp" || a == "-classpath") {
            args.extend(["-cp".into(), cp.clone()]);
        }
        if !args
            .iter()
            .any(|a| a.starts_with("-Dminecraft.launcher.brand="))
        {
            args.push("-Dminecraft.launcher.brand=CialloCraftLauncher".into());
        }
        if !args
            .iter()
            .any(|a| a.starts_with("-Dminecraft.launcher.version="))
        {
            args.push(format!(
                "-Dminecraft.launcher.version={}",
                env!("CARGO_PKG_VERSION")
            ));
        }
        for arg in &a.game {
            append_argument(arg, &ctx, &values, options.max_memory_mb, &mut game_args)?;
        }
    } else {
        args.extend([
            format!("-Djava.library.path={}", values["natives_directory"]),
            "-Dminecraft.launcher.brand=CialloCraftLauncher".into(),
            format!("-Dminecraft.launcher.version={}", env!("CARGO_PKG_VERSION")),
            "-cp".into(),
            cp,
        ]);
        if let Some(legacy) = &metadata.minecraft_arguments {
            game_args.extend(
                split_legacy(legacy)
                    .iter()
                    .map(|s| expand(s, &values))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        } else if let Some(Arguments::Legacy(s)) = &metadata.arguments {
            game_args.extend(
                split_legacy(s)
                    .iter()
                    .map(|s| expand(s, &values))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        } else {
            game_args.extend([
                "--username".into(),
                options.account.username.clone(),
                "--version".into(),
                options.version.clone(),
                "--gameDir".into(),
                path_text(&game),
                "--assetsDir".into(),
                values["assets_root"].clone(),
                "--uuid".into(),
                options.account.uuid.clone(),
                "--accessToken".into(),
                "0".into(),
                "--userType".into(),
                "legacy".into(),
                "--userProperties".into(),
                "{}".into(),
            ]);
        }
    }
    if main == "cpw.mods.bootstraplauncher.BootstrapLauncher" && client_version != options.version {
        add_bootstrap_client_exclusion(&mut args, &format!("{client_version}.jar"));
    }
    args.push(format!("-Xmx{}M", options.max_memory_mb));
    args.push(main.to_owned());
    args.extend(game_args);
    Ok(LaunchPlan {
        executable,
        arguments: args,
        working_directory: game,
        sensitive_values: identity
            .map(|i| vec![i.access_token.clone()])
            .unwrap_or_default(),
    })
}

fn add_bootstrap_client_exclusion(args: &mut Vec<String>, filename: &str) {
    // BootstrapLauncher builds modules from the legacy classpath. Its profile
    // excludes the primary client by file name; inheritance changes that name.
    if let Some(argument) = args.iter_mut().find(|a| a.starts_with("-DignoreList=")) {
        let mut entries: Vec<_> = argument[13..]
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if !entries.iter().any(|entry| filename.starts_with(entry)) {
            entries.push(filename.into());
        }
        *argument = format!("-DignoreList={}", entries.join(","));
    } else {
        args.push(format!("-DignoreList=asm,securejarhandler,{filename}"));
    }
}

fn placeholders(
    m: &VersionMetadata,
    o: &PlanOptions,
    game: &Path,
    natives: &Path,
    cp: &str,
) -> HashMap<String, String> {
    let root = path_text(game);
    let assets = path_text(&game.join("assets"));
    HashMap::from([
        ("auth_player_name".into(), o.account.username.clone()),
        ("auth_uuid".into(), o.account.uuid.clone()),
        ("auth_access_token".into(), "0".into()),
        ("clientid".into(), String::new()),
        ("auth_xuid".into(), String::new()),
        ("auth_session".into(), "0".into()),
        ("user_type".into(), "legacy".into()),
        ("user_properties".into(), "{}".into()),
        ("version_name".into(), o.version.clone()),
        (
            "version_type".into(),
            m.version_type.clone().unwrap_or_else(|| "release".into()),
        ),
        ("game_directory".into(), root.clone()),
        ("assets_root".into(), assets),
        (
            "game_assets".into(),
            path_text(
                &game
                    .join("assets/virtual")
                    .join(m.assets.as_deref().unwrap_or("legacy")),
            ),
        ),
        (
            "assets_index_name".into(),
            m.asset_index
                .as_ref()
                .and_then(|a| a.id.clone())
                .or_else(|| m.assets.clone())
                .unwrap_or_default(),
        ),
        ("natives_directory".into(), path_text(natives)),
        ("classpath".into(), cp.into()),
        (
            "classpath_separator".into(),
            if o.platform.os == "windows" { ";" } else { ":" }.into(),
        ),
        ("launcher_name".into(), "CialloCraftLauncher".into()),
        ("launcher_version".into(), env!("CARGO_PKG_VERSION").into()),
        (
            "library_directory".into(),
            path_text(&game.join("libraries")),
        ),
        ("resolution_width".into(), "854".into()),
        ("resolution_height".into(), "480".into()),
    ])
}
fn append_argument(
    arg: &Argument,
    ctx: &RuleContext<'_>,
    vals: &HashMap<String, String>,
    max_memory_mb: u32,
    out: &mut Vec<String>,
) -> Result<(), PlanError> {
    let (enabled, items) = match arg {
        Argument::Plain(s) => (true, vec![s.as_str()]),
        Argument::Conditional(c) => (library_allowed(&c.rules, ctx), c.value.values()),
    };
    if enabled {
        for item in items {
            // Memory is an explicit launch option and must remain authoritative.
            if item.starts_with("-Xmx") || xms_exceeds(item, max_memory_mb) {
                continue;
            }
            out.push(expand(item, vals)?);
        }
    }
    Ok(())
}

fn xms_exceeds(argument: &str, max_memory_mb: u32) -> bool {
    let Some(amount) = argument.strip_prefix("-Xms") else {
        return false;
    };
    let split = amount
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(amount.len());
    let (number, suffix) = amount.split_at(split);
    let Ok(value) = number.parse::<f64>() else {
        return false;
    };
    let multiplier = match suffix.to_ascii_lowercase().as_str() {
        "k" | "kb" => 1.0 / 1024.0,
        "m" | "mb" | "" => 1.0,
        "g" | "gb" => 1024.0,
        "t" | "tb" => 1024.0 * 1024.0,
        _ => return false,
    };
    value * multiplier > f64::from(max_memory_mb)
}
fn expand(s: &str, vals: &HashMap<String, String>) -> Result<String, PlanError> {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        rest = &rest[i + 2..];
        let end = rest
            .find('}')
            .ok_or_else(|| PlanError::UnknownPlaceholder(rest.into()))?;
        let key = &rest[..end];
        out.push_str(
            vals.get(key)
                .ok_or_else(|| PlanError::UnknownPlaceholder(key.into()))?,
        );
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}
fn split_legacy(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in s.chars() {
        if escaped {
            token.push(ch);
            escaped = false;
        } else if ch == '\\' && quote.is_some() {
            escaped = true;
        } else if Some(ch) == quote {
            quote = None;
        } else if quote.is_none() && (ch == '\'' || ch == '"') {
            quote = Some(ch);
        } else if quote.is_none() && ch.is_whitespace() {
            if !token.is_empty() {
                result.push(std::mem::take(&mut token));
            }
        } else {
            token.push(ch);
        }
    }
    if escaped {
        token.push('\\');
    }
    if !token.is_empty() {
        result.push(token);
    }
    result
}
pub(crate) fn path_text(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
    }
}
fn absolute(p: &Path) -> Result<PathBuf, PlanError> {
    let p = if p.is_absolute() {
        p.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|e| PlanError::Path(p.display().to_string(), e))?
            .join(p)
    };
    p.canonicalize()
        .map_err(|e| PlanError::Path(p.display().to_string(), e))
}
fn check_exists(p: &Path) -> Result<(), PlanError> {
    if p.exists() {
        Ok(())
    } else {
        Err(PlanError::MissingPath(p.to_path_buf()))
    }
}
fn resolve_artifact(base: &Path, path: Option<&str>, name: &str) -> Result<PathBuf, PlanError> {
    let rel = if let Some(path) = path {
        PathBuf::from(path)
    } else {
        let parts: Vec<&str> = name.split(':').collect();
        if parts.len() < 3 {
            return Err(PlanError::InvalidLibraryPath(name.into()));
        }
        let group = parts[0].replace('.', "/");
        let artifact = parts[1];
        let version = parts[2];
        let classifier = parts.get(3).map(|s| format!("-{s}")).unwrap_or_default();
        PathBuf::from(group)
            .join(artifact)
            .join(version)
            .join(format!("{artifact}-{version}{classifier}.jar"))
    };
    if rel.to_string_lossy().contains(['\\', ':'])
        || rel.is_absolute()
        || rel.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(PlanError::InvalidLibraryPath(rel.display().to_string()));
    }
    let path = base.join("libraries").join(rel);
    path.canonicalize()
        .map_err(|e| PlanError::Path(path.display().to_string(), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_excludes_actual_inherited_client_without_changing_other_arguments() {
        let mut args = vec![
            "-DignoreList=client-extra,neoforge-21.1.234.jar".into(),
            "--version".into(),
            "neoforge-21.1.234".into(),
        ];
        add_bootstrap_client_exclusion(&mut args, "1.21.1.jar");
        add_bootstrap_client_exclusion(&mut args, "1.21.1.jar");
        assert_eq!(
            args[0],
            "-DignoreList=client-extra,neoforge-21.1.234.jar,1.21.1.jar"
        );
        assert_eq!(args[2], "neoforge-21.1.234");
    }
    use crate::metadata::VersionMetadata;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_root() -> PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = env::temp_dir().join(format!("ccl-launch-plan-{n}"));
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn setup() -> (PathBuf, PlanOptions) {
        let root = temp_root();
        let game = root.join("game with spaces");
        fs::create_dir_all(game.join("versions/v1")).unwrap();
        fs::create_dir_all(game.join("libraries/a/b/1")).unwrap();
        fs::create_dir_all(root.join("natives with spaces")).unwrap();
        fs::write(game.join("versions/v1/v1.jar"), b"jar").unwrap();
        fs::write(game.join("libraries/a/b/1/b-1.jar"), b"lib").unwrap();
        let java = root.join("java");
        fs::write(&java, b"java").unwrap();
        let options = PlanOptions {
            game_directory: game,
            version: "v1".into(),
            natives_directory: root.join("natives with spaces"),
            java_executable: java,
            max_memory_mb: 2048,
            account: OfflineAccount::new("Player_1").unwrap(),
            platform: LaunchPlatform {
                os: "linux".into(),
                arch: "x86_64".into(),
                os_version: "6.0".into(),
            },
            logging_argument: None,
        };
        (root, options)
    }
    #[test]
    fn offline_uuid_is_name_based_v3_and_username_is_validated() {
        let a = OfflineAccount::new("Notch").unwrap();
        assert_eq!(a.uuid, "b50ad385-829d-3141-a216-7e7d7539ba7f");
        assert!(OfflineAccount::new("bad name").is_err());
        assert!(OfflineAccount::new("abcdefghijklmnopq").is_err());
    }
    #[test]
    fn expands_modern_offline_fields_and_skips_optional_features() {
        let (root, options) = setup();
        let metadata = VersionMetadata::parse(r#"{"id":"v1","mainClass":"Main","downloads":{"client":{}},"arguments":{"jvm":["-cp","${classpath}"],"game":["--clientId","${clientid}","--xuid","${auth_xuid}","--accessToken","${auth_access_token}",{"rules":[{"action":"allow","features":{"is_demo_user":true}}],"value":"--demo"},{"rules":[{"action":"allow","features":{"has_quick_plays_support":true}}],"value":["--quickPlayPath","${quickPlayPath}"]}]}}"#).unwrap();
        let plan = build_launch_plan(&metadata, &options).unwrap();
        assert!(
            plan.arguments
                .windows(2)
                .any(|args| args == ["--clientId", ""])
        );
        assert!(plan.arguments.windows(2).any(|args| args == ["--xuid", ""]));
        assert!(
            !plan
                .arguments
                .iter()
                .any(|arg| arg.contains("${") || arg == "--demo")
        );
        assert_eq!(plan.arguments.iter().filter(|arg| *arg == "-cp").count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn builds_legacy_and_structured_plans_with_paths_as_single_values() {
        let (root, options) = setup();
        let lib = r#"{"name":"a:b:1","downloads":{"artifact":{"path":"a/b/1/b-1.jar"}}}"#;
        let legacy = VersionMetadata::parse(&format!(r#"{{"id":"v1","mainClass":"example.Main","minecraftArguments":"--username ${{auth_player_name}} --gameDir ${{game_directory}}","downloads":{{"client":{{}}}},"libraries":[{lib}]}}"#)).unwrap();
        let p = build_launch_plan(&legacy, &options).unwrap();
        assert!(p.arguments.iter().any(|a| a == "--gameDir"));
        let directory_index = p
            .arguments
            .iter()
            .position(|arg| arg == "--gameDir")
            .unwrap()
            + 1;
        let directory_argument = &p.arguments[directory_index];
        assert_eq!(
            Path::new(directory_argument).canonicalize().unwrap(),
            options.game_directory.canonicalize().unwrap()
        );
        assert!(!directory_argument.starts_with(r"\\?\"));
        let main = p
            .arguments
            .iter()
            .position(|a| a == "example.Main")
            .unwrap();
        assert_eq!(p.arguments[main + 1], "--username");
        let cp_index = p.arguments.iter().position(|a| a == "-cp").unwrap();
        assert!(p.arguments[cp_index + 1].contains("b-1.jar"));
        let structured = VersionMetadata::parse(&format!(r#"{{"id":"v1","mainClass":"example.Main","arguments":{{"jvm":["-Done=1"],"game":["--username","${{auth_player_name}}"]}},"downloads":{{"client":{{}}}},"libraries":[{lib}]}}"#)).unwrap();
        let p = build_launch_plan(&structured, &options).unwrap();
        let main = p
            .arguments
            .iter()
            .position(|a| a == "example.Main")
            .unwrap();
        assert_eq!(p.arguments[main + 1], "--username");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn windows_classpath_separator_and_unknown_placeholder() {
        let (root, mut options) = setup();
        options.platform.os = "windows".into();
        let m=VersionMetadata::parse(r#"{"id":"v1","mainClass":"Main","arguments":{"game":["${unknown}"]},"downloads":{"client":{}}}"#).unwrap();
        assert!(
            matches!(build_launch_plan(&m,&options), Err(PlanError::UnknownPlaceholder(k)) if k=="unknown")
        );
        let m =
            VersionMetadata::parse(r#"{"id":"v1","mainClass":"Main","downloads":{"client":{}},"libraries":[{"name":"a:b:1","downloads":{"artifact":{"path":"a/b/1/b-1.jar"}}}]}"#)
                .unwrap();
        let p = build_launch_plan(&m, &options).unwrap();
        assert!(p.arguments.iter().any(|a| a == "-cp"));
        let cp = p.arguments.iter().position(|a| a == "-cp").unwrap() + 1;
        assert_eq!(p.arguments[cp].split(';').count(), 2);
        assert!(!p.arguments[cp].contains(r"\\?\"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validates_version_and_memory_and_caps_metadata_heap_settings() {
        let (root, mut options) = setup();
        let metadata = VersionMetadata::parse(
            r#"{"id":"v1","mainClass":"Main","arguments":{"jvm":["-Xms2G","-Xmx8G"]},"downloads":{"client":{}}}"#,
        )
        .unwrap();
        options.max_memory_mb = 1024;
        let plan = build_launch_plan(&metadata, &options).unwrap();
        assert!(
            !plan
                .arguments
                .iter()
                .any(|arg| arg == "-Xms2G" || arg == "-Xmx8G")
        );
        assert!(plan.arguments.iter().any(|arg| arg == "-Xmx1024M"));
        options.version = "../escape".into();
        assert!(matches!(
            build_launch_plan(&metadata, &options),
            Err(PlanError::InvalidVersion)
        ));
        options.version = "v1".into();
        options.max_memory_mb = 128;
        assert!(matches!(
            build_launch_plan(&metadata, &options),
            Err(PlanError::InvalidMemory)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn configured_memory_bounds_match_launch_plan_bounds() {
        let (root, mut options) = setup();
        let metadata =
            VersionMetadata::parse(r#"{"id":"v1","mainClass":"Main","downloads":{"client":{}}}"#)
                .unwrap();

        for memory in [MIN_LAUNCH_MEMORY_MB, MAX_LAUNCH_MEMORY_MB] {
            let mut config = crate::config::LauncherConfig::default();
            config.launch.max_memory_mb = memory;
            config.validate().unwrap();

            options.max_memory_mb = memory;
            let plan = build_launch_plan(&metadata, &options).unwrap();
            assert!(
                plan.arguments
                    .iter()
                    .any(|arg| arg == &format!("-Xmx{memory}M"))
            );
        }

        for memory in [MIN_LAUNCH_MEMORY_MB - 1, MAX_LAUNCH_MEMORY_MB + 1] {
            let mut config = crate::config::LauncherConfig::default();
            config.launch.max_memory_mb = memory;
            assert!(config.validate().is_err());

            options.max_memory_mb = memory;
            assert!(matches!(
                build_launch_plan(&metadata, &options),
                Err(PlanError::InvalidMemory)
            ));
        }
        fs::remove_dir_all(root).unwrap();
    }
}
