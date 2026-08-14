//! Application changelog lookup for the `/changelog` command.
//!
//! Parses Keep a Changelog sections from the bundled `CHANGELOG.md`, and can
//! fetch the same file from the latest published release tag when requested.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, USER_AGENT};

use crate::update::{latest_release_tag, release_tag_to_version};

/// Bundled app changelog shipped with this build.
pub const BUNDLED_CHANGELOG: &str = include_str!("../CHANGELOG.md");

const RAW_CHANGELOG_URL_PREFIX: &str = "https://raw.githubusercontent.com/matthewyjiang/rho/";
const RAW_CHANGELOG_URL_SUFFIX: &str = "/crates/rho/CHANGELOG.md";
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);

/// Which release section `/changelog` should show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangelogRequest {
    /// Section for the running binary version (bundled file).
    Current,
    /// Newest section from the latest published release (network).
    Latest,
}

/// One Keep a Changelog release section after display cleanup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangelogSection {
    pub version: String,
    pub date: Option<String>,
    pub groups: Vec<ChangelogGroup>,
}

/// A titled bullet group inside a release section (`Features`, `Bug Fixes`, …).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangelogGroup {
    pub title: String,
    pub items: Vec<String>,
}

/// Where the displayed section came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangelogSource {
    Bundled,
    LatestRelease,
}

/// User-facing payload for the TUI command block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangelogDisplay {
    pub section: ChangelogSection,
    pub source: ChangelogSource,
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChangelogRequestError {
    #[error("usage: /changelog [latest]")]
    Usage,
}

/// Parse `/changelog` args into a request.
pub fn parse_request(args: &str) -> Result<ChangelogRequest, ChangelogRequestError> {
    match args.trim() {
        "" => Ok(ChangelogRequest::Current),
        token if token.eq_ignore_ascii_case("latest") => Ok(ChangelogRequest::Latest),
        _ => Err(ChangelogRequestError::Usage),
    }
}

/// Resolve the running version from the bundled changelog.
pub fn bundled_current_display(current_version: &str) -> Result<ChangelogDisplay, String> {
    let section = section_for_version(BUNDLED_CHANGELOG, current_version).ok_or_else(|| {
        format!("no changelog section for v{current_version} in the bundled release notes")
    })?;
    let note = section
        .groups
        .is_empty()
        .then(|| "this version only updated workspace dependencies".to_string());
    Ok(ChangelogDisplay {
        section,
        source: ChangelogSource::Bundled,
        note,
    })
}

/// Fetch and parse the newest published app changelog section.
pub async fn fetch_latest_display() -> anyhow::Result<ChangelogDisplay> {
    let tag = latest_release_tag().await?;
    let version = release_tag_to_version(&tag)
        .ok_or_else(|| anyhow::anyhow!("latest release tag '{tag}' does not contain a version"))?;
    let body = fetch_remote_changelog(&tag).await?;
    let section = section_for_latest_tag(&body, &version).ok_or_else(|| {
        anyhow::anyhow!("latest release changelog did not contain a readable section")
    })?;
    Ok(ChangelogDisplay {
        section,
        source: ChangelogSource::LatestRelease,
        note: Some(format!("latest published release ({tag})")),
    })
}

/// Extract the section whose version heading matches `version`.
pub fn section_for_version(changelog: &str, version: &str) -> Option<ChangelogSection> {
    let version = normalize_version(version);
    parse_sections(changelog)
        .into_iter()
        .find(|section| section.version == version)
}

/// Extract the first (newest) release section that still has user-facing notes.
pub fn latest_section(changelog: &str) -> Option<ChangelogSection> {
    parse_sections(changelog)
        .into_iter()
        .find(|section| !section.groups.is_empty())
}

/// Select the section `/changelog latest` shows for a published tag.
///
/// Exact-version lookup wins only when that heading still has user-facing
/// groups. A dependency-only latest tag falls through to the newest
/// non-empty section in the same file.
pub fn section_for_latest_tag(changelog: &str, tagged_version: &str) -> Option<ChangelogSection> {
    section_for_version(changelog, tagged_version)
        .filter(|section| !section.groups.is_empty())
        .or_else(|| latest_section(changelog))
}

fn fetch_remote_changelog(tag: &str) -> impl std::future::Future<Output = anyhow::Result<String>> {
    let tag = tag.to_string();
    async move {
        if !is_safe_git_ref(&tag) {
            anyhow::bail!("latest release tag '{tag}' is not a plain git ref");
        }
        let url = format!("{RAW_CHANGELOG_URL_PREFIX}{tag}{RAW_CHANGELOG_URL_SUFFIX}");
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("rho-coding-agent"));
        headers.insert(ACCEPT, HeaderValue::from_static("text/plain"));
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(FETCH_TIMEOUT)
            .build()?;
        let body = client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(body)
    }
}

