use pretty_assertions::assert_eq;

use super::SessionKind;

#[test]
fn remote_markers_win_over_wsl() {
    let session = SessionKind::detect_from(
        |name| matches!(name, "SSH_CONNECTION" | "WSL_DISTRO_NAME"),
        || true,
    );
    assert_eq!(session, SessionKind::Remote);
}

#[test]
fn wsl_is_detected_without_remote_markers() {
    let session = SessionKind::detect_from(|_| false, || true);
    assert_eq!(session, SessionKind::Wsl);
}

#[test]
fn local_is_the_default() {
    let session = SessionKind::detect_from(|_| false, || false);
    assert_eq!(session, SessionKind::Local);
}

#[test]
fn mosh_counts_as_remote() {
    let session = SessionKind::detect_from(|name| name == "MOSH_IP", || false);
    assert_eq!(session, SessionKind::Remote);
}
