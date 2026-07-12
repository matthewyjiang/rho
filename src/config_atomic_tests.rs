use super::Config;

#[test]
fn saves_config_atomically_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let config = Config {
        model: "round-trip-model".into(),
        ..Config::default()
    };

    config.save(Some(path.clone())).unwrap();
    let loaded = Config::load(Some(path.clone())).unwrap();

    assert_eq!(loaded.model, "round-trip-model");
    assert!(!path.with_extension("toml.tmp").exists());
}
