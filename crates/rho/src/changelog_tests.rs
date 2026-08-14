use pretty_assertions::assert_eq;

use super::{
    bundled_current_display, latest_section, parse_request, section_for_version, ChangelogGroup,
    ChangelogRequest, ChangelogRequestError, ChangelogSection, ChangelogSource, BUNDLED_CHANGELOG,
};

const SAMPLE: &str = r#"# Changelog

## [1.25.1](https://example.test/compare/v1.25.0...v1.25.1) (2026-07-31)

### Bug Fixes

* **tui:** suppress false notice ([#675](https://example.test/issues/675)) ([5a354f0](https://example.test/commit/5a354f0))
* **tui:** validate attachments ([#677](https://example.test/issues/677))

## [1.25.0](https://example.test/compare/v1.24.1...v1.25.0) (2026-07-31)

### Features

* **hooks:** add typed lifecycle hooks ([#668](https://example.test/issues/668))

### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.13.1 to 1.14.0

## [1.24.0] - 2026-07-30

### Features

- plain dash item without links
"#;

// Covers: version lookup must stop at the next release heading and keep only that section.
// Owner: pure unit
#[test]
fn section_for_version_returns_only_the_requested_release() {
    assert_eq!(
        section_for_version(SAMPLE, "1.25.1"),
        Some(ChangelogSection {
            version: "1.25.1".into(),
            date: Some("2026-07-31".into()),
            groups: vec![ChangelogGroup {
                title: "Bug Fixes".into(),
                items: vec![
                    "tui: suppress false notice (#675) (5a354f0)".into(),
                    "tui: validate attachments (#677)".into(),
                ],
            }],
        })
    );
}

// Covers: dependency-only noise must not appear in the user-facing section.
// Owner: pure unit
#[test]
fn section_for_version_skips_dependency_groups() {
    let section = section_for_version(SAMPLE, "1.25.0").expect("section exists");
    assert_eq!(
        section,
        ChangelogSection {
            version: "1.25.0".into(),
            date: Some("2026-07-31".into()),
            groups: vec![ChangelogGroup {
                title: "Features".into(),
                items: vec!["hooks: add typed lifecycle hooks (#668)".into()],
            }],
        }
    );
}

// Covers: document-order "latest" must be the first real release section.
// Owner: pure unit
#[test]
fn latest_section_uses_document_order() {
    let section = latest_section(SAMPLE).expect("section exists");
    assert_eq!(section.version, "1.25.1");
}

// Covers: classic Keep a Changelog date form and plain dash bullets still parse.
// Owner: pure unit
#[test]
fn classic_heading_and_dash_bullets_parse() {
    assert_eq!(
        section_for_version(SAMPLE, "v1.24.0"),
        Some(ChangelogSection {
            version: "1.24.0".into(),
            date: Some("2026-07-30".into()),
            groups: vec![ChangelogGroup {
                title: "Features".into(),
                items: vec!["plain dash item without links".into()],
            }],
        })
    );
}

// Covers: unknown versions and Unreleased must not invent a section.
// Owner: pure unit
#[test]
fn missing_or_unreleased_versions_return_none() {
    assert_eq!(section_for_version(SAMPLE, "9.9.9"), None);
    assert_eq!(
        section_for_version(
            "## [Unreleased]\n\n### Features\n\n* pending\n",
            "Unreleased"
        ),
        None
    );
}

// Covers: /changelog args accept only empty or latest.
// Owner: pure unit
#[test]
fn parse_request_accepts_current_and_latest_only() {
    assert_eq!(parse_request(""), Ok(ChangelogRequest::Current));
    assert_eq!(parse_request("  "), Ok(ChangelogRequest::Current));
    assert_eq!(parse_request("latest"), Ok(ChangelogRequest::Latest));
    assert_eq!(parse_request("LATEST"), Ok(ChangelogRequest::Latest));
    assert_eq!(parse_request("all"), Err(ChangelogRequestError::Usage));
    assert_eq!(
        parse_request("latest please"),
        Err(ChangelogRequestError::Usage)
    );
}

// Covers: the packaged changelog must include a section for this crate version.
// Owner: pure unit
#[test]
fn bundled_changelog_contains_this_package_version() {
    let display = bundled_current_display(env!("CARGO_PKG_VERSION"))
        .expect("bundled changelog should include this package version");
    assert_eq!(display.source, ChangelogSource::Bundled);
    assert_eq!(display.section.version, env!("CARGO_PKG_VERSION"));
    assert!(
        latest_section(BUNDLED_CHANGELOG).is_some_and(|section| !section.groups.is_empty()),
        "bundled changelog should parse at least one user-facing section"
    );
}

// Covers: a dependency-only current version must still resolve, while latest
// skips empty headings and uses the newest notes users can actually read.
// Owner: pure unit
#[test]
fn dependency_only_version_resolves_and_is_skipped_as_latest() {
    let changelog = r#"# Changelog

## [1.40.1](https://example.test/compare/v1.40.0...v1.40.1) (2026-08-14)

### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 1.3.0 to 1.3.1

## [1.40.0](https://example.test/compare/v1.39.1...v1.40.0) (2026-08-14)

### Bug Fixes

* compile openai-compatible hosts
"#;
    assert_eq!(
        section_for_version(changelog, "1.40.1"),
        Some(ChangelogSection {
            version: "1.40.1".into(),
            date: Some("2026-08-14".into()),
            groups: vec![],
        })
    );
    assert_eq!(
        latest_section(changelog),
        Some(ChangelogSection {
            version: "1.40.0".into(),
            date: Some("2026-08-14".into()),
            groups: vec![ChangelogGroup {
                title: "Bug Fixes".into(),
                items: vec!["compile openai-compatible hosts".into()],
            }],
        })
    );
}
