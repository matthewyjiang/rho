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
