use super::*;

#[test]
fn repeated_atomic_updates_replace_existing_contents() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("result.json");

    write_bytes_atomically(&path, br#"{"n":1}"#).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"n":1}"#);

    write_bytes_atomically(&path, br#"{"n":2}"#).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"n":2}"#);

    write_bytes_atomically(&path, br#"{"n":3,"ok":true}"#).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"n":3,"ok":true}"#);
}

#[test]
fn unique_temp_paths_differ_for_same_destination() {
    let path = PathBuf::from("/tmp/result.json");
    let left = unique_temp_path(&path);
    let right = unique_temp_path(&path);
    assert_ne!(left, right);
    assert_eq!(left.parent(), path.parent());
    assert!(left
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".result.json.") && name.ends_with(".tmp")));
}

#[cfg(windows)]
#[test]
fn windows_replace_overwrites_existing_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("status.json");
    fs::write(&path, b"old").unwrap();
    write_bytes_atomically(&path, b"new-contents").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "new-contents");
}
