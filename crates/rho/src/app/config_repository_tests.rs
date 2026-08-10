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

// Covers: injected save failure is stored on the repository and shared by clones
// Owner: config repository
#[test]
fn injected_save_failure_is_instance_scoped_and_shared_by_clones() {
    let repository = ConfigRepository::temporary_for_tests().unwrap();
    let clone = repository.clone();
    repository.fail_next_save_for_tests();

    let failed = clone
        .update(|config| {
            config.max_output_bytes = 42;
            config.max_output_bytes
        })
        .expect_err("clone must observe the injected save failure");
    assert!(
        failed.to_string().contains("injected config save failure"),
        "{failed}"
    );

    // One-shot: the next save on either handle succeeds.
    let value = repository
        .update(|config| {
            config.max_output_bytes = 7;
            config.max_output_bytes
        })
        .expect("injection is consumed after one save");
    assert_eq!(value, 7);
    assert_eq!(repository.load().unwrap().max_output_bytes, 7);
}
