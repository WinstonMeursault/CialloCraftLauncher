use ccl_gui::{Preferences, PreferencesStore, Theme, bootstrap};
use std::fs;

#[test]
fn preferences_preserve_root_and_never_modify_core_config() {
    let temp = tempfile::tempdir().unwrap();
    let core_config = temp.path().join("config.json");
    let original = br#"{"launch":{"max_memory_mb":4096}}"#;
    fs::write(&core_config, original).unwrap();
    let game = temp.path().join("game");
    fs::create_dir(&game).unwrap();
    let store = PreferencesStore::new(temp.path().join("gui"));
    assert_eq!(store.load().unwrap().theme, Theme::System);
    assert!(!store.directory().exists());
    store.select_root(game.clone()).unwrap();
    let selected = Preferences {
        theme: Theme::Dark,
        language: "en".into(),
    };
    store.save(selected.clone()).unwrap();
    assert_eq!(store.root().unwrap(), Some(game));
    assert_eq!(store.load().unwrap(), selected);
    assert_eq!(fs::read(core_config).unwrap(), original);
}
#[test]
fn unsupported_preferences_are_preserved_without_rewrite() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("gui.json");
    let original = br#"{"schema":99,"preferences":{"theme":"dark","language":"en"},"root":null}"#;
    fs::write(&path, original).unwrap();
    let store = PreferencesStore::new(temp.path().into());
    assert!(store.save(Preferences::default()).is_err());
    assert_eq!(fs::read(path).unwrap(), original);
}
#[test]
fn missing_game_root_is_not_created_by_bootstrap() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("missing-game");
    let store = PreferencesStore::new(temp.path().join("gui"));
    let result = bootstrap(&store, &temp.path().join("config.json"), Some(root.clone())).unwrap();
    assert!(!result.root_exists);
    assert!(result.entries.is_empty());
    assert!(!root.exists());
    assert!(!store.directory().exists());
}
#[test]
fn shared_and_managed_entries_have_distinct_keys_for_same_id() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    let vanilla = root.join("versions/demo");
    fs::create_dir_all(&vanilla).unwrap();
    fs::write(vanilla.join("demo.json"), r#"{"id":"demo","type":"release","mainClass":"example.Main","libraries":[],"javaVersion":{"majorVersion":21}}"#).unwrap();
    let managed = root.join(".ccl/instances/demo");
    let game = managed.join("game");
    fs::create_dir_all(&game).unwrap();
    fs::write(managed.join("instance.json"), serde_json::to_vec(&serde_json::json!({"id":"demo","minecraft":"demo","loader":null,"loader_version":null,"game_directory":fs::canonicalize(&game).unwrap()})).unwrap()).unwrap();
    let result = bootstrap(
        &PreferencesStore::new(temp.path().join("gui")),
        &temp.path().join("config.json"),
        Some(root),
    )
    .unwrap();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.entries[0].key, "vanilla:demo");
    assert_eq!(result.entries[1].key, "managed:demo");
}
#[test]
fn incompatible_core_config_blocks_discovery_without_rewriting() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.json");
    let original = br#"{"future_field":true}"#;
    fs::write(&config, original).unwrap();
    let root = temp.path().join("game");
    let result = bootstrap(
        &PreferencesStore::new(temp.path().join("gui")),
        &config,
        Some(root.clone()),
    )
    .unwrap();
    assert!(!result.warnings.is_empty());
    assert!(result.entries.is_empty());
    assert_eq!(fs::read(config).unwrap(), original);
    assert!(!root.exists());
}

#[test]
fn explicit_initialization_creates_root_and_preserves_launch_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("new-game");
    let config = temp.path().join("config.json");
    fs::write(&config, r#"{"downloads":{"offline":true},"launch":{"max_memory_mb":4096,"auto_install_java":true}}"#).unwrap();
    let before = fs::read(&config).unwrap();
    let store = PreferencesStore::new(temp.path().join("gui"));
    let proposal = bootstrap(&store, &config, Some(root.clone())).unwrap();
    assert!(proposal.setup_required);
    assert!(!root.exists());
    let initialized = ccl_gui::initialize_root(&store, &config, Some(root.clone())).unwrap();
    assert!(initialized.root_exists);
    assert!(!initialized.setup_required);
    assert_eq!(store.root().unwrap(), Some(root));
    assert_eq!(initialized.launch_defaults.max_memory_mb, 4096);
    assert!(initialized.launch_defaults.network_offline);
    assert!(initialized.launch_defaults.auto_install_java);
    assert_eq!(fs::read(&config).unwrap(), before);
}
