use std::time::Duration;

use tokio::process::Command;

const REWRITE_TIMEOUT: Duration = Duration::from_secs(2);

pub fn is_available() -> bool {
    let Ok(output) = std::process::Command::new("rtk").arg("--version").output() else {
        return false;
    };
    output.status.success() && supports_rewrite(&String::from_utf8_lossy(&output.stdout))
}

fn supports_rewrite(version: &str) -> bool {
    let Some((major, minor, _patch)) = parse_version(version) else {
        return true;
    };
    major > 0 || minor >= 23
}

fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let version = version
        .trim()
        .strip_prefix("rtk ")
        .unwrap_or(version.trim());
    let mut parts = version.split('.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.split_whitespace().next()?.parse().ok()?,
    ))
}

pub async fn rewrite(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty()
        || command.starts_with("rtk ")
        || std::env::var("RTK_DISABLED").ok().as_deref() == Some("1")
    {
        return None;
    }

    let output = tokio::time::timeout(
        REWRITE_TIMEOUT,
        Command::new("rtk").arg("rewrite").arg(command).output(),
    )
    .await
    .ok()?
    .ok()?;

    let code = output.status.code()?;
    if code != 0 && code != 3 {
        return None;
    }

    let rewritten = String::from_utf8(output.stdout).ok()?;
    let rewritten = rewritten.trim();
    (!rewritten.is_empty() && rewritten != command).then(|| rewritten.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rtk_versions() {
        assert_eq!(parse_version("rtk 0.23.0"), Some((0, 23, 0)));
        assert_eq!(parse_version("0.28.2"), Some((0, 28, 2)));
    }

    #[test]
    fn old_rtk_versions_do_not_support_rewrite() {
        assert!(!supports_rewrite("rtk 0.22.9"));
        assert!(supports_rewrite("rtk 0.23.0"));
    }
}
