use super::*;

pub(super) const GOAL_BLOCKED_AND_RESUMED_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("discover_goal_actions"),
    Step::TypeText("/goal"),
    Step::WaitText {
        text: "/goal [condition|resume|clear]",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "/goal resume",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "/goal clear",
        timeout: STREAM,
    },
    Step::Key(Key::Tab),
    Step::Phase("block_goal"),
    Step::TypeText("fixture goal blocked"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "goal blocked: remaining steps need you",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "publish the fixture release",
        timeout: STREAM,
    },
    Step::Phase("inspect_blocked_goal"),
    Step::SubmitText("/goal"),
    Step::WaitText {
        text: "goal blocked: fixture goal blocked",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "use /goal resume",
        timeout: STREAM,
    },
    Step::Phase("resume_goal"),
    Step::SubmitText("/goal resume"),
    Step::WaitText {
        text: "verified that the fixture release is now published",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "goal achieved",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

fn assert_goal_waited_for_subagents(harness: &mut crate::PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    let raw = String::from_utf8_lossy(harness.raw_output());
    if screen.contains("goal continued before delegated agent finished")
        || raw.contains("goal continued before delegated agent finished")
    {
        anyhow::bail!("goal prompted the model while a delegated run was active:\n{screen}");
    }
    if screen.contains("goal not yet met: the delegated result still needs review")
        || raw.contains("goal not yet met: the delegated result still needs review")
    {
        anyhow::bail!("goal was evaluated before the delegated result was delivered:\n{screen}");
    }
    Ok(())
}

pub(super) const GOAL_WAITS_FOR_SUBAGENTS_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("delegate_goal_work"),
    Step::SubmitText("/goal fixture goal delegation"),
    Step::WaitText {
        text: "background agent completion received with delegated result (delivery 1)",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "goal achieved",
        timeout: STREAM,
    },
    Step::Custom(assert_goal_waited_for_subagents),
    Step::ExitCommand,
];

fn assert_goal_retry_waited_for_subagents(harness: &mut crate::PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if screen.contains("goal retry started before delegated agent finished") {
        anyhow::bail!("goal retried while a delegated run was active:\n{screen}");
    }
    Ok(())
}

pub(super) const GOAL_WAITS_FOR_SUBAGENTS_DURING_RETRY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("retry_after_delegation"),
    Step::SubmitText("/goal fixture goal delegation retry"),
    Step::WaitText {
        text: "goal retry resumed after delegated agent finished",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "goal achieved",
        timeout: STREAM,
    },
    Step::Custom(assert_goal_retry_waited_for_subagents),
    Step::ExitCommand,
];
