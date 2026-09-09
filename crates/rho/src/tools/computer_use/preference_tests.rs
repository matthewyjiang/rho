use pretty_assertions::assert_eq;
use rho_sdk::SessionId;

use super::{preference_path, session_path, ComputerUsePreference};

// Covers: defaults are inherited once, and session changes never authorize another session.
// Owner: machine-local consent storage; lifecycle routing belongs to the caller.
#[test]
fn sessions_keep_their_choice_when_the_default_changes() {
    use ComputerUsePreference::{Disabled, Enabled};
    for (initial, changed) in [(Enabled, Disabled), (Disabled, Enabled)] {
        let root = tempfile::tempdir().unwrap();
        let default_path = preference_path(root.path()).unwrap();
        let first = SessionId::new();
        let second = SessionId::new();
        let legacy = SessionId::new();
        initial.save_to(&default_path).unwrap();
        assert_eq!(
            ComputerUsePreference::initialize_session_in(root.path(), &first).unwrap(),
            initial
        );
        changed.save_to(&default_path).unwrap();
        assert_eq!(
            [
                ComputerUsePreference::load_from(&session_path(root.path(), &first).unwrap())
                    .unwrap(),
                ComputerUsePreference::initialize_session_in(root.path(), &second).unwrap(),
                ComputerUsePreference::load_from(&session_path(root.path(), &legacy).unwrap())
                    .unwrap(),
            ],
            [initial, changed, Disabled]
        );
        changed
            .save_to(&session_path(root.path(), &first).unwrap())
            .unwrap();
        assert_eq!(
            ComputerUsePreference::load_from(&session_path(root.path(), &first).unwrap()).unwrap(),
            changed
        );
        initial
            .save_to(&session_path(root.path(), &second).unwrap())
            .unwrap();
        assert_eq!(
            [
                ComputerUsePreference::load_from(&session_path(root.path(), &first).unwrap())
                    .unwrap(),
                ComputerUsePreference::load_from(&default_path).unwrap(),
            ],
            [changed, changed]
        );
    }
}

// Covers: malformed stored records cannot fall back to an enabled default.
// Owner: session consent filesystem boundary.
#[test]
fn initialization_does_not_replace_invalid_session_records() {
    let root = tempfile::tempdir().unwrap();
    ComputerUsePreference::Enabled
        .save_to(&preference_path(root.path()).unwrap())
        .unwrap();
    let session = SessionId::new();
    let path = session_path(root.path(), &session).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for contents in ["", "enabled = 'yes'", "enabled = true\nextra = true"] {
        std::fs::write(&path, contents).unwrap();
        assert!(ComputerUsePreference::initialize_session_in(root.path(), &session).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    }
}

// Covers: relative homes and host-supplied IDs cannot redirect consent into project files.
// Owner: preference storage path validation.
#[test]
fn preference_paths_reject_untrusted_components() {
    let root = tempfile::tempdir().unwrap();
    assert!(preference_path(std::path::Path::new("relative-home")).is_err());
    assert!(session_path(std::path::Path::new("relative-home"), &SessionId::new()).is_err());
    for id in ["../project", "/tmp/project", "not-a-uuid", "..\\project"] {
        assert!(session_path(root.path(), &SessionId::from_string(id).unwrap()).is_err());
    }
}

// Covers: only an explicit, valid saved consent may enable startup access.
// Owner: machine-local preference filesystem boundary, independent of the TUI.
#[test]
fn preference_requires_explicit_consent_and_persists_revocation() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("computer-use.toml");
    assert_eq!(
        ComputerUsePreference::load_from(&path).unwrap(),
        ComputerUsePreference::Disabled
    );
    for preference in [
        ComputerUsePreference::Enabled,
        ComputerUsePreference::Disabled,
    ] {
        preference.save_to(&path).unwrap();
        assert_eq!(ComputerUsePreference::load_from(&path).unwrap(), preference);
    }
    for contents in [
        "",
        "enabled = 'yes'",
        "enabled = true\nextra = true",
        "invalid",
    ] {
        std::fs::write(&path, contents).unwrap();
        assert!(ComputerUsePreference::load_from(&path).is_err());
    }
    let directory = root.path().join("directory");
    std::fs::create_dir(&directory).unwrap();
    assert!(ComputerUsePreference::load_from(&directory).is_err());
    assert!(ComputerUsePreference::Disabled.save_to(&directory).is_err());
}

// Covers: saved desktop consent must not follow a link into project content.
// Owner: preference file security; writer permission mechanics have their own tests.
#[cfg(unix)]
#[test]
fn preference_rejects_symlink_consent() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("project.toml");
    let path = root.path().join("computer-use.toml");
    std::fs::write(&target, "enabled = true").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(ComputerUsePreference::load_from(&path).is_err());
    ComputerUsePreference::Disabled.save_to(&path).unwrap();
    assert_eq!(
        ComputerUsePreference::load_from(&path).unwrap(),
        ComputerUsePreference::Disabled
    );
    assert_eq!(std::fs::read_to_string(target).unwrap(), "enabled = true");

    let session = SessionId::new();
    let session_file = session_path(root.path(), &session).unwrap();
    std::fs::create_dir_all(session_file.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&path, &session_file).unwrap();
    assert!(ComputerUsePreference::initialize_session_in(root.path(), &session).is_err());

    let linked_home = root.path().join("linked-home");
    std::fs::create_dir(&linked_home).unwrap();
    std::os::unix::fs::symlink(root.path(), linked_home.join("computer-use-sessions")).unwrap();
    assert!(session_path(&linked_home, &session).is_err());
}
