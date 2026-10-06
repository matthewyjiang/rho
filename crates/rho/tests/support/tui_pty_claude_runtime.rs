//! Fake Claude runtime delegation, delivery, cost accounting, and advisor flow.

use rho_tui_pty::{IsolatedHome, Key, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout};
use std::{fs, path::PathBuf, time::Duration};

use super::claude_e2e;

/// Full fake-Claude runtime path: matrix parent -> agent tool -> binder/executor
/// -> `claude -p` spawn -> stream-json -> result/events persistence -> parent
/// completion UI -> `rho attach` replay. Never touches a real Claude binary or
/// the network.
#[test]
fn fake_claude_runtime_end_to_end_success() {
    let home = IsolatedHome::new().unwrap();
    claude_e2e::install_claude_planner_agent(&home.home);

    let fake_root = home.path().join("fake-claude");
    let fake = claude_e2e::install_fake_claude(&fake_root, claude_e2e::FakeClaudeMode::Success);
    let path = claude_e2e::path_with_fake(&fake.bin_dir);

    // Prove the isolated PATH cannot resolve a host Claude: only our stub.
    assert!(fake.claude.is_file());
    assert_eq!(
        which_on_path("claude", &path).as_deref(),
        Some(fake.claude.as_path()),
        "PATH must resolve the fake claude first"
    );

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 32,
            cols: 120,
        },
    )
    .with_env("PATH", path);
    let mut harness = PtyHarness::spawn_named(&plan, "fake_claude_runtime_e2e").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    // Real agent-tool path via matrix fixture prompt, without an execution-mode flag.
    harness.submit_text("fixture claude agent").unwrap();
    harness
        .wait_for_text(
            "claude-planner",
            WaitTimeout::secs(15, "agent tool started"),
        )
        .unwrap();

    claude_e2e::wait_for_spawn(&fake, Duration::from_secs(15));
    let record = fake.read_spawn_record();
    claude_e2e::assert_success_spawn(&record, &home.workspace);

    // Wait for the parent's durable response to the completion notification,
    // not the short-lived launch receipt or a card that can scroll out of view.
    harness
        .wait_for_text(
            "claude-background-delivery-1: delegated result received",
            WaitTimeout::secs(20, "parent received delegated result"),
        )
        .unwrap();

    let run_dir = claude_e2e::wait_for_single_run_dir(&home.home, Duration::from_secs(10));
    let run_id = run_dir
        .file_name()
        .and_then(|name| name.to_str())
        .expect("run id")
        .to_string();
    let status = claude_e2e::wait_for_terminal_result(&run_dir, Duration::from_secs(10));
    claude_e2e::assert_success_result(&status, &run_dir);
    assert!(
        run_dir.starts_with(home.home.join(".rho/sessions")),
        "interactive run was not nested under its session: {}",
        run_dir.display()
    );
    assert!(
        !home.home.join(".rho/subagents").join(&run_id).exists(),
        "interactive run also appeared in the global pool"
    );

    // Offline proof: only the fake binary ran; spawn marker is under the temp root.
    assert!(
        fake.spawn_marker.starts_with(home.path()),
        "spawn marker escaped isolated root: {}",
        fake.spawn_marker.display()
    );
    assert!(
        record.args.iter().all(|arg| !arg.contains("anthropic.com")),
        "spawn argv must not reference network endpoints: {:?}",
        record.args
    );

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);

    // Replay the real on-disk artifacts through `rho attach`.
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let mut plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 120,
        },
    )
    .with_arg("attach")
    .with_arg(&run_id);
    plan.cwd = home.path().join("attach-workspace");
    fs::create_dir_all(&plan.cwd).unwrap();
    let mut attach = PtyHarness::spawn_named(&plan, "fake_claude_runtime_e2e_attach").unwrap();
    attach
        .wait_for_text(
            &format!("attach {run_id}"),
            WaitTimeout::secs(10, "attach startup"),
        )
        .unwrap();
    attach
        .wait_for_text(
            "Say hello in one short sentence.",
            WaitTimeout::secs(5, "attach prompt"),
        )
        .unwrap();
    // Partial deltas are stored separately; the live fixture splits the final
    // phrase across two assistant_text_delta events ("r" + "ho-claude-e2e-ok").
    attach
        .wait_for_text(
            "ho-claude-e2e-ok",
            WaitTimeout::secs(5, "attach final text tail"),
        )
        .unwrap();
    attach
        .wait_for_text(
            // Resolved Claude models lengthen the identity line, so the full
            // session UUID ellipsizes on a 120-col attach header. The stable
            // prefix still proves the session id landed.
            "claude 11111111",
            WaitTimeout::secs(5, "attach session id"),
        )
        .unwrap();
    attach
        .wait_for_text(
            "ran as claude-sonnet-5",
            WaitTimeout::secs(5, "attach resolved model"),
        )
        .unwrap();
    attach
        .wait_for_text("claude-planner", WaitTimeout::secs(5, "attach agent id"))
        .unwrap();
    assert!(attach.screen().contains_text("read-only"));
    assert!(attach.screen().contains_text("ok") || attach.screen().contains_text("complete"));
    // Joined final text lives in result.json (asserted earlier); attach stream
    // fidelity keeps the partial pieces visible.
    let attach_screen = attach.screen().contents();
    assert!(
        attach_screen.contains('r') && attach_screen.contains("ho-claude-e2e-ok"),
        "attach should replay streamed halves:\n{attach_screen}"
    );
    attach.inject_key(&Key::Char('q')).unwrap();
    assert_eq!(
        attach
            .wait_for_exit(WaitTimeout::secs(5, "detach"))
            .unwrap(),
        0
    );
}

