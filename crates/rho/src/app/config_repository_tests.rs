use tempfile::tempdir;

use super::ConfigRepository;

#[test]
fn update_loads_mutates_and_persists_config() {
    let directory = tempdir().unwrap();
    let repository = ConfigRepository::new(Some(directory.path().join("config.toml")));

    let updated = repository
        .update(|config| {
            config.max_output_bytes = 42;
            config.max_output_bytes
        })
        .unwrap();

    assert_eq!(updated, 42);
    assert_eq!(repository.load().unwrap().max_output_bytes, 42);
}

#[test]
fn failed_save_does_not_return_the_update_value() {
    let directory = tempdir().unwrap();
    let repository = ConfigRepository::new(Some(directory.path().to_path_buf()));

    let result = repository.update(|config| {
        config.max_output_bytes = 42;
        config.max_output_bytes
    });

    assert!(result.is_err());
}
