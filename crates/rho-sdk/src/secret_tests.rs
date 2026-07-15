use super::SecretString;

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn formatting_is_always_redacted() {
    let secret = SecretString::new("sk-test-secret-value");

    assert_eq!(format!("{secret:?}"), "SecretString([REDACTED])");
    assert_eq!(secret.to_string(), "[REDACTED]");
    assert!(!format!("{secret:?}").contains(secret.expose_secret()));
    assert!(!secret.to_string().contains(secret.expose_secret()));
}

#[test]
fn requires_explicit_exposure_or_ownership_transfer() {
    let secret = SecretString::new("token");

    assert_eq!(secret.expose_secret(), "token");
    assert_eq!(secret.into_secret(), "token");
}

#[test]
fn is_thread_safe() {
    assert_send_sync::<SecretString>();
}
