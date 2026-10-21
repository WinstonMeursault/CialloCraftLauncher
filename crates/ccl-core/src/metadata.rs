//! Minecraft Java Edition version metadata models and helpers.
//!
//! The types intentionally preserve unknown fields so metadata additions do not
//! make older launcher builds unable to read newer manifests.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("invalid metadata JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("inheritance cycle: {0}")]
    InheritanceCycle(String),
    #[error("missing inherited version: {0}")]
    MissingParent(String),
    #[error("invalid metadata: {0}")]
    Invalid(String),
}

pub type MetadataResult<T> = std::result::Result<T, MetadataError>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VersionManifest {
    #[serde(default)]
    pub latest: LatestVersions,
    #[serde(default)]
    pub versions: Vec<ManifestVersion>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestVersions {
    pub release: Option<String>,
    pub snapshot: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManifestVersion {
    pub id: String,
    #[serde(rename = "type", default)]
    pub version_type: Option<String>,
    pub url: String,
    #[serde(default)]
    pub time: Option<String>,
    #[serde(default, rename = "releaseTime")]
    pub release_time: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default, rename = "complianceLevel")]
    pub compliance_level: Option<u32>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}
impl VersionManifest {
    pub fn parse(json: &str) -> MetadataResult<Self> {
        let manifest: Self = serde_json::from_str(json)?;
        if manifest.versions.is_empty() {
            return Err(MetadataError::Invalid("manifest has no versions".into()));
        }
        if manifest
            .versions
            .iter()
            .any(|v| v.id.trim().is_empty() || v.url.trim().is_empty())
        {
            return Err(MetadataError::Invalid(
                "manifest version is missing id or url".into(),
            ));
        }
        Ok(manifest)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct VersionMetadata {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "inheritsFrom", default)]
    pub inherits_from: Option<String>,
    #[serde(rename = "type", default)]
    pub version_type: Option<String>,
    #[serde(default)]
    pub time: Option<String>,
    #[serde(default)]
    #[serde(rename = "releaseTime")]
    pub release_time: Option<String>,
    #[serde(default, rename = "mainClass")]
    pub main_class: Option<String>,
    #[serde(rename = "minecraftArguments", default)]
    pub minecraft_arguments: Option<String>,
    #[serde(default)]
    pub arguments: Option<Arguments>,
    #[serde(default)]
    pub downloads: HashMap<String, DownloadInfo>,
    #[serde(rename = "assetIndex", default)]
    pub asset_index: Option<AssetIndex>,
    #[serde(default)]
    pub assets: Option<String>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(rename = "javaVersion", default)]
    pub java_version: Option<JavaVersion>,
    #[serde(default)]
    pub logging: Option<Value>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}
