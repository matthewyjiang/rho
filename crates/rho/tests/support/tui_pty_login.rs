//! Terminal handoff and credential-store selection during login.

use rho_tui_pty::{IsolatedHome, Key, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout};
use std::{path::PathBuf, time::Duration};

use super::claude_e2e;

fn confirm_claude_code_login(harness: &mut PtyHarness) {
    harness
        .wait_for_text(
            "Hand the terminal to Claude Code?",
            WaitTimeout::secs(10, "login confirmation"),
        )
        .unwrap();
    // Cancel is the default option, so pick Continue by its shortcut.
    harness.inject_key(&Key::Char('2')).unwrap();
}

fn wait_for_claude_code_login_complete(harness: &mut PtyHarness) {
    // Resume re-enables bracketed paste. The status line can appear before the
    // event loop is reading again; quitting then turns `\x1b[200~/exit` into
    // literal `[200~/exit` and the child never leaves.
    harness
        .wait_for_text(
            "signed in as fake@example.com",
            WaitTimeout::secs(10, "post-login status"),
        )
        .unwrap();
    harness
        .wait_for_text(
            "Managed by the claude binary",
            WaitTimeout::secs(10, "ownership copy"),
        )
        .unwrap();
    harness
        .wait_for_quiet(
            Duration::from_millis(200),
            WaitTimeout::secs(5, "idle after login"),
        )
        .unwrap();
}

#[test]
fn claude_code_login_hands_terminal_to_fake_claude() {
    let home = IsolatedHome::new().unwrap();
    // Ensure /login claude-code does not stop at credential-store choice.
    std::fs::write(
        &home.config_path,
        r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search.mode = "off"

[behavior]
credential_store = "file"
"#,
    )
    .unwrap();

    let fake = claude_e2e::install_fake_claude_login();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    )
    .with_env("PATH", &fake.path);
    let mut harness = PtyHarness::spawn_named(&plan, "claude_code_login").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    harness.submit_text("/login claude-code").unwrap();
    confirm_claude_code_login(&mut harness);
    harness
        .wait_for_text(
            "handing the terminal to the claude binary",
            WaitTimeout::secs(10, "handoff notice"),
        )
        .unwrap();
    // The fake claude process exits immediately, so its stdout may only appear on
    // the suspended main screen. Prefer the post-status source of truth.
    wait_for_claude_code_login_complete(&mut harness);
    assert!(fake.marker.exists(), "fake claude login should have run");
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

