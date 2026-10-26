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
        ..Preferences::default()
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
fn separate_directories_have_distinct_unified_keys_for_same_id() {
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
    assert_ne!(result.entries[0].key, result.entries[1].key);
    for entry in &result.entries {
        assert!(entry.key.starts_with("instance:"));
        assert_eq!(entry.display_name, "demo");
    }
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

#[test]
fn selected_configuration_persists_separately_from_theme_and_root() {
    let temp = tempfile::tempdir().unwrap();
    let store = ccl_gui::PreferencesStore::new(temp.path().join("gui"));
    let config = temp.path().join("alternate.json");
    std::fs::write(&config, "{}").unwrap();
    store.select_config(config.clone()).unwrap();
    let preferences = ccl_gui::Preferences {
        theme: ccl_gui::Theme::Dark,
        language: "en".into(),
        ..ccl_gui::Preferences::default()
    };
    store.save(preferences.clone()).unwrap();
    assert_eq!(store.config_path().unwrap(), Some(config));
    assert_eq!(store.load().unwrap(), preferences);
    assert!(store.select_config("relative.json".into()).is_err());
}

#[test]
fn appearance_mutations_preserve_instance_selection_and_validate_keys() {
    let temp = tempfile::tempdir().unwrap();
    let store = PreferencesStore::new(temp.path().join("gui"));
    store
        .select_instance(Some("instance:stable-key".into()))
        .unwrap();
    let mut appearance = store.load().unwrap();
    appearance.theme = Theme::Dark;
    appearance.reduce_motion = true;
    store.save_appearance(appearance).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(
        loaded.last_selected_instance.as_deref(),
        Some("instance:stable-key")
    );
    assert_eq!(loaded.theme, Theme::Dark);
    assert!(loaded.reduce_motion);
    assert!(store.select_instance(Some("".into())).is_err());
    assert_eq!(
        store.load().unwrap().last_selected_instance.as_deref(),
        Some("instance:stable-key")
    );
}

#[test]
fn background_import_is_cached_bounded_and_resettable() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.png");
    image::RgbImage::from_pixel(32, 16, image::Rgb([20, 80, 160]))
        .save(&source)
        .unwrap();
    let store = PreferencesStore::new(temp.path().join("gui"));
    let selected = store.import_background(&source).unwrap();
    assert_eq!(selected.background_image.as_deref(), Some("background.jpg"));
    assert!(
        store
            .background_data_url()
            .unwrap()
            .unwrap()
            .starts_with("data:image/jpeg;base64,")
    );
    assert!(store.directory().join("background.jpg").is_file());
    let reset = store.reset_background().unwrap();
    assert_eq!(reset.background_image, None);
    assert_eq!(store.background_data_url().unwrap(), None);
    assert!(!store.directory().join("background.jpg").exists());
}

#[test]
fn background_import_rejects_invalid_images_without_changing_preferences() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("bad.png");
    fs::write(&source, b"not an image").unwrap();
    let store = PreferencesStore::new(temp.path().join("gui"));
    assert!(store.import_background(&source).is_err());
    assert_eq!(store.load().unwrap().background_image, None);
}

#[test]
fn stale_appearance_save_keeps_newer_wallpaper_and_recent_selection() {
    let temp = tempfile::tempdir().unwrap();
    let store = PreferencesStore::new(temp.path().join("gui"));
    let mut stale = store.load().unwrap();
    let source = temp.path().join("large.png");
    image::RgbImage::from_pixel(2200, 1400, image::Rgb([20, 80, 160]))
        .save(&source)
        .unwrap();
    store.import_background(&source).unwrap();
    fs::remove_file(source).unwrap();
    store
        .select_instance(Some("instance:new-selection".into()))
        .unwrap();
    stale.reduce_transparency = true;
    let saved = store.save_appearance(stale).unwrap();
    assert_eq!(saved.background_image.as_deref(), Some("background.jpg"));
    assert_eq!(
        saved.last_selected_instance.as_deref(),
        Some("instance:new-selection")
    );
    assert!(saved.reduce_transparency);
    let cached = image::open(store.directory().join("background.jpg")).unwrap();
    assert!(cached.width() <= 1920 && cached.height() <= 1080);
    assert!(store.background_data_url().unwrap().is_some());
    fs::remove_file(store.directory().join("background.jpg")).unwrap();
    assert_eq!(store.background_data_url().unwrap(), None);
}
