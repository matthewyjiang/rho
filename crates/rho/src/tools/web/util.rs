use std::{sync::LazyLock, time::Duration};

use regex::Regex;
use serde_json::Value;
use url::Url;

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

static SCRIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<script[^>]*>.*?</script>").expect("valid script regex"));
static STYLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<style[^>]*>.*?</style>").expect("valid style regex"));
static BLOCK_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)</?(p|br|div|section|article|h[1-6]|li)[^>]*>").expect("valid block tag regex")
});
static HTML_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<[^>]+>").expect("valid HTML tag regex"));
static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").expect("valid title regex"));
static DOMAIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9.-]*\.[a-z]{2,}$").expect("valid domain regex"));

pub(super) fn to_pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// Shared HTTP client for web tools.
///
/// Redirects are disabled so the resolve-then-check SSRF guard in `ssrf` remains
/// valid for every content fetch. Provider API calls do not need redirects.
pub(super) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .expect("HTTP client configuration must be valid")
}

pub(super) fn fetch_http_client() -> reqwest::Client {
    http_client()
}

pub(super) fn html_to_text(content: &str) -> String {
    let without_scripts = SCRIPT.replace_all(content, "");
    let without_scripts = STYLE.replace_all(&without_scripts, "");
    let with_breaks = BLOCK_TAG.replace_all(&without_scripts, "\n");
    let without_tags = HTML_TAG.replace_all(&with_breaks, "");
    without_tags
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn extract_title(content: &str) -> Option<String> {
    TITLE
        .captures(content)?
        .get(1)
        .map(|capture| html_to_text(capture.as_str()))
}

pub(super) fn is_valid_domain(value: &str) -> bool {
    DOMAIN.is_match(value)
}

pub(super) fn is_youtube_url(target: &str) -> bool {
    Url::parse(target)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .is_some_and(|host| {
            host == "youtu.be" || host.ends_with(".youtube.com") || host == "youtube.com"
        })
}

pub(super) fn is_video_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "mp4" | "mov" | "webm" | "mkv" | "avi" | "m4v"
    )
}
