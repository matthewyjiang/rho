//! Collect a single-line secret without echoing it into a host terminal log.

use std::io::{self, IsTerminal, Write};

use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};

struct SecretTerminal;

impl Drop for SecretTerminal {
    fn drop(&mut self) {
        let _ = execute!(io::stderr(), DisableBracketedPaste);
        let _ = disable_raw_mode();
        let _ = writeln!(io::stderr());
    }
}

pub(super) fn read(label: &str) -> anyhow::Result<String> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        anyhow::bail!("API-key login requires an interactive terminal");
    }
    enable_raw_mode()?;
    let _terminal = SecretTerminal;
    execute!(io::stderr(), EnableBracketedPaste)?;
    write!(
        io::stderr(),
        "enter {label} (input hidden; Enter save, Esc cancel): "
    )?;
    io::stderr().flush()?;

    let mut secret = String::new();
    loop {
        match event::read()? {
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
