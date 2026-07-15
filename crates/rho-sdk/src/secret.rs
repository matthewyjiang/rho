use std::{fmt, ops::Deref};

/// An explicitly handled secret value with redacted formatting.
///
/// `Debug` and `Display` never reveal the contained value. Call
/// [`SecretString::expose_secret`] only at the narrow transport boundary that
/// needs the credential, for example while adding an authorization header.
/// `SecretString` deliberately does not implement `Serialize`, so credentials
/// cannot enter SDK snapshots or events through an accidental derive.
///
/// ```compile_fail
/// use rho_sdk::SecretString;
///
/// let secret = SecretString::new("provider-token");
/// let _ = serde_json::to_string(&secret);
/// ```
///
/// ```
/// use rho_sdk::SecretString;
///
/// let credential = SecretString::new("provider-token");
/// assert_eq!(format!("{credential:?}"), "SecretString([REDACTED])");
/// assert_eq!(credential.expose_secret(), "provider-token");
/// ```
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a secret without inspecting or normalizing it.
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    /// Exposes the secret to an explicit credential consumer.
    ///
    /// Avoid formatting, logging, including in errors, or retaining the
    /// returned value outside the provider transport operation that needs it.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    /// Consumes the wrapper and returns the secret.
    ///
    /// This is intended for transferring ownership into another redacting
    /// credential type. Prefer [`SecretString::expose_secret`] for request
    /// headers so fewer unwrapped copies exist.
    pub fn into_secret(self) -> String {
        self.0
    }

    /// Returns whether the secret has no bytes.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl From<String> for SecretString {
    fn from(secret: String) -> Self {
        Self::new(secret)
    }
}

impl From<&str> for SecretString {
    fn from(secret: &str) -> Self {
        Self::new(secret)
    }
}

impl Deref for SecretString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.expose_secret()
    }
}

#[cfg(test)]
#[path = "secret_tests.rs"]
mod tests;