#[test]
fn login_shows_provider_picker_before_credential_store_choice() {
    let home = IsolatedHome::new().unwrap();
    // Deliberately leave behavior.credential_store unset so first normal
    // provider login must choose a store after the group picker.
    std::fs::write(
        &home.config_path,
        r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search.mode = "off"
"#,
    )
    .unwrap();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    );
    let mut harness = PtyHarness::spawn_named(&plan, "login_provider_then_store").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    // Bare /login opens the group picker first, not the store chooser.
    harness.submit_text("/login").unwrap();
    harness
        .wait_for_text(
            "Select provider to login",
            WaitTimeout::secs(10, "group picker first"),
        )
        .unwrap();
    let screen = harness.screen().contents();
    assert!(
        !screen.contains("Where should Rho store provider credentials?"),
        "store chooser must wait until a normal provider is selected:\n{screen}"
    );
    assert!(
        !screen.contains("Claude Code (delegation only)"),
        "claude-code belongs under Anthropic methods, not the top-level group picker:\n{screen}"
    );
    assert!(
        screen.contains("Anthropic"),
        "Anthropic group must remain in the bare login picker:\n{screen}"
    );

    // Filter to OpenAI so the test does not depend on picker sort order.
    harness.type_text("openai").unwrap();
    harness
        .wait_for_text("OpenAI", WaitTimeout::secs(5, "openai filtered"))
        .unwrap();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text(
            "Select OpenAI login method",
            WaitTimeout::secs(10, "openai methods"),
        )
        .unwrap();
    // API Key is the first method.
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text(
            "Where should Rho store provider credentials?",
            WaitTimeout::secs(10, "store after provider"),
        )
        .unwrap();
    harness
        .wait_for_text("Local file", WaitTimeout::secs(5, "file option"))
        .unwrap();
    harness.inject_key(&Key::Esc).unwrap();
    harness
        .wait_for_quiet(
            Duration::from_millis(150),
            WaitTimeout::secs(5, "after cancel"),
        )
        .unwrap();

    // Direct provider args still prompt for the store before secrets.
    harness.submit_text("/login openai").unwrap();
    harness
        .wait_for_text(
            "Where should Rho store provider credentials?",
            WaitTimeout::secs(10, "store for direct provider"),
        )
        .unwrap();
    harness.inject_key(&Key::Char('2')).unwrap();
    harness
        .wait_for_text(
            "credential store set to file",
            WaitTimeout::secs(10, "store persisted"),
        )
        .unwrap();
    harness
        .wait_for_text("enter", WaitTimeout::secs(10, "api key prompt"))
        .unwrap();
    harness.inject_key(&Key::Esc).unwrap();
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);

    let config = std::fs::read_to_string(&home.config_path).unwrap();
    assert!(
        config.contains("credential_store = \"file\""),
        "chooser should persist file backend:\n{config}"
    );

    // Second /login must not re-prompt once config is set.
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    );
    let mut harness = PtyHarness::spawn_named(&plan, "login_provider_then_store_again").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup again"))
        .unwrap();
    harness.submit_text("/login").unwrap();
    harness
        .wait_for_text(
            "Select provider to login",
            WaitTimeout::secs(10, "no second chooser"),
        )
        .unwrap();
    let screen = harness.screen().contents();
    assert!(
        !screen.contains("Where should Rho store provider credentials?"),
        "chooser should not reappear after config is set:\n{screen}"
    );
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

#[test]
fn login_claude_code_skips_credential_store_when_unset() {
    let home = IsolatedHome::new().unwrap();
    // Leave credential_store unset. Claude login must never ask for it.
    std::fs::write(
        &home.config_path,
        r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search.mode = "off"
"#,
    )
    .unwrap();

    let fake = claude_e2e::install_fake_claude_login();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    )
    .with_env("PATH", &fake.path);
    let mut harness = PtyHarness::spawn_named(&plan, "claude_code_login_no_store").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    harness.submit_text("/login claude-code").unwrap();
    confirm_claude_code_login(&mut harness);
    harness
        .wait_for_text(
            "handing the terminal to the claude binary",
            WaitTimeout::secs(10, "handoff notice"),
        )
        .unwrap();
    let screen = harness.screen().contents();
    assert!(
        !screen.contains("Where should Rho store provider credentials?"),
        "claude-code must never open the Rho store chooser:\n{screen}"
    );
    wait_for_claude_code_login_complete(&mut harness);
    assert!(fake.marker.exists(), "fake claude login should have run");
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);

    let config = std::fs::read_to_string(&home.config_path).unwrap();
    assert!(
        !config.contains("credential_store"),
        "claude login must not write Rho credential_store:\n{config}"
    );
}

// Covers: first-time /login claude-code cancel must not start the claude binary
// Owner: interactive UX
#[test]
fn login_claude_code_cancel_stays_in_rho() {
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
"#,
    )
    .unwrap();

    let fake = claude_e2e::install_fake_claude_login();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    )
    .with_env("PATH", &fake.path);
    let mut harness = PtyHarness::spawn_named(&plan, "claude_code_login_cancel").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    harness.submit_text("/login claude-code").unwrap();
    harness
        .wait_for_text(
            "Hand the terminal to Claude Code?",
            WaitTimeout::secs(10, "login confirmation"),
        )
        .unwrap();
    // A stray Enter must take the default option, and the default must be Cancel.
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_quiet(
            Duration::from_millis(150),
            WaitTimeout::secs(5, "after cancel"),
        )
        .unwrap();
    assert!(
        !fake.marker.exists(),
        "cancel must not run claude auth login"
    );
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}
