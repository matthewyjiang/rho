//! Claude-flavoured rate-limit window labels and `/limits` copy.

use crate::cli_runtime::stream_effect::RateLimitInfo;

/// Human window label (`five_hour` → `5-hour`, `seven_day` → `Weekly`).
pub(crate) fn window_label(info: &RateLimitInfo) -> String {
    crate::claude_runtime::window_kind::WindowKind::from_key(info.window_key()).label()
}

pub(crate) fn describe_rate_limit(info: &RateLimitInfo) -> String {
    let mut parts = vec![window_label(info)];
    if let Some(remaining) = info.remaining_percent() {
        parts.push(format!("{}% left", remaining.round() as u8));
    }
    if let Some(status) = notable_rate_limit_status(info.status.as_deref()) {
        parts.push(status);
    }
    if let Some(resets_at) = info.resets_at {
        parts.push(format!("resets {}", format_unix_local(resets_at)));
    }
    if info.is_using_overage == Some(true) {
        parts.push("using overage".into());
    }
    parts.join(", ")
}

/// Status text worth showing. Plain `allowed` is noise and is omitted.
pub(crate) fn notable_rate_limit_status(status: Option<&str>) -> Option<String> {
    let status = status?.trim();
    if status.is_empty() {
        return None;
    }
    match RateLimitStatusKind::parse(status) {
        RateLimitStatusKind::Allowed => None,
        RateLimitStatusKind::AllowedWarning => Some("warning".into()),
        RateLimitStatusKind::Rejected => Some("limited".into()),
        RateLimitStatusKind::Other(other) => Some(other.replace('_', " ")),
    }
}

/// Wire status values Claude may report on a rate-limit window.
enum RateLimitStatusKind<'a> {
    Allowed,
    AllowedWarning,
    Rejected,
    Other(&'a str),
}

impl<'a> RateLimitStatusKind<'a> {
    fn parse(status: &'a str) -> Self {
        match status {
            "allowed" => Self::Allowed,
            "allowed_warning" => Self::AllowedWarning,
            "rejected" => Self::Rejected,
            other => Self::Other(other),
        }
    }
}

fn format_unix_local(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|value| {
            value
                .with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_else(|| format!("unix {unix}"))
}