fn parse_sections(changelog: &str) -> Vec<ChangelogSection> {
    let mut sections = Vec::new();
    let mut lines = changelog.lines().peekable();

    while let Some(line) = lines.next() {
        let Some((version, date)) = parse_version_heading(line) else {
            continue;
        };

        let mut groups: Vec<ChangelogGroup> = Vec::new();
        let mut current_title: Option<String> = None;
        let mut current_items: Vec<String> = Vec::new();

        while let Some(next) = lines.peek() {
            if parse_version_heading(next).is_some() {
                break;
            }
            let line = lines.next().expect("peeked line exists");
            if let Some(title) = parse_group_heading(line) {
                flush_group(&mut groups, current_title.take(), &mut current_items);
                current_title = Some(title);
                continue;
            }
            if let Some(item) = parse_item_line(line) {
                // Items before any group heading are rare; drop them.
                if current_title.is_some() {
                    current_items.push(item);
                }
            }
        }
        flush_group(&mut groups, current_title.take(), &mut current_items);

        // Drop dependency-only noise, but keep the heading so a dep-only
        // current version still resolves for `/changelog`.
        groups.retain(|group| !is_skipped_group(&group.title));
        sections.push(ChangelogSection {
            version,
            date,
            groups,
        });
    }

    sections
}

fn flush_group(groups: &mut Vec<ChangelogGroup>, title: Option<String>, items: &mut Vec<String>) {
    let Some(title) = title else {
        items.clear();
        return;
    };
    if items.is_empty() {
        return;
    }
    groups.push(ChangelogGroup {
        title,
        items: std::mem::take(items),
    });
}

fn parse_version_heading(line: &str) -> Option<(String, Option<String>)> {
    let rest = line.trim().strip_prefix("##")?;
    let rest = rest.trim();
    if rest.is_empty() || rest.starts_with('#') {
        return None;
    }

    // ## [1.25.1](url) (2026-07-31)
    // ## [1.25.1] - 2026-07-31
    // ## [1.25.1]
    let (version_token, after_version) = if let Some(rest) = rest.strip_prefix('[') {
        let end = rest.find(']')?;
        let version = rest[..end].trim();
        let after = rest[end + 1..].trim_start();
        (version, after)
    } else {
        let mut parts = rest.splitn(2, char::is_whitespace);
        let version = parts.next()?.trim();
        (version, parts.next().unwrap_or("").trim_start())
    };

    if version_token.is_empty() || version_token.eq_ignore_ascii_case("unreleased") {
        return None;
    }
    let version = normalize_version(version_token);
    if version.is_empty() {
        return None;
    }

    let after_link = strip_markdown_link_target(after_version);
    let date = parse_heading_date(after_link);
    Some((version, date))
}

fn strip_markdown_link_target(text: &str) -> &str {
    let text = text.trim_start();
    if let Some(rest) = text.strip_prefix('(') {
        if let Some(end) = rest.find(')') {
            return rest[end + 1..].trim_start();
        }
    }
    text
}

fn parse_heading_date(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let text = text.strip_prefix('-').map(str::trim).unwrap_or(text);
    let text = text
        .strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')'))
        .unwrap_or(text)
        .trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn parse_group_heading(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("###")?;
    let title = rest.trim();
    if title.is_empty() || title.starts_with('#') {
        None
    } else {
        Some(title.to_string())
    }
}

fn parse_item_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let body = trimmed
        .strip_prefix("* ")
        .or_else(|| trimmed.strip_prefix("- "))
        .or_else(|| trimmed.strip_prefix("+ "))?;
    let cleaned = clean_markup(body);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn is_skipped_group(title: &str) -> bool {
    title.eq_ignore_ascii_case("Dependencies")
}

fn clean_markup(input: &str) -> String {
    let without_links = strip_markdown_links(input);
    let without_bold = without_links.replace("**", "");
    collapse_whitespace(&without_bold)
}

fn strip_markdown_links(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('[') {
        output.push_str(&rest[..start]);
        let after_bracket = &rest[start + 1..];
        let Some(label_end) = after_bracket.find(']') else {
            output.push('[');
            rest = after_bracket;
            continue;
        };
        let label = &after_bracket[..label_end];
        let after_label = &after_bracket[label_end + 1..];
        if let Some(after_paren) = after_label.strip_prefix('(') {
            if let Some(url_end) = after_paren.find(')') {
                output.push_str(label);
                rest = &after_paren[url_end + 1..];
                continue;
            }
        }
        output.push('[');
        rest = after_bracket;
    }
    output.push_str(rest);
    output
}

fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_version(version: &str) -> String {
    version.trim().trim_start_matches('v').trim().to_string()
}

fn is_safe_git_ref(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'/'))
}

#[cfg(test)]
#[path = "changelog_tests.rs"]
mod tests;
