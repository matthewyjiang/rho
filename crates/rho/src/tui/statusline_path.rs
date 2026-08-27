//! Path display fitting for the status-line cwd row.

use std::path::Path;

use super::super::render::{display_width, truncate_keep_end};

pub(in crate::tui) fn compact_cwd(path: &Path) -> String {
    let Some(home) = crate::paths::home_dir() else {
        return path.display().to_string();
    };

    if let Ok(rest) = path.strip_prefix(home) {
        let rel = rest.display().to_string();
        if rel.is_empty() {
            "~".to_string()
        } else {
            // Always use "~/" so home-relative markers stay stable across platforms.
            // On Windows, `rel` may still contain backslashes: "~/work\project".
            format!("~/{rel}")
        }
    } else {
        path.display().to_string()
    }
}

pub(super) fn format_cwd_left(path: &str, branch: Option<&str>) -> String {
    match branch {
        Some(branch) => format!("{path} ({branch})"),
        None => path.to_string(),
    }
}

/// Fit cwd path + optional branch into `width`.
///
/// Basename visibility outranks the branch suffix. Degradation order:
/// 1. full `path (branch)`
/// 2. shortened path + branch, only while the full basename remains
/// 3. drop branch
/// 4. shortened path (may end-truncate a too-long final segment)
pub(super) fn fit_cwd(path: &str, branch: Option<&str>, width: usize) -> String {
    fit_cwd_keeping_basename(path, branch, width)
        .unwrap_or_else(|| shorten_path_display(path, width))
}

/// Fit cwd path, optional branch, and optional extra chrome into `width`.
///
/// Extra is dropped first. After that, [`fit_cwd`] owns the remaining
/// degradation (branch, then path).
pub(super) fn fit_cwd_row(
    path: &str,
    branch: Option<&str>,
    extra: Option<&str>,
    width: usize,
) -> (String, bool) {
    if width == 0 {
        return (String::new(), false);
    }
    if let Some(extra) = extra {
        let extra_width = display_width(extra);
        if extra_width > 0 && extra_width < width {
            let path_budget = width - extra_width;
            if let Some(left) = fit_cwd_keeping_basename(path, branch, path_budget) {
                return (left, true);
            }
        }
    }
    (fit_cwd(path, branch, width), false)
}

/// Full path+branch, or shortened path + branch while the basename stays intact.
/// Returns `None` when that would require dropping the branch or mangling the
/// basename; [`fit_cwd`] then falls back to dropping the branch.
fn fit_cwd_keeping_basename(path: &str, branch: Option<&str>, width: usize) -> Option<String> {
    if width == 0 {
        return None;
    }
    let full = format_cwd_left(path, branch);
    if display_width(&full) <= width {
        return Some(full);
    }
    let Some(branch) = branch else {
        return shorten_path_keeping_basename(path, width);
    };
    let suffix = format!(" ({branch})");
    let suffix_width = display_width(&suffix);
    if suffix_width >= width {
        return None;
    }
    let shortened = shorten_path_keeping_basename(path, width - suffix_width)?;
    Some(format!("{shortened}{suffix}"))
}

fn shorten_path_keeping_basename(path: &str, width: usize) -> Option<String> {
    if display_width(path) <= width {
        return Some(path.to_string());
    }
    let shortened = shorten_path_display(path, width);
    retains_full_basename(path, &shortened).then_some(shortened)
}

fn retains_full_basename(path: &str, shortened: &str) -> bool {
    let base = path_basename(path);
    if base.is_empty() {
        return true;
    }
    if shortened == base {
        return true;
    }
    let Some(prefix) = shortened.strip_suffix(base) else {
        return false;
    };
    prefix.ends_with('/') || prefix.ends_with('\\')
}

fn path_basename(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}

/// Shorten a display path by dropping leading segments.
///
/// Keeps a root marker when it still fits (`~/…/api-gateway`, `/…/api-gateway`),
/// otherwise falls back to `…/api-gateway`, then end-truncates the last segment.
pub(super) fn shorten_path_display(path: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(path) <= width {
        return path.to_string();
    }
    if width <= 1 {
        return truncate_keep_end(path, width);
    }

    let sep = path_display_separator(path);
    let (prefix, rest) = split_path_display_prefix(path, sep);
    let segments: Vec<&str> = rest
        .split(sep)
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() <= 1 {
        return truncate_keep_end(path, width);
    }

    // Prefer the longest trailing-segment form that still fits.
    let mut best: Option<String> = None;
    let sep_str = sep.to_string();
    for keep in 1..segments.len() {
        let tail = segments[segments.len() - keep..].join(&sep_str);
        let candidate = if prefix.is_empty() {
            format!("…{sep}{tail}")
        } else {
            format!("{prefix}…{sep}{tail}")
        };

        if display_width(&candidate) <= width {
            best = Some(candidate);
        } else if best.is_some() {
            // Further candidates only grow.
            break;
        }
    }

    if let Some(candidate) = best {
        return candidate;
    }

    // Even the shortest rooted form failed. Drop the root prefix:
    // `…/last` instead of `~/…/last`.
    let minimal = format!("…{sep}{}", segments[segments.len() - 1]);
    if display_width(&minimal) <= width {
        return minimal;
    }

    // Last segment itself is too long: keep its end so the name stays identifiable.
    truncate_keep_end(&minimal, width)
}

fn path_display_separator(path: &str) -> char {
    // compact_cwd always emits "~/" for home-relative paths, but on Windows the
    // relative portion still uses backslashes: "~/work\project\api".
    if let Some(rest) = path.strip_prefix("~/") {
        if rest.contains('\\') && !rest.contains('/') {
            return '\\';
        }
        return '/';
    }
    if path.contains('/') {
        '/'
    } else if path.contains('\\') {
        '\\'
    } else {
        '/'
    }
}

fn split_path_display_prefix(path: &str, sep: char) -> (&str, &str) {
    if let Some(rest) = path.strip_prefix("~/") {
        return ("~/", rest);
    }
    if let Some(rest) = path.strip_prefix(sep) {
        return (&path[..sep.len_utf8()], rest);
    }
    ("", path)
}

#[cfg(test)]
mod tests {
    use super::{path_display_separator, shorten_path_display};
    use pretty_assertions::assert_eq;

    #[test]
    fn home_relative_windows_paths_use_backslash_segments() {
        assert_eq!(path_display_separator(r"~/work\company\api-gateway"), '\\');
        assert_eq!(
            shorten_path_display(r"~/work\company\services\api-gateway", 18),
            r"~/…\api-gateway"
        );
        assert_eq!(
            shorten_path_display(r"~/work\company\services\api-gateway", 24),
            r"~/…\services\api-gateway"
        );
    }

    #[test]
    fn unix_home_relative_paths_keep_forward_slash() {
        assert_eq!(path_display_separator("~/work/company/api-gateway"), '/');
        assert_eq!(
            shorten_path_display("~/work/company/services/api-gateway", 18),
            "~/…/api-gateway"
        );
    }
}
