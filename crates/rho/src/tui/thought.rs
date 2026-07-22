use std::time::Duration;

/// Formats a reasoning duration for the post-thinking summary line.
///
/// Short thinks keep tenths of a second. Longer values use the same compact
/// progressive style as other Rho elapsed labels (`2m 5s`, `1h 2m`).
pub(super) fn format_duration(elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs < 60.0 {
        return format!("{secs:.1}s");
    }
    let total = elapsed.as_secs();
    if total < 3_600 {
        format!("{}m {}s", total / 60, total % 60)
    } else {
        format!("{}h {}m", total / 3_600, total % 3_600 / 60)
    }
}

pub(super) fn summary(elapsed: Duration) -> String {
    format!("Thought for {}", format_duration(elapsed))
}

#[cfg(test)]
#[path = "thought_tests.rs"]
mod tests;
