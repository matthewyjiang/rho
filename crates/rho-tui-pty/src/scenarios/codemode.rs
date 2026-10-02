//! Covers: nested progress updates must reach the running card without replacing
//! its source. Owner: interactive UX; marker gates keep both updates observable.

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
    Step::AssertText("hits = call_tools("),
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
    Step::Custom(|harness| release_fixture(harness, ".rho-fixture-codemode-2")),
    Step::Phase("finished"),
    Step::WaitText {
        text: "codemode fixture complete",
        timeout: STREAM,
    },
    Step::AssertText("codemode(2 calls)"),
    Step::AssertText("hits = call_tools("),
    Step::AssertText("print(\"codemode fixture batch\", len(hits))"),
    Step::Custom(super::assert_helpers::assert_no_escaped_script_json),
    Step::ExitCommand,
];
