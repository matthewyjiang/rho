use tempfile::tempdir;

use super::ConfigRepository;

// Covers: failed save must not report the in-memory update as committed
// Owner: config repository
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