impl VersionMetadata {
    pub fn parse(json: &str) -> MetadataResult<Self> {
        let metadata: Self = serde_json::from_str(json)?;
        if metadata.id.as_deref().unwrap_or("").trim().is_empty() {
            return Err(MetadataError::Invalid(
                "version metadata is missing id".into(),
            ));
        }
        if metadata
            .libraries
            .iter()
            .any(|library| library.name.trim().is_empty())
        {
            return Err(MetadataError::Invalid("library is missing name".into()));
        }
        Ok(metadata)
    }
    pub fn client(&self) -> Option<&DownloadInfo> {
        self.downloads.get("client")
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Arguments {
    #[default]
    Empty,
    Legacy(String),
    Structured(StructuredArguments),
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct StructuredArguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
    #[serde(rename = "default-user-jvm", default)]
    pub default_user_jvm: Vec<Argument>,
    #[serde(flatten)]
    pub extra: HashMap<String, Vec<Argument>>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Conditional(ConditionalArgument),
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConditionalArgument {
    #[serde(default)]
    pub rules: Vec<Rule>,
    pub value: ArgumentValue,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ArgumentValue {
    One(String),
    Many(Vec<String>),
}
impl ArgumentValue {
    pub fn values(&self) -> Vec<&str> {
        match self {
            Self::One(s) => vec![s],
            Self::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct DownloadInfo {
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AssetIndex {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(rename = "totalSize", default)]
    pub total_size: Option<u64>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct JavaVersion {
    #[serde(default)]
    pub component: Option<String>,
    #[serde(default, rename = "majorVersion")]
    pub major_version: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Library {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub downloads: LibraryDownloads,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub natives: HashMap<String, String>,
    #[serde(default)]
    pub extract: Option<ExtractRules>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub clientreq: Option<bool>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LibraryDownloads {
    #[serde(default)]
    pub artifact: Option<DownloadInfo>,
    #[serde(default)]
    pub classifiers: HashMap<String, DownloadInfo>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ExtractRules {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Rule {
    pub action: RuleAction,
    #[serde(default)]
    pub os: Option<OsRule>,
    #[serde(default)]
    pub features: HashMap<String, bool>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Disallow,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct OsRule {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default, rename = "versionRange")]
    pub version_range: Option<VersionRange>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct VersionRange {
    #[serde(default)]
    pub min: Option<String>,
    #[serde(default)]
    pub max: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RuleContext<'a> {
    pub os: &'a str,
    pub os_version: &'a str,
    pub arch: &'a str,
    pub features: &'a HashMap<String, bool>,
}
impl Rule {
    pub fn matches(&self, c: &RuleContext<'_>) -> bool {
        let os_ok = self.os.as_ref().is_none_or(|o| {
            o.name.as_ref().is_none_or(|v| v == c.os)
                && o.arch.as_ref().is_none_or(|v| arch_matches(v, c.arch))
                && o.version
                    .as_ref()
                    .is_none_or(|v| regex_like(v, c.os_version))
                && o.version_range
                    .as_ref()
                    .is_none_or(|range| version_in_range(c.os_version, range))
        });
        os_ok
            && self
                .features
                .iter()
                .all(|(k, v)| c.features.get(k).copied().unwrap_or(false) == *v)
    }
}
fn version_in_range(version: &str, range: &VersionRange) -> bool {
    fn parts(v: &str) -> Option<Vec<u64>> {
        v.split('.')
            .map(str::parse)
            .collect::<Result<Vec<_>, _>>()
            .ok()
    }
    let Some(current) = parts(version) else {
        return false;
    };
    let compare = |bound: &str| -> Option<std::cmp::Ordering> {
        let mut left = current.clone();
        let mut right = parts(bound)?;
        let length = left.len().max(right.len());
        left.resize(length, 0);
        right.resize(length, 0);
        Some(left.cmp(&right))
    };
    range
        .min
        .as_ref()
        .is_none_or(|min| compare(min).is_some_and(|o| o != std::cmp::Ordering::Less))
        && range
            .max
            .as_ref()
            .is_none_or(|max| compare(max).is_some_and(|o| o != std::cmp::Ordering::Greater))
}
fn arch_matches(pattern: &str, arch: &str) -> bool {
    pattern == arch
        || (pattern == "x86" && matches!(arch, "x86" | "i386" | "i486" | "i586" | "i686"))
        || (pattern == "x86_64" && matches!(arch, "x86_64" | "amd64"))
        || (pattern == "aarch64" && matches!(arch, "aarch64" | "arm64"))
}
// Mojang metadata uses simple regular expressions for OS versions. This covers
// anchors and wildcards without making regex a mandatory dependency.
fn regex_like(pattern: &str, value: &str) -> bool {
    let p = pattern.trim_start_matches('^').trim_end_matches('$');
    if p == ".*" {
        return true;
    }
    if p.contains(".*") {
        let mut rest = value;
        for part in p.split(".*").filter(|x| !x.is_empty()) {
            if let Some(i) = rest.find(part) {
                rest = &rest[i + part.len()..];
            } else {
                return false;
            }
        }
        return true;
    }
    p == value
}
impl VersionMetadata {
    /// Return the OS-specific native classifier, substituting `${arch}` as Mojang does.
    pub fn native_classifier<'a>(
        &'a self,
        library: &'a Library,
        os: &str,
        arch: &str,
    ) -> Option<(String, &'a DownloadInfo)> {
        if !library.rules.is_empty()
            && !library_allowed(
                &library.rules,
                &RuleContext {
                    os,
                    os_version: "",
                    arch,
                    features: &HashMap::new(),
                },
            )
        {
            return None;
        }
        let template = library.natives.get(os)?;
        let bits = if matches!(arch, "x86" | "i386" | "i486" | "i586" | "i686") {
            "32"
        } else {
            "64"
        };
        let key = template.replace("${arch}", bits);
        library.downloads.classifiers.get(&key).map(|d| (key, d))
    }

    pub fn native_classifier_for<'a>(
        &'a self,
        library: &'a Library,
        context: &RuleContext<'_>,
    ) -> Option<(String, &'a DownloadInfo)> {
        if !library_allowed(&library.rules, context) {
            return None;
        }
        let template = library.natives.get(context.os)?;
        let bits = if matches!(context.arch, "x86" | "i386" | "i486" | "i586" | "i686") {
            "32"
        } else {
            "64"
        };
        let key = template.replace("${arch}", bits);
        library.downloads.classifiers.get(&key).map(|d| (key, d))
    }
}
pub fn library_allowed(rules: &[Rule], context: &RuleContext<'_>) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for r in rules {
        if r.matches(context) {
            allowed = matches!(r.action, RuleAction::Allow);
        }
    }
    allowed
}

/// Resolve a child metadata document and its parents. The map is keyed by version id.
/// Child scalar values win; libraries and argument lists are merged child-first.
pub fn resolve_inheritance(
    id: &str,
    versions: &HashMap<String, VersionMetadata>,
) -> MetadataResult<VersionMetadata> {
    fn visit(
        id: &str,
        all: &HashMap<String, VersionMetadata>,
        stack: &mut Vec<String>,
    ) -> MetadataResult<VersionMetadata> {
        if stack.iter().any(|x| x == id) {
            let mut chain = stack.join(" -> ");
            chain.push_str(" -> ");
            chain.push_str(id);
            return Err(MetadataError::InheritanceCycle(chain));
        }
        let child = all
            .get(id)
            .ok_or_else(|| MetadataError::MissingParent(id.to_owned()))?;
        stack.push(id.to_owned());
        let mut merged = if let Some(parent) = &child.inherits_from {
            visit(parent, all, stack)?
        } else {
            VersionMetadata::default()
        };
        stack.pop();
        merge_child(&mut merged, child);
        Ok(merged)
    }
    visit(id, versions, &mut Vec::new())
}
fn merge_child(base: &mut VersionMetadata, child: &VersionMetadata) {
    macro_rules! scalar { ($($f:ident),*) => {$(if child.$f.is_some(){base.$f=child.$f.clone();})*}; }
    scalar!(
        id,
        inherits_from,
        version_type,
        time,
        release_time,
        main_class,
        minecraft_arguments,
        asset_index,
        assets,
        java_version,
        logging
    );
    if !child.downloads.is_empty() {
        base.downloads.extend(child.downloads.clone());
    }
    let mut libs = child.libraries.clone();
    for inherited in base.libraries.drain(..) {
        if !libs.iter().any(|library| library.name == inherited.name) {
            libs.push(inherited);
        }
    }
    base.libraries = libs;
    if let (Some(Arguments::Structured(dst)), Some(Arguments::Structured(src))) =
        (&mut base.arguments, &child.arguments)
    {
        let mut game = src.game.clone();
        game.append(&mut dst.game);
        dst.game = game;
        let mut jvm = src.jvm.clone();
        jvm.append(&mut dst.jvm);
        dst.jvm = jvm;
        let mut default_user_jvm = src.default_user_jvm.clone();
        default_user_jvm.append(&mut dst.default_user_jvm);
        dst.default_user_jvm = default_user_jvm;
        for (key, values) in &src.extra {
            let mut combined = values.clone();
            combined.append(dst.extra.entry(key.clone()).or_default());
            dst.extra.insert(key.clone(), combined);
        }
    } else if child.arguments.is_some() {
        base.arguments = child.arguments.clone();
    }
    base.extra.extend(child.extra.clone());
}

/// Generic merge utility for callers that need to retain metadata fields unknown to this version.
pub fn merge_json(child: Value, parent: Value) -> Value {
    merge_value(child, parent)
}
fn merge_value(child: Value, parent: Value) -> Value {
    match (child, parent) {
        (Value::Object(mut c), Value::Object(p)) => {
            for (k, v) in p {
                if let Some(cv) = c.remove(&k) {
                    c.insert(k, merge_value(cv, v));
                } else {
                    c.insert(k, v);
                }
            }
            Value::Object(c)
        }
        (Value::Array(mut c), Value::Array(mut p)) => {
            c.append(&mut p);
            Value::Array(c)
        }
        (Value::Null, p) => p,
        (c, _) => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_legacy_and_modern_arguments() {
        let old = VersionMetadata::parse(
            r#"{"id":"old","minecraftArguments":"--username ${auth_player_name}"}"#,
        )
        .unwrap();
        assert!(old.arguments.is_none());
        assert_eq!(
            old.minecraft_arguments.as_deref(),
            Some("--username ${auth_player_name}")
        );
        let new = VersionMetadata::parse(r#"{"id":"new","arguments":{"game":["--demo"],"jvm":["-Xmx2G"],"default-user-jvm":[{"value":["-Xms2G"]}]}}"#)
            .unwrap();
        assert!(matches!(new.arguments, Some(Arguments::Structured(_))));
        let Some(Arguments::Structured(args)) = new.arguments else {
            panic!("expected structured arguments")
        };
        assert_eq!(args.default_user_jvm.len(), 1);
    }
    #[test]
    fn inheritance_detects_cycles_and_child_precedence() {
        let p =
            VersionMetadata::parse(r#"{"id":"p","mainClass":"Parent","libraries":[{"name":"a"}]}"#)
                .unwrap();
        let c = VersionMetadata::parse(
            r#"{"id":"c","inheritsFrom":"p","mainClass":"Child","libraries":[{"name":"b"}]}"#,
        )
        .unwrap();
        let g = VersionMetadata::parse(
            r#"{"id":"g","libraries":[{"name":"shared"},{"name":"grand"}]}"#,
        )
        .unwrap();
        let p = VersionMetadata {
            inherits_from: Some("g".into()),
            libraries: vec![
                Library {
                    name: "a".into(),
                    ..Default::default()
                },
                Library {
                    name: "shared".into(),
                    ..Default::default()
                },
            ],
            ..p
        };
        let all = HashMap::from([("g".into(), g), ("p".into(), p), ("c".into(), c)]);
        let out = resolve_inheritance("c", &all).unwrap();
        assert_eq!(out.main_class.as_deref(), Some("Child"));
        assert_eq!(out.libraries[0].name, "b");
        assert_eq!(out.libraries[1].name, "a");
        assert!(out.libraries.iter().any(|l| l.name == "grand"));
        assert_eq!(
            out.libraries.iter().filter(|l| l.name == "shared").count(),
            1
        );
        let cyc = HashMap::from([
            (
                "x".into(),
                VersionMetadata {
                    id: Some("x".into()),
                    inherits_from: Some("y".into()),
                    ..Default::default()
                },
            ),
            (
                "y".into(),
                VersionMetadata {
                    id: Some("y".into()),
                    inherits_from: Some("x".into()),
                    ..Default::default()
                },
            ),
        ]);
        assert!(matches!(
            resolve_inheritance("x", &cyc),
            Err(MetadataError::InheritanceCycle(_))
        ));
    }

    #[test]
    fn parses_manifest_camel_case_and_rejects_broken_minimums() {
        let manifest = VersionManifest::parse(r#"{"latest":{"release":"1.12.2","snapshot":"x"},"versions":[{"id":"1.12.2","type":"release","url":"https://example.invalid/v.json","releaseTime":"2026-01-01T00:00:00Z","complianceLevel":1}]}"#).unwrap();
        assert_eq!(
            manifest.versions[0].release_time.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
        assert_eq!(manifest.versions[0].compliance_level, Some(1));
        let version = VersionMetadata::parse(r#"{"id":"v","releaseTime":"t","mainClass":"Main","javaVersion":{"component":"java-runtime-gamma","majorVersion":25}}"#).unwrap();
        assert_eq!(version.release_time.as_deref(), Some("t"));
        assert_eq!(version.main_class.as_deref(), Some("Main"));
        assert_eq!(version.java_version.unwrap().major_version, Some(25));
        assert!(VersionMetadata::parse("{}").is_err());
        assert!(VersionManifest::parse(r#"{"versions":[]}"#).is_err());
    }

    #[test]
    fn matches_os_version_range() {
        let rule: Rule = serde_json::from_str(
            r#"{"action":"allow","os":{"name":"windows","versionRange":{"min":"10.0.17134"}}}"#,
        )
        .unwrap();
        let features = HashMap::new();
        assert!(rule.matches(&RuleContext {
            os: "windows",
            os_version: "10.0.19045",
            arch: "x86_64",
            features: &features
        }));
        assert!(!rule.matches(&RuleContext {
            os: "windows",
            os_version: "10.0.15000",
            arch: "x86_64",
            features: &features
        }));
    }
    #[test]
    fn classifier_substitutes_arch() {
        let m = VersionMetadata::default();
        let l = Library {
            natives: HashMap::from([("linux".into(), "natives-linux-${arch}".into())]),
            downloads: LibraryDownloads {
                classifiers: HashMap::from([("natives-linux-64".into(), DownloadInfo::default())]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(m.native_classifier(&l, "linux", "x86_64").is_some());
    }
}