/// Background Claude completion path: terminal `total_cost_usd` must fold into
/// the parent session total shown by `/info` after automatic delivery.
#[test]
fn fake_claude_background_cost_appears_in_info() {
    let home = IsolatedHome::new().unwrap();
    claude_e2e::install_claude_planner_agent(&home.home);

    let fake_root = home.path().join("fake-claude");
    let fake = claude_e2e::install_fake_claude(&fake_root, claude_e2e::FakeClaudeMode::Success);
    let path = claude_e2e::path_with_fake(&fake.bin_dir);

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 36,
            cols: 120,
        },
    )
    .with_env("PATH", path);
    let mut harness = PtyHarness::spawn_named(&plan, "fake_claude_background_cost_info").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    harness
        .submit_text("fixture background claude agent")
        .unwrap();
    // The fake emits its terminal result before marking spawn. Its dispatch
    // echo can already be superseded, so assert durable delivery and cost below.
    claude_e2e::wait_for_spawn(&fake, Duration::from_secs(15));
    let run_dir = claude_e2e::wait_for_single_run_dir(&home.home, Duration::from_secs(10));
    let status = claude_e2e::wait_for_terminal_result(&run_dir, Duration::from_secs(10));
    claude_e2e::assert_success_result(&status, &run_dir);

    harness
        .wait_for_text(
            "claude-background-delivery-1:",
            WaitTimeout::secs(20, "completion delivery"),
        )
        .unwrap();

    // Delivery should include the fixture cost on the statusline total.
    harness
        .wait_for_text("$0.034", WaitTimeout::secs(10, "statusline subagent cost"))
        .unwrap();

    harness.submit_text("/info").unwrap();
    harness
        .wait_for_text("Session usage", WaitTimeout::secs(10, "info opened"))
        .unwrap();
    harness
        .wait_for_text(
            "Subagent cost",
            WaitTimeout::secs(10, "subagent cost label"),
        )
        .unwrap();
    harness
        .wait_for_text("$0.034", WaitTimeout::secs(10, "subagent cost value"))
        .unwrap();

    let screen = harness.screen().contents();
    assert!(
        screen.contains("Subagent cost") && screen.contains("$0.034"),
        "expected /info to show delivered subagent cost:\n{screen}"
    );
    assert!(
        !screen.contains("No token usage recorded yet."),
        "subagent cost should replace the empty-usage note:\n{screen}"
    );

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

/// Sibling error path: fake Claude emits a terminal error stream and nonzero
/// exit; the parent surfaces a failed delegated run without network access.
#[test]
fn fake_claude_runtime_end_to_end_error() {
    let home = IsolatedHome::new().unwrap();
    claude_e2e::install_claude_planner_agent(&home.home);

    let fake_root = home.path().join("fake-claude");
    let fake = claude_e2e::install_fake_claude(&fake_root, claude_e2e::FakeClaudeMode::Error);
    let path = claude_e2e::path_with_fake(&fake.bin_dir);

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 32,
            cols: 120,
        },
    )
    .with_env("PATH", path);
    let mut harness = PtyHarness::spawn_named(&plan, "fake_claude_runtime_e2e_error").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();
    harness.submit_text("fixture claude agent error").unwrap();
    harness
        .wait_for_text(
            "claude-planner",
            WaitTimeout::secs(15, "agent tool started"),
        )
        .unwrap();

    claude_e2e::wait_for_spawn(&fake, Duration::from_secs(15));

    // The parent incorporates a failed child result through automatic delivery.
    harness
        .wait_for_text(
            "claude-background-delivery-1: failed result received",
            WaitTimeout::secs(20, "parent received delegated failure"),
        )
        .unwrap();

    let run_dir = claude_e2e::wait_for_single_run_dir(&home.home, Duration::from_secs(10));
    let status = claude_e2e::wait_for_terminal_result(&run_dir, Duration::from_secs(10));
    claude_e2e::assert_error_result(&status);
    assert!(
        fake.spawn_marker.exists(),
        "error path must still have spawned the fake binary"
    );

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

