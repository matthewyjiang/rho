use super::*;

// Covers: search configuration must persist endpoint edits and mode/backend
// selection without selecting a backend merely by opening its settings page.
// Owner: interactive TUI
pub(super) const WEB_SEARCH_CONFIG_SCENARIO: Scenario = Scenario::new(
    "web_search_config",
    "Configure search routing and self-hosted endpoints; reset and cancel a connection test",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("/config"),
        Step::WaitText {
            text: "Appearance",
            timeout: SETTLE,
        },
        Step::TypeText("Tools"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Inline shell",
            timeout: SETTLE,
        },
        Step::TypeText("web_search"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Next turn route",
            timeout: SETTLE,
        },
        Step::Phase("force_backend"),
        // Isolated matrix config begins Off. Pick Backend, then Firecrawl.
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Web search mode",
            timeout: SETTLE,
        },
        Step::TypeText("web_search_mode:backend"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Next turn route",
            timeout: SETTLE,
        },
        Step::Key(Key::Down),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Web search backend",
            timeout: SETTLE,
        },
        Step::TypeText("web_search_backend:firecrawl"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Firecrawl API",
            timeout: SETTLE,
        },
        Step::Phase("edit_endpoint"),
        Step::TypeText("web_search_firecrawl"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Firecrawl API base URL",
            timeout: SETTLE,
        },
        Step::Key(Key::Enter),
        Step::Paste("http://127.0.0.1:3002/proxy"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "http://127.0.0.1:3002/proxy",
            timeout: SETTLE,
        },
        Step::Phase("reopen_persisted_endpoint"),
        Step::Key(Key::Esc),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Firecrawl API base URL",
            timeout: SETTLE,
        },
        Step::AssertText("http://127.0.0.1:3002/proxy"),
        Step::Phase("reset_endpoint"),
        Step::Key(Key::Down),
        Step::Key(Key::Enter),
        Step::WaitTextGone {
            text: "http://127.0.0.1:3002/proxy",
            timeout: SETTLE,
        },
        Step::AssertText("https://api.firecrawl.dev"),
        Step::Key(Key::Esc),
        Step::Custom(clear_filter),
        Step::TypeText("web_search_test"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Send test query",
            timeout: SETTLE,
        },
        Step::Phase("cancel_without_network"),
        Step::Key(Key::Char('n')),
        Step::WaitText {
            text: "Test connection",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        Step::Key(Key::Esc),
        Step::Key(Key::Esc),
        Step::Custom(test_connection_lifecycle),
    ],
    /*smoke*/ true,
)
.with_env(OPENAI_KEY_ENV);

