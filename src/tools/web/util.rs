use std::time::Duration;

use regex::Regex;
use serde_json::Value;
use url::Url;

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn to_pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

pub(super) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

pub(super) fn safe_path_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn html_to_text(content: &str) -> String {
    let without_scripts = Regex::new(r"(?is)<script[^>]*>.*?</script>")
        .unwrap()
        .replace_all(content, "");
    let without_scripts = Regex::new(r"(?is)<style[^>]*>.*?</style>")
        .unwrap()
        .replace_all(&without_scripts, "");
    let with_breaks = Regex::new(r"(?i)</?(p|br|div|section|article|h[1-6]|li)[^>]*>")
        .unwrap()
        .replace_all(&without_scripts, "\n");
    let without_tags = Regex::new(r"(?s)<[^>]+>")
        .unwrap()
        .replace_all(&with_breaks, "");
    without_tags
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn extract_title(content: &str) -> Option<String> {
    Regex::new(r"(?is)<title[^>]*>(.*?)</title>")
        .ok()?
        .captures(content)?
        .get(1)
        .map(|capture| html_to_text(capture.as_str()))
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
