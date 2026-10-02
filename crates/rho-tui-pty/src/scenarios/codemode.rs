//! Covers: batches exceeding the collapsed row budget must keep source visible
//! alongside bounded live status, including wrapped rows on a narrow terminal.
//! Owner: interactive UX; marker gates keep both updates observable.

use super::{fixture_release::release_fixture, Step, STARTUP, STREAM};

pub(super) const STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture codemode"),
    Step::Phase("nested_progress"),
    Step::WaitText {
        text: "progress one",
        timeout: STREAM,
    },
    Step::AssertText("12/13 completed"),
    Step::AssertText("hits = call_tools("),
    Step::Phase("wrapped_progress"),
    Step::Resize { rows: 30, cols: 72 },
    Step::WaitText {
        text: "hits = call_tools(",
        timeout: STREAM,
    },
    Step::Custom(|harness| release_fixture(harness, ".rho-fixture-codemode-1")),
    Step::WaitText {
        text: "progress two",
        timeout: STREAM,
    },
    Step::Custom(|harness| {
        if harness.screen().contains_text("progress one") {
            anyhow::bail!("codemode accumulated stale progress instead of replacing its snapshot");
        }
        super::assert_helpers::assert_no_escaped_script_json(harness)
    }),
    Step::AssertText("12/13 completed"),
    Step::AssertText("hits = call_tools("),
    Step::Resize {
        rows: 30,
        cols: 100,
    },
    Step::Custom(|harness| release_fixture(harness, ".rho-fixture-codemode-2")),
    Step::Phase("finished"),
    Step::WaitText {
        text: "codemode fixture complete",
        timeout: STREAM,
    },
    Step::AssertText("codemode(13 calls)"),
    Step::AssertText("hits = call_tools("),
    Step::AssertText("print(\"codemode fixture batch\", len(hits))"),
    Step::Custom(super::assert_helpers::assert_no_escaped_script_json),
    Step::ExitCommand,
];