/// Resolve `program` on a PATH string the same way a shell would (first hit).
fn which_on_path(program: &str, path_var: &str) -> Option<PathBuf> {
    for dir in path_var.split(':').filter(|dir| !dir.is_empty()) {
        let candidate = PathBuf::from(dir).join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Full Claude Code advisor path: picker selection -> config -> advisor tool
/// call -> `claude -p` spawn with no tools -> stream-json -> advice in the card.
/// Never touches a real Claude binary or the network.
#[test]
fn fake_claude_advisor_reviews_the_session() {
    let home = IsolatedHome::new().unwrap();
    std::fs::write(
        &home.config_path,
        r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search.mode = "off"

[behavior]
credential_store = "file"
advisor_mode = true
"#,
    )
    .unwrap();

    let fake_root = home.path().join("fake-claude");
    let fake = claude_e2e::install_fake_claude(&fake_root, claude_e2e::FakeClaudeMode::Success);
    let path = claude_e2e::path_with_fake(&fake.bin_dir);
    assert_eq!(
        which_on_path("claude", &path).as_deref(),
        Some(fake.claude.as_path()),
        "PATH must resolve the fake claude first"
    );

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 32,
            cols: 120,
        },
    )
    .with_env("PATH", path);
    let mut harness = PtyHarness::spawn_named(&plan, "fake_claude_advisor").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    // Picking a Claude Code row is the only step: it selects the runtime too.
    harness.submit_text("/advisor on").unwrap();
    harness
        .wait_for_text(
            "select model for advisor",
            WaitTimeout::secs(10, "advisor model picker"),
        )
        .unwrap();
    harness.type_text("claude-code/opus").unwrap();
    harness
        .wait_for_text("(1/1)", WaitTimeout::secs(10, "claude code row filtered"))
        .unwrap();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text(
            "advisor mode is on: claude-code/opus reviews the session",
            WaitTimeout::secs(10, "advisor turned on"),
        )
        .unwrap();
    harness
        .wait_for_text(
            "advisor: claude-code/opus",
            WaitTimeout::secs(10, "advisor divider"),
        )
        .unwrap();

    harness.submit_text("fixture advisor").unwrap();
    claude_e2e::wait_for_spawn(&fake, Duration::from_secs(20));
    let record = fake.read_spawn_record();

    // Parity with the Rho advisor: one turn, no tools, no workspace access.
    // The recorder joins argv on NUL and drops empty chunks, so the empty
    // `--tools` value shows up as the flag with nothing of its own after it.
    assert!(
        value_after(&record.args, "--tools").is_none_or(|value| value.starts_with("--")),
        "the advisor must run with no tools: {:?}",
        record.args
    );
    assert!(
        record.args.iter().any(|arg| arg == "--tools"),
        "--tools must always be set so ambient tools are not inherited: {:?}",
        record.args
    );
    assert!(
        !record.args.iter().any(|arg| arg == "--allowedTools"),
        "the advisor must allow no tools: {:?}",
        record.args
    );
    for pair in [
        ["--model", "opus"],
        ["--max-turns", "1"],
        ["--permission-mode", "dontAsk"],
    ] {
        assert!(
            record.args.windows(2).any(|window| window == pair),
            "missing {pair:?}: {:?}",
            record.args
        );
    }
    assert!(
        record
            .args
            .iter()
            .any(|arg| arg == "--no-session-persistence"),
        "a one-shot advisor call must leave no resumable session: {:?}",
        record.args
    );
    let prompt =
        value_after(&record.args, "--system-prompt").expect("advisor system prompt on argv");
    assert!(
        !prompt.is_empty(),
        "advisor system prompt must not be empty"
    );
    assert!(
        record.stdin.contains("fixture advisor"),
        "the advisor must receive the session transcript on stdin: {}",
        record.stdin
    );

    // The fixture's result text is the advice the executor gets back.
    harness
        .wait_for_text(
            "rho-claude-e2e-ok",
            WaitTimeout::secs(20, "advice in the advisor card"),
        )
        .unwrap();

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

/// First value following `flag` in a recorded argv.
fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str())
}
