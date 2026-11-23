use pretty_assertions::assert_eq;

use super::DaemonTokenSet;

// Covers: daemon HTTP tokens rotate cleanly, invalidate prior secrets
// immediately, use the distinct `rho_daemon_` prefix, and revoke cleanly.
// Owner: daemon_http::tokens.
#[test]
fn rotate_authenticate_and_revoke_daemon_tokens() {
    let mut set = DaemonTokenSet::default();
    let first = set.rotate("laptop").unwrap();
    assert!(first.starts_with("rho_daemon_"));
    assert_eq!(
        set.authenticate(&first).map(|record| record.name.as_str()),
        Some("laptop")
    );
    assert_eq!(set.authenticate("rho_daemon_wrong"), None);

    let rotated = set.rotate("laptop").unwrap();
    assert_ne!(first, rotated);
    assert_eq!(set.authenticate(&first), None);
    assert_eq!(
        set.authenticate(&rotated)
            .map(|record| record.name.as_str()),
        Some("laptop")
    );
    assert_eq!(set.tokens.len(), 1);

    assert!(set.revoke("laptop"));
    assert!(!set.revoke("laptop"));
    assert_eq!(set.authenticate(&rotated), None);
}

// Covers: invalid token names are refused before mutating the token set.
// Owner: daemon_http::tokens.
#[test]
fn invalid_token_names_are_rejected() {
    let mut set = DaemonTokenSet::default();
    for bad in ["", "has space", "bad/slash", &"a".repeat(65)] {
        assert!(set.rotate(bad).is_err(), "{bad:?}");
    }
    assert!(set.tokens.is_empty());
}
