use pretty_assertions::assert_eq;

use super::SessionKind;

// Covers: remote markers (SSH or Mosh) win over WSL, and WSL wins over local.
// Owner: clipboard session policy (pure unit).
#[test]
fn detects_session_kind_by_precedence() {
    for (case, env, is_wsl, expected) in [
        (
            "remote wins over wsl",
            &["SSH_CONNECTION", "WSL_DISTRO_NAME"][..],
            true,
            SessionKind::Remote,
        ),
        (
            "wsl without remote markers",
            &[][..],
            true,
            SessionKind::Wsl,
        ),
        (
            "mosh counts as remote",
            &["MOSH_IP"][..],
            false,
            SessionKind::Remote,
        ),
        ("local otherwise", &[][..], false, SessionKind::Local),
    ] {
        let session =
            SessionKind::detect_from(|name| env.iter().any(|marker| *marker == name), || is_wsl);
        assert_eq!(session, expected, "{case}");
    }
}