// The server holds each response until the UI has handled input. Channels make
// pending-state coverage independent of machine speed and external services.
fn test_connection_lifecycle(harness: &mut crate::harness::PtyHarness) -> Result<()> {
    use anyhow::{ensure, Context};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        time::Duration,
    };

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}/proxy", listener.local_addr()?);
    let (requests_tx, requests_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = std::thread::spawn(move || -> Result<()> {
        for index in 0..3 {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(15)))?;
            let mut bytes = Vec::new();
            let mut byte = [0];
            while !bytes.ends_with(b"\r\n\r\n") {
                ensure!(stream.read(&mut byte)? == 1, "request headers ended early");
                bytes.push(byte[0]);
                ensure!(bytes.len() < 16384, "oversized request headers");
            }
            let headers = String::from_utf8(bytes)?;
            ensure!(
                headers.starts_with("POST /proxy/v2/search HTTP/1.1"),
                "wrong endpoint: {headers}"
            );
            ensure!(
                !headers.to_ascii_lowercase().contains("authorization:"),
                "selfhost sent authorization"
            );
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>())
                })
                .context("missing content length")??;
            let mut body = vec![0; length];
            stream.read_exact(&mut body)?;
            let body: serde_json::Value = serde_json::from_slice(&body)?;
            ensure!(
                body["query"] == "rho web search connection test",
                "wrong test query"
            );
            requests_tx.send(())?;
            if index == 2 {
                ensure!(
                    stream.read(&mut byte)? == 0,
                    "shutdown did not close the pending request"
                );
            } else {
                release_rx.recv_timeout(Duration::from_secs(15))?;
                let (status, body) = if index == 0 {
                    (
                        "200 OK",
                        r#"{"success":true,"data":{"web":[{"title":"Local mock","url":"https://example.com","description":"Local result"}]}}"#,
                    )
                } else {
                    (
                        "503 Unavailable",
                        r#"{"success":false,"error":"local mock unavailable"}"#,
                    )
                };
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
            }
        }
        Ok(())
    });

    harness.submit_text("/config")?;
    harness.wait_for_text("Appearance", SETTLE)?;
    select(harness, "Tools")?;
    harness.wait_for_text("Inline shell", SETTLE)?;
    select(harness, "web_search")?;
    harness.wait_for_text("Next turn route", SETTLE)?;
    select(harness, "web_search_firecrawl")?;
    harness.wait_for_text("Firecrawl API base URL", SETTLE)?;
    harness.inject_key(&Key::Enter)?;
    harness.paste(&endpoint)?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text(&endpoint, SETTLE)?;
    harness.inject_key(&Key::Esc)?;

    // Applying an endpoint must enable the live tool on the next turn.
    close_search_config(harness)?;
    harness.submit_text("fixture tool available web_search")?;
    harness.wait_for_text("tool available web_search: true", STREAM)?;
    harness.submit_text("fixture delay")?;
    harness.wait_for_text("partial assistant before cancellation", STREAM)?;
    open_search_config(harness)?;
    select(harness, "web_search_mode")?;
    harness.wait_for_text("Web search mode", SETTLE)?;
    select(harness, "web_search_mode:off")?;
    harness.wait_for_text("web search mode: Off; applies next turn", SETTLE)?;
    close_search_config(harness)?;
    harness.inject_key(&Key::Esc)?;
    harness.wait_for_text("model interrupted", STREAM)?;
    harness.submit_text("fixture tool available web_search")?;
    harness.wait_for_text("tool available web_search: false", STREAM)?;
    open_search_config(harness)?;

    // Tests deliberately use the selected backend even after turning search Off.
    for index in 0..3 {
        harness.set_phase(format!("confirmed_test_{index}"));
        select(harness, "web_search_test")?;
        harness.wait_for_text("Send test query", SETTLE)?;
        harness.inject_key(&Key::Char('y'))?;
        requests_rx
            .recv_timeout(Duration::from_secs(15))
            .context("test did not reach local endpoint")?;
        // Filtering remains responsive while the server holds the response.
        clear_filter(harness)?;
        harness.type_text("web_search_firecrawl")?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("Firecrawl API base URL", SETTLE)?;
        if index == 2 {
            harness.inject_key(&Key::Esc)?;
            close_search_config(harness)?;
            ensure!(harness.quit_with_exit_command()? == 0, "shutdown failed");
        } else {
            release_tx.send(())?;
            harness.wait_for_text(
                if index == 0 {
                    "web search test: 1 result"
                } else {
                    "could not test web search"
                },
                SETTLE,
            )?;
            harness.inject_key(&Key::Esc)?;
        }
    }
    server
        .join()
        .map_err(|_| anyhow::anyhow!("mock server panicked"))??;
    Ok(())
}

fn select(harness: &mut crate::harness::PtyHarness, label: &str) -> Result<()> {
    clear_filter(harness)?;
    harness.type_text(label)?;
    harness.settle_plain_text_input();
    harness.inject_key(&Key::Enter)
}

fn clear_filter(harness: &mut crate::harness::PtyHarness) -> Result<()> {
    // All scenario filters fit in this longest row identifier. Picker filters
    // support Backspace, while Ctrl+U belongs to the message composer.
    for _ in "web_search_firecrawl".chars() {
        harness.inject_key(&Key::Backspace)?;
    }
    Ok(())
}

fn close_search_config(harness: &mut crate::harness::PtyHarness) -> Result<()> {
    for _ in 0..3 {
        // Web search -> Tools -> Config -> composer.
        harness.inject_key(&Key::Esc)?;
    }
    harness.wait_for_text_gone("Type to search", SETTLE)
}

fn open_search_config(harness: &mut crate::harness::PtyHarness) -> Result<()> {
    harness.submit_text("/config")?;
    harness.wait_for_text("Appearance", SETTLE)?;
    select(harness, "Tools")?;
    harness.wait_for_text("Inline shell", SETTLE)?;
    select(harness, "web_search")?;
    harness.wait_for_text("Next turn route", SETTLE)
}
