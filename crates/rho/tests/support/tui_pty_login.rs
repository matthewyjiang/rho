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
            "Managed by Claude Code",
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
fn login_shows_provider_picker_before_credential_store_choice() {
    let home = IsolatedHome::new().unwrap();
    // Deliberately leave behavior.credential_store unset so first normal
    // provider login must choose a store after the group picker.
    std::fs::write(
        &home.config_path,
        r#"web_search.mode = "off"

[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"
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
        r#"web_search.mode = "off"

[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"
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
            "handing the terminal to Claude Code",
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
        r#"web_search.mode = "off"

[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"

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

/// Where the TUI under test runs its session.
#[derive(Clone, Copy)]
enum SessionHost {
    InProcess,
    /// In the daemon the TUI follows (`RHO_TUI_DAEMON=1`, #1667).
    Daemon,
}

/// A 32x140 matrix launch in `home` whose session runs in `host`.
fn ollama_plan(home: &IsolatedHome, host: SessionHost) -> RhoLaunchPlan {
    let plan = RhoLaunchPlan::matrix(
        PathBuf::from(env!("CARGO_BIN_EXE_rho")),
        home,
        PtySize {
            rows: 32,
            cols: 140,
        },
    );
    match host {
        SessionHost::InProcess => plan,
        SessionHost::Daemon => plan.with_env("RHO_TUI_DAEMON", "1"),
    }
}

// Covers: an Ollama device login counts only after Enter, logout signs Rho
// out, and Esc on the prompt records nothing.
// Owner: interactive login lifecycle.
#[test]
fn ollama_device_login_needs_enter_and_logout_signs_out() {
    ollama_device_login_needs_enter(SessionHost::InProcess);
}

// Covers: the same with RHO_TUI_DAEMON=1, where the daemon records the
// login: not before Enter, and not at all after Esc.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_ollama_device_login_needs_enter_and_logout_signs_out() {
    ollama_device_login_needs_enter(SessionHost::Daemon);
}

fn ollama_device_login_needs_enter(host: SessionHost) {
    let home = IsolatedHome::new().unwrap();
    let plan = ollama_plan(&home, host)
        // Headless on every platform, so no real browser opens.
        .with_env("SSH_CONNECTION", "127.0.0.1 1 127.0.0.1 2");
    let mut harness = PtyHarness::spawn_named(&plan, "ollama_device_login").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    harness.submit_text("/login ollama-cloud-device").unwrap();
    harness
        .wait_for_text("Enter done", WaitTimeout::secs(10, "device prompt"))
        .unwrap();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text(
            "stored credentials for ollama-cloud",
            WaitTimeout::secs(10, "device login finished"),
        )
        .unwrap();

    harness.submit_text("/logout ollama-cloud-device").unwrap();
    harness
        .wait_for_text(
            "deleted stored credentials for ollama-cloud",
            WaitTimeout::secs(10, "device logout"),
        )
        .unwrap();

    harness.submit_text("/login ollama-cloud-device").unwrap();
    harness
        .wait_for_text("Enter done", WaitTimeout::secs(10, "second device prompt"))
        .unwrap();
    harness.inject_key(&Key::Esc).unwrap();
    harness
        .wait_for_text_gone("Enter done", WaitTimeout::secs(5, "prompt closed"))
        .unwrap();
    harness.submit_text("/logout ollama-cloud-device").unwrap();
    harness
        .wait_for_text(
            "no stored credentials for ollama-cloud were present",
            WaitTimeout::secs(10, "cancelled login recorded nothing"),
        )
        .unwrap();
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

/// Serves one chat model from Ollama's native `/api/tags` on a loopback port.
/// Returns the `/v1` API base to log in with and a count of requests served.
fn spawn_fake_ollama() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let served = std::sync::Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            // Drain headers; only GET /api/tags is answered with models.
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 0) && line != "\r\n" {
                line.clear();
            }
            let body = if request_line.starts_with("GET /api/tags ") {
                r#"{"models":[{"name":"llama-pty:latest","details":{"context_length":8192},"capabilities":["completion","tools"]}]}"#
            } else {
                "{}"
            };
            let _ = write!(
                reader.get_mut(),
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (base, requests)
}

// Covers: Ollama is refreshed and listed only after /login ollama, key or no
// key; a list cached before its table was removed stops showing on the next
// start; /logout ollama signs a keyless login out and drops its list.
// Owner: interactive provider setup.
#[test]
fn ollama_is_refreshed_and_listed_only_after_login() {
    ollama_is_refreshed_and_listed_after_login(SessionHost::InProcess);
}

// Covers: the same with RHO_TUI_DAEMON=1, where the daemon records and
// forgets the login.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_ollama_is_refreshed_and_listed_only_after_login() {
    ollama_is_refreshed_and_listed_after_login(SessionHost::Daemon);
}

fn ollama_is_refreshed_and_listed_after_login(host: SessionHost) {
    let home = IsolatedHome::new().unwrap();
    let original_config = std::fs::read_to_string(&home.config_path).unwrap();
    let (ollama_base, ollama_requests) = spawn_fake_ollama();
    let plan = ollama_plan(&home, host);
    let mut harness = PtyHarness::spawn_named(&plan, "ollama_login_only").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    // Refresh-all reports the models.dev failure last (offline proxy), so by
    // then any Ollama refresh would already be in the transcript.
    harness.submit_text("/refresh-models").unwrap();
    harness
        .wait_for_text(
            "failed to refresh the models.dev catalog",
            WaitTimeout::secs(20, "refresh-all finished"),
        )
        .unwrap();
    let screen = harness.screen().contents();
    assert!(
        !screen.contains("ollama"),
        "refresh-all must not touch Ollama before /login ollama:\n{screen}"
    );

    keyless_ollama_login_lists_its_model(&mut harness, &ollama_base);
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);

    // Dropping [providers.ollama] undoes the login; its cached list must go.
    std::fs::write(&home.config_path, &original_config).unwrap();
    let mut harness = PtyHarness::spawn_named(&plan, "ollama_login_removed").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "restart"))
        .unwrap();
    expect_no_ollama_model_listed(&mut harness, host, "ollama list dropped");

    // A keyless login is still a login, so /logout ollama signs it out.
    keyless_ollama_login_lists_its_model(&mut harness, &ollama_base);
    harness.submit_text("/logout ollama").unwrap();
    harness
        .wait_for_text("logged out of ollama", WaitTimeout::secs(10, "logout"))
        .unwrap();
    // A fresh transcript, so the notice below cannot be the earlier one.
    harness.submit_text("/new").unwrap();
    harness
        .wait_for_text_gone(
            "logged out of ollama",
            WaitTimeout::secs(10, "fresh transcript"),
        )
        .unwrap();
    expect_no_ollama_model_listed(&mut harness, host, "logout drops the ollama list");
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
    let config = std::fs::read_to_string(&home.config_path).unwrap();
    assert!(
        !config.contains("[providers.ollama]"),
        "logout must remove the login record:\n{config}"
    );

    // The daemon refuses to open a session whose auth has no login, so a
    // TUI following one has nothing to start signed out in yet.
    if matches!(host, SessionHost::Daemon) {
        return;
    }

    // A saved Ollama selection without its login starts signed out, and
    // startup asks nothing of the server.
    std::fs::write(
        &home.config_path,
        original_config.replacen(
            "[model]\nprovider = \"openai\"\nmodel = \"gpt-5.5\"\nauth = \"api-key\"\n",
            "[model]\nprovider = \"ollama\"\nmodel = \"llama-pty:latest\"\nauth = \"none\"\n",
            1,
        ),
    )
    .unwrap();
    let served_before = ollama_requests.load(std::sync::atomic::Ordering::SeqCst);
    let mut harness = PtyHarness::spawn_named(&plan, "ollama_saved_without_login").unwrap();
    harness
        .wait_for_text(
            "Not signed in",
            WaitTimeout::secs(20, "saved ollama starts signed out"),
        )
        .unwrap();
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
    assert_eq!(
        ollama_requests.load(std::sync::atomic::Ordering::SeqCst),
        served_before,
        "startup must not probe an Ollama that is not logged in"
    );
}

