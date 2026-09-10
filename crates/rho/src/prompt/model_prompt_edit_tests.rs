use super::*;
use pretty_assertions::assert_eq;

fn document(provider: &str, model: &str, body: &str) -> Vec<u8> {
    let fields = serde_yaml_ng::to_string(&serde_json::json!({
        "provider": provider,
        "model": model,
        "mode": "append",
    }))
    .unwrap();
    format!("---\n{fields}---\n{body}").into_bytes()
}

// Covers filesystem-safe names, serialization of exact identities, and non-file collisions.
#[test]
fn new_names_preserve_identity_and_skip_occupied_paths() {
    for (provider, model, expected) in [
        ("provider", "a.b-c_d", "provider_a.b-c_d.md"),
        ("host/v1", "org/model:tag", "host-v1_org-model-tag.md"),
        ("a///::b", "c💥💥d", "a-b_c-d.md"),
        ("a-/-b", "m", "a---b_m.md"),
        ("..", "../m", "_.._..-m.md"),
        ("yes", "1:#'\"", "yes_1-.md"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let edit = ModelPromptEdit::prepare(home.path(), provider, model).unwrap();
        let directory = home.path().join(".rho/model-prompts");
        // The private blank draft must never enter catalog validation.
        assert_eq!(
            model_prompts::load(Some(home.path()), &identity(provider, model)).unwrap(),
            None
        );
        let occupied = directory.join(expected);
        fs::create_dir(&occupied).unwrap();
        let second = directory.join(format!("{}-2.md", expected.trim_end_matches(".md")));
        fs::create_dir(&second).unwrap();
        let expected = directory.join(format!("{}-3.md", expected.trim_end_matches(".md")));
        let bytes = document(provider, model, "prompt\n");
        fs::write(edit.path(), &bytes).unwrap();
        assert_eq!(edit.finish().unwrap(), EditOutcome::Saved(expected.clone()));
        assert_eq!(fs::read(expected).unwrap(), bytes);
        assert!(occupied.is_dir() && second.is_dir());
        assert!(
            model_prompts::load(Some(home.path()), &identity(provider, model))
                .unwrap()
                .is_some()
        );
    }
}

#[test]
fn unchanged_drafts_do_not_publish_or_rewrite_files() {
    for existing in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".rho/model-prompts");
        fs::create_dir_all(&directory).unwrap();
        let destination = directory.join("arbitrary name.md");
        let bytes = b"---\r\nprovider: p\r\nmodel: m\r\n---\r\nexact body\r\n";
        if existing {
            fs::write(&destination, bytes).unwrap();
        }
        let edit = ModelPromptEdit::prepare(home.path(), "p", "m").unwrap();
        let draft = edit.path().to_owned();
        if existing {
            assert_eq!(fs::read(&draft).unwrap(), bytes);
        }
        assert_eq!(edit.finish().unwrap(), EditOutcome::Unchanged);
        assert!(!draft.exists());
        assert_eq!(destination.exists(), existing);
        assert!(!directory.join("p_m.md").exists());
    }
}

// Owner: filesystem transaction. CLI E2E owns spawning the editor, not these commit races.
#[test]
fn concurrent_catalog_changes_reject_commit_and_preserve_draft() {
    enum Change {
        Rewrite,
        Delete,
        Directory,
        Duplicate,
        Malformed,
        NewIdentity,
        Locked,
    }
    for change in [
        Change::Rewrite,
        Change::Delete,
        Change::Directory,
        Change::Duplicate,
        Change::Malformed,
        Change::NewIdentity,
        Change::Locked,
    ] {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".rho/model-prompts");
        fs::create_dir_all(&directory).unwrap();
        let original_path = directory.join("custom.md");
        let original = document("p", "m", "original\n");
        if !matches!(change, Change::NewIdentity) {
            fs::write(&original_path, &original).unwrap();
        }
        let edit = ModelPromptEdit::prepare(home.path(), "p", "m").unwrap();
        let draft = edit.path().to_owned();
        let edited = document("p", "m", "edited\n");
        fs::write(&draft, &edited).unwrap();
        let mut lock = None;
        match change {
            Change::Rewrite => {
                fs::write(&original_path, document("p", "m", "concurrent\n")).unwrap()
            }
            Change::Delete => fs::remove_file(&original_path).unwrap(),
            Change::Directory => {
                fs::remove_file(&original_path).unwrap();
                fs::create_dir(&original_path).unwrap();
            }
            Change::Duplicate => fs::write(directory.join("duplicate.md"), &original).unwrap(),
            Change::Malformed => fs::write(directory.join("unrelated.md"), "invalid").unwrap(),
            Change::NewIdentity => fs::write(&original_path, &original).unwrap(),
            Change::Locked => lock = Some(lock_catalog(&directory).unwrap()),
        }
        let before = fs::read(&original_path).ok();
        let error = edit.finish().unwrap_err();
        // Recovery location is part of the data-loss prevention contract, not UI copy.
        assert!(format!("{error:#}").contains(&draft.display().to_string()));
        assert_eq!(fs::read(&draft).unwrap(), edited);
        assert_eq!(fs::read(&original_path).ok(), before);
        drop(lock);
    }
}

#[test]
fn invalid_drafts_never_replace_live_prompt() {
    for bytes in [
        document("other", "m", "body"),
        document("p", "other", "body"),
        document("p", "m", " \n\t"),
        b"---\nprovider: p\nmodel: m\nmode: invalid\n---\nbody".to_vec(),
        b"---\nprovider: p\nmodel: m\nextra: true\n---\nbody".to_vec(),
        b"not frontmatter".to_vec(),
        vec![0xff],
    ] {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".rho/model-prompts");
        fs::create_dir_all(&directory).unwrap();
        let live = directory.join("custom.md");
        let original = document("p", "m", "original");
        fs::write(&live, &original).unwrap();
        let edit = ModelPromptEdit::prepare(home.path(), "p", "m").unwrap();
        let draft = edit.path().to_owned();
        fs::write(&draft, &bytes).unwrap();
        assert!(edit.finish().is_err());
        assert_eq!(fs::read(live).unwrap(), original);
        assert_eq!(fs::read(draft).unwrap(), bytes);
    }
}

#[cfg(unix)]
#[test]
fn saves_preserve_permissions_and_refuse_readonly_or_symlink_targets() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    for (mode, link) in [(0o640, false), (0o440, false), (0o640, true)] {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".rho/model-prompts");
        fs::create_dir_all(&directory).unwrap();
        let target = home.path().join("target");
        let live = directory.join("arbitrary.md");
        let original = document("p", "m", "original");
        fs::write(if link { &target } else { &live }, &original).unwrap();
        if link {
            symlink(&target, &live).unwrap();
        }
        fs::set_permissions(&live, fs::Permissions::from_mode(mode)).unwrap();
        let prepared = ModelPromptEdit::prepare(home.path(), "p", "m");
        if link || mode == 0o440 {
            assert!(prepared.is_err());
            assert_eq!(fs::read(&live).unwrap(), original);
            assert_eq!(fs::symlink_metadata(&live).unwrap().is_symlink(), link);
        } else {
            let edit = prepared.unwrap();
            let bytes = document("p", "m", "edited");
            fs::write(edit.path(), &bytes).unwrap();
            assert_eq!(edit.finish().unwrap(), EditOutcome::Saved(live.clone()));
            assert_eq!(fs::read(&live).unwrap(), bytes);
            assert_eq!(
                fs::metadata(&live).unwrap().permissions().mode() & 0o777,
                mode
            );
        }
    }
}
