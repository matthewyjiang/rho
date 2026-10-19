use super::*;
use pretty_assertions::assert_eq;

// Covers: metadata caching must not hide edits, invalid-file recovery, deletion,
// recreation, or a switch to another configured models.toml path.
// Owner: local model overrides filesystem boundary; no process environment mutation.
#[test]
fn local_override_cache_tracks_edits_deletion_and_path_changes() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.toml");
    let second = dir.path().join("second.toml");
    let document = |name: &str| format!("[models.\"probe/model\"]\ndisplay_name = '{name}'\n");
    let lookup = |path: &Path| {
        local_override_table_at(path, "probe", "model")
            .map(|table| merge_toml_override(ModelMetadata::default(), &table))
    };
    let expected = |name: &str| {
        Some(ModelMetadata {
            display_name: Some(name.to_string()),
            ..ModelMetadata::default()
        })
    };

    assert_eq!(lookup(&first), None);
    for (contents, value) in [
        (document("initial"), expected("initial")),
        (document("edited and longer"), expected("edited and longer")),
        ("[invalid".to_string(), None),
        (document("repaired"), expected("repaired")),
    ] {
        fs::write(&first, &contents).unwrap();
        assert_eq!(lookup(&first), value);
        assert_eq!(lookup(&first), value);
    }
    fs::write(&second, document("other path")).unwrap();
    assert_eq!(lookup(&second), expected("other path"));
    assert_eq!(lookup(&first), expected("repaired"));
    fs::remove_file(&first).unwrap();
    assert_eq!(lookup(&first), None);
    fs::write(&first, document("recreated")).unwrap();
    assert_eq!(lookup(&first), expected("recreated"));
}

// Covers: editors' atomic saves can preserve both length and mtime; the replaced
// file's identity must still invalidate the parsed overrides.
// Owner: Unix override-file identity (rename-over-open behavior is OS-specific).
#[cfg(unix)]
#[test]
fn atomic_override_replacement_invalidates_with_matching_length_and_mtime() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.toml");
    let replacement = dir.path().join("replacement.toml");
    let original = "[models.\"probe/model\"]\nimage_input = true\n";
    let updated = "[models.\"probe/model\"]\nimage_input = false";
    assert_eq!(original.len(), updated.len());
    for (file, contents) in [(&path, original), (&replacement, updated)] {
        fs::write(file, contents).unwrap();
        fs::File::options()
            .write(true)
            .open(file)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
            .unwrap();
    }
    let lookup = || {
        local_override_table_at(&path, "probe", "model")
            .as_ref()
            .and_then(local_image_input)
    };
    assert_eq!(lookup(), Some(ImageInput::Supported));
    fs::rename(replacement, &path).unwrap();
    assert_eq!(lookup(), Some(ImageInput::Unsupported));
}