/// Opens `/model` and checks it lists no model of the fake Ollama server.
/// In process nothing else is cached, so the picker does not open; a daemon
/// session's picker always offers the session's own model.
fn expect_no_ollama_model_listed(harness: &mut PtyHarness, host: SessionHost, what: &'static str) {
    harness.submit_text("/model").unwrap();
    match host {
        SessionHost::InProcess => {
            harness
                .wait_for_text("no cached provider models", WaitTimeout::secs(10, what))
                .unwrap();
        }
        SessionHost::Daemon => {
            harness
                .wait_for_text("select model", WaitTimeout::secs(10, what))
                .unwrap();
            let screen = harness.screen().contents();
            assert!(!screen.contains("llama-pty"), "{what}:\n{screen}");
            harness.inject_key(&Key::Esc).unwrap();
            harness
                .wait_for_text_gone("select model", WaitTimeout::secs(5, "picker closed"))
                .unwrap();
        }
    }
}

/// Runs `/login ollama` against `base` with no key, then checks the login
/// refreshed and lists the fake server's model.
fn keyless_ollama_login_lists_its_model(harness: &mut PtyHarness, base: &str) {
    harness.submit_text("/login ollama").unwrap();
    harness
        .wait_for_text("edit base URL", WaitTimeout::secs(10, "endpoint prompt"))
        .unwrap();
    for _ in 0..rho_providers::provider::OLLAMA_API_BASE.len() {
        harness.inject_key(&Key::Backspace).unwrap();
    }
    harness.type_text(base).unwrap();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text(
            "enter API key (optional)",
            WaitTimeout::secs(10, "key prompt"),
        )
        .unwrap();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text(
            "refreshed ollama model list: 1 models",
            WaitTimeout::secs(20, "login refreshes ollama"),
        )
        .unwrap();
    harness.submit_text("/model").unwrap();
    harness
        .wait_for_text(
            "llama-pty:latest",
            WaitTimeout::secs(10, "ollama model listed"),
        )
        .unwrap();
    harness.inject_key(&Key::Esc).unwrap();
    harness
        .wait_for_text_gone("llama-pty:latest", WaitTimeout::secs(5, "picker closed"))
        .unwrap();
}

