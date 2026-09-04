//! `@` workspace file path autocomplete.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP};

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

const FILE_ALPHA: &str = "alpha-unique-fixture.txt";
const FILE_BETA: &str = "beta-unique-fixture.txt";

fn setup_file_autocomplete(home: &IsolatedHome) -> Result<()> {
    std::fs::write(home.workspace.join(FILE_ALPHA), "alpha fixture body\n")?;
    std::fs::write(home.workspace.join(FILE_BETA), "beta fixture body\n")?;
    Ok(())
}

// Covers: @ opens path autocomplete, filtering narrows matches, and Enter
// inserts the selected path into the composer.
// Owner: interactive TUI
const FILE_PATH_AUTOCOMPLETE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_file_palette"),
    Step::TypeText("@alpha"),
    Step::WaitText {
        text: FILE_ALPHA,
        timeout: SETTLE,
    },
    // The unfiltered list already contains FILE_ALPHA. Wait until the filter
    // drops beta rather than asserting on the first frame that shows alpha.
    Step::WaitTextGone {
        text: FILE_BETA,
        timeout: SETTLE,
    },
    Step::Custom(assert_file_palette_filtered_to_alpha),
    Step::Phase("select"),
    Step::Custom(select_file_path),
    Step::Custom(assert_file_path_inserted),
    Step::Key(Key::Ctrl('c')),
    Step::ExitCommand,
];

pub(super) const FILE_PATH_AUTOCOMPLETE_SCENARIO: Scenario = Scenario::new(
    "file_path_autocomplete",
    "Open @ path autocomplete and insert a workspace file reference",
    SIZE,
    FILE_PATH_AUTOCOMPLETE_STEPS,
    /* smoke */ false,
)
.with_setup(setup_file_autocomplete);

fn select_file_path(harness: &mut PtyHarness) -> Result<()> {
    harness.settle_input();
    harness.inject_key(&Key::Enter)
}

fn assert_file_palette_filtered_to_alpha(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if !screen.contains(FILE_ALPHA) {
        anyhow::bail!("file palette missing {FILE_ALPHA}:\n{screen}");
    }
    if screen.contains(FILE_BETA) {
        anyhow::bail!("file palette still listed {FILE_BETA} after @alpha filter:\n{screen}");
    }
    Ok(())
}

fn assert_file_path_inserted(harness: &mut PtyHarness) -> Result<()> {
    let inserted = format!("@{FILE_ALPHA}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        harness.poll(Duration::from_millis(30));
        let screen = harness.screen().contents();
        if screen.contains(&inserted) {
            // Palette list rows use a leading marker; require composer insert,
            // not only a still-open suggestion row.
            let on_composer = screen.lines().any(|line| {
                let trimmed = line.trim_start();
                trimmed.starts_with(&format!("> {inserted}")) || trimmed.starts_with(&inserted)
            });
            if on_composer {
                return Ok(());
            }
        }
    }
    anyhow::bail!(
        "composer missing inserted path {inserted}:\n{}",
        harness.screen().contents()
    )
}
