use anyhow::{ensure, Result};

use crate::{harness::PtyHarness, scenario::Step};

use super::{SETTLE, STARTUP};

pub(super) const CHANGELOG_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "rho",
        timeout: STARTUP,
    },
    Step::Phase("open_changelog"),
    Step::SubmitText("/changelog"),
    Step::WaitText {
        text: "changelog",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "notes for this installed version",
        timeout: SETTLE,
    },
    Step::Custom(assert_changelog_header_has_version),
    Step::ExitCommand,
];

// Covers: /changelog must open a versioned release-notes block for this install.
// Owner: interactive UX (PTY)
fn assert_changelog_header_has_version(harness: &mut PtyHarness) -> Result<()> {
    let rows = harness.screen().rows_text();
    let header = rows.iter().find(|row| {
        let trimmed = row.trim_start();
        trimmed.starts_with("changelog")
    });
    let Some(header) = header else {
        ensure!(
            false,
            "changelog header row missing:\n{}",
            harness.screen().debug_dump()
        );
        unreachable!();
    };
    let has_version = header.split_whitespace().any(|token| {
        let token = token.trim_start_matches('v');
        let mut parts = token.split('.');
        matches!(
            (parts.next(), parts.next(), parts.next(), parts.next()),
            (Some(major), Some(minor), Some(patch), None)
                if !major.is_empty()
                    && major.chars().all(|ch| ch.is_ascii_digit())
                    && !minor.is_empty()
                    && minor.chars().all(|ch| ch.is_ascii_digit())
                    && !patch.is_empty()
                    && patch.chars().all(|ch| ch.is_ascii_digit())
        )
    });
    ensure!(
        has_version,
        "changelog header did not include a dotted version:\n{header}\n{}",
        harness.screen().debug_dump()
    );
    Ok(())
}