// Covers: `rho login` to a custom provider on a machine whose session has
// no D-Bus, as an SSH login's: the daemon it starts cannot reach the OS
// store, so the store prompt marks it unavailable and Enter takes the file
// store; the daemon saves that choice and keeps the key there (#1662).
// Owner: rho login CLI against the daemon it starts (PTY, real terminal).
#[cfg(target_os = "linux")]
#[test]
fn rho_login_without_a_session_bus_falls_back_to_the_file_store_it_offers() {
    let home = IsolatedHome::new().unwrap();
    // Nothing chooses the store; the sandbox's empty XDG_RUNTIME_DIR is the
    // missing session bus, for `rho login` and the daemon it starts.
    // The key check reaches the discard port, so the key saves unchecked.
    std::fs::write(
        &home.config_path,
        "[model]\nprovider = \"mock\"\nmodel = \"mock-model\"\nauth = \"mock-api-key\"\n\n[providers.custom.mock]\nbase_url = \"http://127.0.0.1:9/v1\"\n",
    )
    .unwrap();
    let plan = RhoLaunchPlan::matrix(
        PathBuf::from(env!("CARGO_BIN_EXE_rho")),
        &home,
        PtySize {
            rows: 28,
            cols: 200,
        },
    )
    .with_arg("login")
    .with_arg("mock");
    let mut harness = PtyHarness::spawn_named(&plan, "rho_login_headless_store").unwrap();

    harness
        .wait_for_text("input hidden", WaitTimeout::secs(20, "key prompt"))
        .unwrap();
    harness.type_text("sk-test-headless").unwrap();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text("(default 2)", WaitTimeout::secs(20, "store prompt"))
        .unwrap();
    harness
        .assert_screen_contains("OS credential store (unavailable:")
        .unwrap();
    harness.inject_key(&Key::Enter).unwrap();
    assert_eq!(
        harness
            .wait_for_exit(WaitTimeout::secs(30, "rho login exits"))
            .unwrap(),
        0,
        "{}",
        harness.screen().contents()
    );

    let config = std::fs::read_to_string(&home.config_path).unwrap();
    let store =
        rho_providers::credentials::FileCredentialStore::with_rho_home(home.rho_home()).unwrap();
    use rho_providers::credentials::CredentialStore;
    assert_eq!(
        (
            config.contains("credential_store = \"file\""),
            store.get_secret("provider:mock:api-key").unwrap()
        ),
        (true, Some("sk-test-headless".to_owned())),
        "{config}"
    );
}
