fn main() {
    for name in ["CCL_BUILD_COMMIT", "CCL_BUILD_CHANNEL", "CCL_BUILD_TARGET"] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let target = std::env::var("TARGET").expect("Cargo TARGET");
    if let Ok(expected) = std::env::var("CCL_BUILD_TARGET") {
        assert_eq!(
            expected, target,
            "release build target does not match Cargo target"
        );
    }
    if let Ok(channel) = std::env::var("CCL_BUILD_CHANNEL") {
        assert!(
            matches!(channel.as_str(), "stable" | "dev"),
            "unknown release channel"
        );
    }
    if let Ok(commit) = std::env::var("CCL_BUILD_COMMIT") {
        assert!(
            commit.len() == 40
                && commit
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "release commit must be a full lowercase SHA"
        );
    }
    println!("cargo:rustc-env=CCL_TARGET={target}");
}
