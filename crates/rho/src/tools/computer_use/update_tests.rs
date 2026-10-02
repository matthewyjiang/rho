#![cfg(unix)]

use std::{fs, os::unix::fs::symlink};

use pretty_assertions::assert_eq;

use super::*;

/// Driver whose behavior is the shell `body`. The executable is a symlink to the
/// checked-in `update_fixture.sh`, which sources `body`: executing a freshly
/// written script races concurrent forks that inherit its write fd (ETXTBSY).
fn fixture_driver(dir: &Path, body: &str) -> PathBuf {
    fs::write(dir.join("driver-body.sh"), body).unwrap();
    let path = dir.join("cua-driver");
    symlink(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/tools/computer_use/update_fixture.sh"),
        &path,
    )
    .unwrap();
    path
}

// Covers: Rho trusts the driver's own channel-aware check, forces telemetry off
// for it, distinguishes "unavailable" from "up to date", and never lets a
// malformed or crafted payload become an installable version.
// Owner: update-check process boundary; the driver is a fixture, no network.
#[tokio::test]
async fn check_maps_driver_payload_to_outcome() {
    let up_to_date = UpdateOutcome::UpToDate {
        current: "0.31.0".into(),
    };
    let cases = [
        (
            r#"{"current_version":"0.28.2","latest_version":"0.31.0","update_available":true,"error":null,"release_notes_url":"https://example.invalid/notes","current_channel":"stable"}"#,
            Some(UpdateOutcome::Available {
                current: "0.28.2".into(),
                latest: "0.31.0".into(),
                notes: Some("https://example.invalid/notes".into()),
            }),
        ),
        (
            r#"{"current_version":"0.31.0","latest_version":"0.31.0","update_available":false,"error":null}"#,
            Some(up_to_date),
        ),
        (
            r#"{"current_version":"0.28.2","latest_version":null,"update_available":false,"error":"offline"}"#,
            Some(UpdateOutcome::Unavailable {
                current: "0.28.2".into(),
                reason: "offline".into(),
            }),
        ),
        (
            r#"{"current_version":"0.28.2","latest_version":null,"update_available":true,"error":null}"#,
            Some(UpdateOutcome::Unavailable {
                current: "0.28.2".into(),
                reason: "the driver reported an update without a version".into(),
            }),
        ),
        // Unavailable checks print their payload, then exit 1.
        (
            "{\"current_version\":\"0.28.2\",\"latest_version\":null,\"update_available\":false,\"error\":\"managed by pacman\"}\nJSON\nexit 1\ncat <<'JSON'\n",
            Some(UpdateOutcome::Unavailable {
                current: "0.28.2".into(),
                reason: "managed by pacman".into(),
            }),
        ),
        // Unavailable checks print their payload, then exit 1.
        (
            "{\"current_version\":\"0.28.2\",\"latest_version\":null,\"update_available\":false,\"error\":\"managed by pacman\"}\nJSON\nexit 1\ncat <<'JSON'\n",
            Some(UpdateOutcome::Unavailable {
                current: "0.28.2".into(),
                reason: "managed by pacman".into(),
            }),
        ),
        ("not json", None),
        // A crafted version must never reach the installer environment.
        (
            r#"{"current_version":"1.0.0","latest_version":"1; rm -rf ~","update_available":true,"error":null}"#,
            None,
        ),
        (
            r#"{"current_version":"1.0.0","latest_version":"-x","update_available":true,"error":null}"#,
            None,
        ),
    ];
    for (payload, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        let driver = fixture_driver(
            dir.path(),
            &format!(
                r#"[ "$CUA_DRIVER_RS_TELEMETRY_ENABLED" = false ]
[ "$CUA_TELEMETRY_ENABLED" = false ]
[ "$1" = check-update ] && [ "$2" = --json ]
cat <<'JSON'
{payload}
JSON
"#
            ),
        );
        let result = check_driver(&driver, dir.path()).await;
        assert_eq!(
            result.as_ref().ok(),
            expected.as_ref(),
            "{payload}: {result:?}"
        );
    }
}

// Covers: the launched executable's version gates both a stale check (before
// installing) and an installer that did not take effect (after).
// Owner: session version guard; driver is a fixture.
#[tokio::test]
async fn version_guard_reads_the_launched_executable() {
    let dir = tempfile::tempdir().unwrap();
    let driver = fixture_driver(
        dir.path(),
        "[ \"$1\" = --version ]\necho 'cua-driver v0.30.0'\n",
    );
    let session = ComputerUseSession::new(Some(driver), 1, dir.path().into());
    assert!(session.require_driver_version("0.30.0").await.is_ok());
    assert!(session.require_driver_version("0.31.0").await.is_err());
}

// Covers: a check may not run while an installer replaces the executable, and
// starting an installer aborts and discards an in-flight check.
// Owner: session lifecycle (state ↔ update check); no installer is executed.
#[tokio::test]
async fn checks_are_excluded_while_installing() {
    let dir = tempfile::tempdir().unwrap();
    // Hangs until killed, so only an abort can end it.
    let driver = fixture_driver(dir.path(), "exec /bin/sleep 600\n");
    let session = ComputerUseSession::new(Some(driver), 1, dir.path().into());
    session.start_update_check().unwrap();
    assert_eq!(session.update_check_status(), UpdateCheckStatus::Checking);
    let task = match &*session.update_check() {
        UpdateCheck::Checking(running) => running.task.clone(),
        UpdateCheck::NotChecked | UpdateCheck::Done(_) => unreachable!(),
    };
    *session.state() = State::Installing(super::super::setup::test_installation());
    assert!(session.start_update_check().is_err());
    session.reset_update_check();
    assert_eq!(session.update_check_status(), UpdateCheckStatus::NotChecked);
    // Resolves only because the reset aborted the hung driver process.
    assert!(task.await.is_err());
}
