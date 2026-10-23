//! Minecraft download URL selection.

use crate::config::{DownloadConfig, SourceMode};

/// Selects the independent configuration setting used for a resource category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadKind {
    VersionList,
    File,
}

/// URL alternatives in preference order. Auto tries official URLs before the
/// configured mirror; Mirror tries mapped mirror URLs before official URLs.
/// Official uses only official URLs. Unmapped hosts retain their original URL.
pub fn url_candidates(
    official_url: &str,
    kind: DownloadKind,
    config: &DownloadConfig,
) -> Vec<String> {
    let mode = match kind {
        DownloadKind::VersionList => config.version_list_source,
        DownloadKind::File => config.file_source,
    };
    let mirror = mirror_url(official_url, &config.mirror_root);
    match mode {
        SourceMode::Official => vec![official_url.to_owned()],
        SourceMode::Mirror => match mirror {
            Some(mirror) => vec![mirror, official_url.to_owned()],
            None => vec![official_url.to_owned()],
        },
        SourceMode::Auto => match mirror {
            Some(mirror) => vec![official_url.to_owned(), mirror],
            None => vec![official_url.to_owned()],
        },
    }
}

/// Rewrite known official Minecraft metadata and artifact hosts to BMCLAPI's
/// corresponding paths. Unknown hosts are deliberately left untouched.
pub fn mirror_url(official_url: &str, mirror_root: &str) -> Option<String> {
    let url = reqwest::Url::parse(official_url).ok()?;
    let path = url.path().trim_start_matches('/');
    let mapped = match url.host_str()? {
        "piston-meta.mojang.com"
        | "launchermeta.mojang.com"
        | "piston-data.mojang.com"
        | "launcher.mojang.com" => {
            if url.path().ends_with("/version_manifest_v2.json") {
                "mc/game/version_manifest_v2.json".to_owned()
            } else if url.path().ends_with("/version_manifest.json") {
                "mc/game/version_manifest.json".to_owned()
            } else {
                path.to_owned()
            }
        }
        "libraries.minecraft.net" => format!("libraries/{path}"),
        "resources.download.minecraft.net" => format!("assets/{path}"),
        "meta.fabricmc.net" => format!("fabric-meta/{path}"),
        "maven.fabricmc.net" | "maven.minecraftforge.net" => format!("maven/{path}"),
        "maven.neoforged.net" => {
            format!("maven/{}", path.strip_prefix("releases/")?)
        }
        "files.minecraftforge.net" => {
            format!("maven/{}", path.strip_prefix("maven/")?)
        }
        _ => return None,
    };
    let mut root = reqwest::Url::parse(mirror_root).ok()?;
    let root_path = root.path().trim_end_matches('/');
    root.set_path(&format!("{root_path}/{mapped}"));
    root.set_query(url.query());
    root.set_fragment(None);
    Some(root.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_official_metadata_and_artifact_hosts() {
        let root = "https://mirror.example/base";
        assert_eq!(
            mirror_url("https://piston-meta.mojang.com/v1/manifest.json", root).as_deref(),
            Some("https://mirror.example/base/v1/manifest.json")
        );
        assert_eq!(
            mirror_url("https://libraries.minecraft.net/com/example/a.jar", root).as_deref(),
            Some("https://mirror.example/base/libraries/com/example/a.jar")
        );
        assert_eq!(
            mirror_url("https://resources.download.minecraft.net/ab/cd", root).as_deref(),
            Some("https://mirror.example/base/assets/ab/cd")
        );
        assert!(mirror_url("https://example.org/file", root).is_none());
    }

    #[test]
    fn auto_keeps_official_first_and_uses_the_right_setting() {
        let config = DownloadConfig::default();
        let urls = url_candidates(
            "https://launcher.mojang.com/v1.json",
            DownloadKind::VersionList,
            &config,
        );
        assert_eq!(urls[0], "https://launcher.mojang.com/v1.json");
        assert_eq!(urls.len(), 2);
    }

    #[test]
    fn maps_version_manifest_to_canonical_mirror_path() {
        assert_eq!(
            mirror_url(
                "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json",
                "https://mirror.example"
            )
            .as_deref(),
            Some("https://mirror.example/mc/game/version_manifest_v2.json")
        );
    }

    #[test]
    fn maps_loader_metadata_and_maven_roots_without_duplicate_prefixes() {
        for (url, path) in [
            (
                "https://meta.fabricmc.net/v2/versions/loader/1.21.1",
                "fabric-meta/v2/versions/loader/1.21.1",
            ),
            (
                "https://maven.fabricmc.net/net/fabricmc/a.jar",
                "maven/net/fabricmc/a.jar",
            ),
            (
                "https://maven.minecraftforge.net/net/minecraftforge/a.jar",
                "maven/net/minecraftforge/a.jar",
            ),
            (
                "https://maven.neoforged.net/releases/net/neoforged/a.jar",
                "maven/net/neoforged/a.jar",
            ),
        ] {
            assert_eq!(
                mirror_url(url, "https://mirror.example/base").unwrap(),
                format!("https://mirror.example/base/{path}")
            );
        }
        assert!(
            mirror_url(
                "https://maven.neoforged.net/api/maven",
                "https://mirror.example"
            )
            .is_none()
        );
    }
}
