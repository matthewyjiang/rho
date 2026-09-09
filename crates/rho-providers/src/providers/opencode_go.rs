//! OpenCode Go routing headers, shared by its three wire adapters.

use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use sha2::{Digest, Sha256};

pub(crate) struct SessionHeaders {
    fallback: Option<String>,
}

impl SessionHeaders {
    pub(crate) fn new(provider: &str) -> Self {
        Self {
            // One random 128-bit identity per provider instance for SDK callers
            // without a conversation key. Never rotate it between turns/retries.
            fallback: (provider == "opencode-go")
                .then(|| format!("rho:{:032x}", rand::random::<u128>())),
        }
    }

    pub(crate) fn headers(&self, cache_key: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let Some(fallback) = &self.fallback else {
            return headers;
        };
        let session = cache_key
            .filter(|key| !key.trim().is_empty())
            .unwrap_or(fallback);
        // SDK cache keys need not be valid HTTP header values. Hash those keys
        // rather than dropping their conversation identity or failing a request.
        let value = HeaderValue::from_str(session).unwrap_or_else(|_| {
            HeaderValue::from_str(&format!("rho:{:x}", Sha256::digest(session.as_bytes())))
                .expect("hex digest is a valid header value")
        });
        headers.insert("x-opencode-session", value);
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&crate::rho_user_agent()).expect("Rho user agent is valid"),
        );
        headers
    }
}
