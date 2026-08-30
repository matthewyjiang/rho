use super::LoginPrompt;
use crate::auth::browser::BrowserOpen;

// Covers: constructors keep a URL, prefer url_with_code for copy, Debug redacts codes
// Owner: login prompt
#[test]
fn constructors_copyable_url_and_debug_redaction() {
    let browser = LoginPrompt::browser_flow(
        "https://auth.example/authorize",
        "Open this URL to finish login.",
    );
    pretty_assertions::assert_eq!(
        browser,
        LoginPrompt {
            url: "https://auth.example/authorize".into(),
            user_code: None,
            url_with_code: None,
            browser: BrowserOpen::Skipped,
            instruction: "Open this URL to finish login.".into(),
        }
    );
    pretty_assertions::assert_eq!(browser.copyable_url(), "https://auth.example/authorize");

    let device = LoginPrompt::device_code(
        "https://auth.example/device",
        "WD4E-T6MC",
        Some("https://auth.example/device?user_code=WD4E-T6MC".into()),
        "Visit this URL and enter the code.",
    )
    .with_browser(BrowserOpen::Launched);
    pretty_assertions::assert_eq!(
        device.copyable_url(),
        "https://auth.example/device?user_code=WD4E-T6MC"
    );
    pretty_assertions::assert_eq!(device.url, "https://auth.example/device");
    pretty_assertions::assert_eq!(device.user_code.as_deref(), Some("WD4E-T6MC"));
    pretty_assertions::assert_eq!(device.browser, BrowserOpen::Launched);

    let debug = format!("{device:?}");
    assert!(debug.contains("https://auth.example/device"));
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("WD4E-T6MC"));
}
