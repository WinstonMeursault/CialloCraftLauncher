use super::*;

const WINDOWS: &str = "x86_64-pc-windows-msvc";
const MACOS: &str = "aarch64-apple-darwin";
const LINUX: &str = "x86_64-unknown-linux-gnu";
const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA256: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn sample(channel: Channel, target: &str) -> GuiManifest {
    let (format, suffix) = match target {
        MACOS => ("app.tar.gz", ".app.tar.gz"),
        WINDOWS => ("nsis", ".exe"),
        LINUX => ("appimage", ".AppImage"),
        _ => panic!("unknown test target"),
    };
    let version = match channel {
        Channel::Stable => "1.2.3",
        Channel::Dev => "1.2.3-dev.4",
    };
    let branch = match channel {
        Channel::Stable => "main",
        Channel::Dev => "dev",
    };
    let tag = match channel {
        Channel::Stable => format!("v{version}"),
        Channel::Dev => format!("dev-{COMMIT}"),
    };
    let name = format!("ccl-gui-{target}{suffix}");
    GuiManifest {
        schema_version: 1,
        repository: REPOSITORY.to_owned(),
        product: "ccl-gui".to_owned(),
        channel,
        version: version.to_owned(),
        commit: COMMIT.to_owned(),
        branch: branch.to_owned(),
        target: target.to_owned(),
        data_schema_min: 1,
        data_schema_max: 1,
        os_signed: false,
        asset: PackageAsset {
            format: format.to_owned(),
            name: name.clone(),
            size: 1,
            sha256: SHA256.to_owned(),
            url: format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{name}"),
        },
    }
}

fn assert_manifest_rejected(manifest: &GuiManifest, channel: Channel, target: &str, tag: &str) {
    assert!(
        manifest.validate(channel, target, tag).is_err(),
        "unexpectedly accepted {manifest:?}"
    );
}

#[test]
fn accepts_all_platform_formats_on_stable_and_dev_channels() {
    for channel in [Channel::Stable, Channel::Dev] {
        for target in [WINDOWS, MACOS, LINUX] {
            let manifest = sample(channel, target);
            manifest.validate(channel, target, &manifest.tag()).unwrap();
        }
    }
}

#[test]
fn accepts_package_size_and_data_schema_boundaries() {
    let mut manifest = sample(Channel::Stable, WINDOWS);
    manifest.asset.size = MAX_PACKAGE;
    manifest.data_schema_min = 0;
    manifest.data_schema_max = 2;
    manifest
        .validate(Channel::Stable, WINDOWS, &manifest.tag())
        .unwrap();
}

#[test]
fn rejects_wrong_product_repository_channel_and_branch() {
    let tag = sample(Channel::Stable, WINDOWS).tag();

    let mut manifest = sample(Channel::Stable, WINDOWS);
    manifest.product = "ccl-cli".to_owned();
    assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);

    let mut manifest = sample(Channel::Stable, WINDOWS);
    manifest.repository = "attacker/CialloCraftLauncher".to_owned();
    assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);

    let mut manifest = sample(Channel::Stable, WINDOWS);
    manifest.channel = Channel::Dev;
    manifest.branch = "dev".to_owned();
    assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);

    let mut manifest = sample(Channel::Stable, WINDOWS);
    manifest.branch = "dev".to_owned();
    assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);

    let mut manifest = sample(Channel::Stable, WINDOWS);
    manifest.commit = "z".repeat(40);
    assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);

    let manifest = sample(Channel::Stable, WINDOWS);
    assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, "v9.9.9");
}

#[test]
fn rejects_mixed_target_and_asset_format() {
    let tag = sample(Channel::Stable, WINDOWS).tag();
    let mut manifest = sample(Channel::Stable, WINDOWS);
    manifest.target = MACOS.to_owned();
    assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);

    let mut manifest = sample(Channel::Stable, WINDOWS);
    manifest.asset.format = "appimage".to_owned();
    assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);

    let mut manifest = sample(Channel::Stable, WINDOWS);
    manifest.asset.name = format!("ccl-gui-{MACOS}.app.tar.gz");
    assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);
}

