//! Collect a single-line secret without echoing it into a host terminal log.

use std::io::{self, IsTerminal, Write};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::tui::InlinePromptInput;

pub(super) async fn read(label: &str) -> anyhow::Result<String> {
    if !crate::stdio::stdin_is_terminal() || !io::stderr().is_terminal() {
        anyhow::bail!("API-key login requires an interactive terminal");
    }
    let secret = read_hidden(label).await;
    // Raw mode has ended, so this newline also returns the cursor.
    let _ = writeln!(io::stderr());
    secret
}

async fn read_hidden(label: &str) -> anyhow::Result<String> {
    let mut input = InlinePromptInput::acquire()?;
    write!(
        io::stderr(),
        "enter {label} (input hidden; Enter save, Esc cancel): "
    )?;
    io::stderr().flush()?;

    let mut secret = String::new();
    loop {
        match input.next().await? {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Esc => anyhow::bail!("login cancelled"),
                KeyCode::Char('c' | 'C' | 'd' | 'D')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    anyhow::bail!("login cancelled");
                }
                KeyCode::Enter => {
                    let secret = secret.trim();
                    if secret.is_empty() {
                        anyhow::bail!("API key cannot be empty");
                    }
                    return Ok(secret.to_owned());
                }
                KeyCode::Backspace => {
                    secret.pop();
                }
                KeyCode::Char(ch)
                    if !ch.is_control()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    secret.push(ch);
                }
                _ => {}
            },
            Event::Paste(text) => {
                let text = text.trim();
                if text.chars().any(char::is_control) {
                    anyhow::bail!("API key must be a single line");
                }
                secret.push_str(text);
            }
            _ => {}
        }
    }
}
