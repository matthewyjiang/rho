//! Shared authorize-URL payload for every interactive login.

use super::browser::BrowserOpen;

/// What the user needs to finish an interactive login.
///
/// `url` is always present. Presenters must show it even when a browser
/// launched successfully. [`Self::browser`] is [`BrowserOpen::Skipped`] until
/// the dispatch edge calls [`Self::with_browser`].
#[derive(Clone, PartialEq, Eq)]
pub struct LoginPrompt {
    pub url: String,
    pub user_code: Option<String>,
    pub url_with_code: Option<String>,
    pub browser: BrowserOpen,
    pub instruction: String,
}

impl LoginPrompt {
    /// Browser or URL-only flow (no device code).
    pub fn browser_flow(url: impl Into<String>, instruction: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            user_code: None,
            url_with_code: None,
            browser: BrowserOpen::Skipped,
            instruction: instruction.into(),
        }
    }

    /// Device-code flow. `url` is the verification URI.
    pub fn device_code(
        verification_uri: impl Into<String>,
        user_code: impl Into<String>,
        url_with_code: Option<String>,
        instruction: impl Into<String>,
    ) -> Self {
        Self {
            url: verification_uri.into(),
            user_code: Some(user_code.into()),
            url_with_code,
            browser: BrowserOpen::Skipped,
            instruction: instruction.into(),
        }
    }

    /// Record whether a browser launch was attempted after the URL is known.
    pub fn with_browser(mut self, browser: BrowserOpen) -> Self {
        self.browser = browser;
        self
    }

    /// Prefer the complete URL when the provider supplied one.
    pub fn copyable_url(&self) -> &str {
        self.url_with_code.as_deref().unwrap_or(&self.url)
    }
}

impl std::fmt::Debug for LoginPrompt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoginPrompt")
            .field("url", &self.url)
            .field("user_code", &self.user_code.as_ref().map(|_| "[REDACTED]"))
            .field(
                "url_with_code",
                &self.url_with_code.as_ref().map(|_| "[REDACTED]"),
            )
            .field("browser", &self.browser)
            .field("instruction", &self.instruction)
            .finish()
    }
}

#[cfg(test)]
#[path = "login_prompt_tests.rs"]
mod tests;