#[test]
fn rejects_asset_name_url_host_scheme_and_path_substitution() {
    let tag = sample(Channel::Stable, WINDOWS).tag();
    let invalid_urls = [
        format!(
            "https://github.com.attacker.invalid/{REPOSITORY}/releases/download/{tag}/ccl-gui-{WINDOWS}.exe"
        ),
        format!("http://github.com/{REPOSITORY}/releases/download/{tag}/ccl-gui-{WINDOWS}.exe"),
        format!(
            "https://user@github.com/{REPOSITORY}/releases/download/{tag}/ccl-gui-{WINDOWS}.exe"
        ),
        format!(
            "https://github.com:444/{REPOSITORY}/releases/download/{tag}/ccl-gui-{WINDOWS}.exe"
        ),
        format!(
            "https://github.com/{REPOSITORY}/releases/download/dev-{COMMIT}/ccl-gui-{WINDOWS}.exe"
        ),
        format!("https://github.com/{REPOSITORY}/releases/download/{tag}/other.exe"),
    ];
    for url in invalid_urls {
        let mut manifest = sample(Channel::Stable, WINDOWS);
        manifest.asset.url = url;
        assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);
    }

    let mut manifest = sample(Channel::Stable, WINDOWS);
    manifest.asset.name = "ccl-gui-x86_64-pc-windows-msvc.exe/../other.exe".to_owned();
    assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);
}

#[test]
fn rejects_zero_or_oversized_payload_and_malformed_hash() {
    let tag = sample(Channel::Stable, WINDOWS).tag();
    let mut manifest = sample(Channel::Stable, WINDOWS);
    manifest.asset.size = 0;
    assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);

    let mut manifest = sample(Channel::Stable, WINDOWS);
    manifest.asset.size = MAX_PACKAGE + 1;
    assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);

    let invalid_hashes = [
        String::new(),
        "b".to_owned(),
        "b".repeat(63),
        "b".repeat(65),
        "B".repeat(64),
        "g".repeat(64),
    ];
    for hash in invalid_hashes {
        let mut manifest = sample(Channel::Stable, WINDOWS);
        manifest.asset.sha256 = hash;
        assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);
    }
}

#[test]
fn rejects_invalid_data_schema_ranges_and_manifest_schema() {
    let tag = sample(Channel::Stable, WINDOWS).tag();
    for (minimum, maximum) in [(2, 2), (1, 0), (2, 1)] {
        let mut manifest = sample(Channel::Stable, WINDOWS);
        manifest.data_schema_min = minimum;
        manifest.data_schema_max = maximum;
        assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);
    }
    let mut manifest = sample(Channel::Stable, WINDOWS);
    manifest.schema_version = 2;
    assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);
}

#[test]
fn stable_rejects_prerelease_and_build_versions() {
    for version in ["1.2.3-rc.1", "1.2.3+build.5"] {
        let mut manifest = sample(Channel::Stable, WINDOWS);
        manifest.version = version.to_owned();
        let tag = manifest.tag();
        assert_manifest_rejected(&manifest, Channel::Stable, WINDOWS, &tag);
    }
}

#[test]
fn trusted_url_allows_only_exact_https_github_hosts() {
    for host in [
        "api.github.com",
        "github.com",
        "objects.githubusercontent.com",
        "release-assets.githubusercontent.com",
        "github-releases.githubusercontent.com",
    ] {
        assert!(
            trusted_url(&format!("https://{host}/path")),
            "did not trust {host}"
        );
    }
    for url in [
        "http://github.com/path",
        "ftp://github.com/path",
        "https://user@github.com/path",
        "https://user:password@github.com/path",
        "https://github.com:444/path",
        "https://github.com.attacker.invalid/path",
        "https://sub.github.com/path",
    ] {
        assert!(!trusted_url(url), "unexpectedly trusted {url}");
    }
}
