use std::{fmt, str::FromStr};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Configurable keyboard shortcuts used by the main TUI composer.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct Keybindings {
    pub reset_conversation: KeyBinding,
    pub jump_to_bottom: KeyBinding,
    pub toggle_tool_output: KeyBinding,
    pub insert_newline: KeyBinding,
    pub paste_image: KeyBinding,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            reset_conversation: KeyBinding::control('r'),
            jump_to_bottom: KeyBinding::control('g'),
            toggle_tool_output: KeyBinding::control('o'),
            insert_newline: KeyBinding::control('j'),
            paste_image: KeyBinding::control('v'),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyBinding {
    modifiers: KeyModifiers,
    code: KeyCode,
}

impl KeyBinding {
    const fn control(ch: char) -> Self {
        Self {
            modifiers: KeyModifiers::CONTROL,
            code: KeyCode::Char(ch),
        }
    }

    pub fn matches(&self, event: KeyEvent) -> bool {
        self.modifiers == event.modifiers && self.code == event.code
    }
}

impl fmt::Display for KeyBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("ctrl".to_string());
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            parts.push("alt".to_string());
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("shift".to_string());
        }
        parts.push(match self.code {
            KeyCode::Char(ch) => ch.to_string(),
            KeyCode::Enter => "enter".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::Delete => "delete".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::Up => "up".into(),
            KeyCode::Down => "down".into(),
            KeyCode::Left => "left".into(),
            KeyCode::Right => "right".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::PageUp => "pageup".into(),
            KeyCode::PageDown => "pagedown".into(),
            _ => return Err(fmt::Error),
        });
        formatter.write_str(&parts.join("+"))
    }
}

impl FromStr for KeyBinding {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parts = value.trim().to_ascii_lowercase();
        let mut modifiers = KeyModifiers::NONE;
        let mut code = None;
        for part in parts.split('+') {
            match part.trim() {
                "ctrl" | "control" => modifiers.insert(KeyModifiers::CONTROL),
                "alt" => modifiers.insert(KeyModifiers::ALT),
                "shift" => modifiers.insert(KeyModifiers::SHIFT),
                "enter" => set_code(&mut code, KeyCode::Enter)?,
                "backspace" => set_code(&mut code, KeyCode::Backspace)?,
                "delete" => set_code(&mut code, KeyCode::Delete)?,
                "esc" | "escape" => set_code(&mut code, KeyCode::Esc)?,
                "tab" => set_code(&mut code, KeyCode::Tab)?,
                "up" => set_code(&mut code, KeyCode::Up)?,
                "down" => set_code(&mut code, KeyCode::Down)?,
                "left" => set_code(&mut code, KeyCode::Left)?,
                "right" => set_code(&mut code, KeyCode::Right)?,
                "home" => set_code(&mut code, KeyCode::Home)?,
                "end" => set_code(&mut code, KeyCode::End)?,
                "pageup" => set_code(&mut code, KeyCode::PageUp)?,
                "pagedown" => set_code(&mut code, KeyCode::PageDown)?,
                part if part.chars().count() == 1 => {
                    set_code(&mut code, KeyCode::Char(part.chars().next().unwrap()))?;
                }
                part => return Err(format!("unknown key binding component: {part}")),
            }
        }
        let code = code.ok_or_else(|| format!("key binding has no key: {value}"))?;
        Ok(Self { modifiers, code })
    }
}

fn set_code(current: &mut Option<KeyCode>, code: KeyCode) -> Result<(), String> {
    if current.replace(code).is_some() {
        return Err("key binding must contain exactly one key".into());
    }
    Ok(())
}

impl Serialize for KeyBinding {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for KeyBinding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::KeyBinding;

    #[test]
    fn key_binding_round_trips() {
        for value in ["ctrl+r", "alt+enter", "ctrl+shift+g", "pageup"] {
            let binding: KeyBinding = value.parse().unwrap();
            assert_eq!(binding.to_string(), value);
        }
    }

    #[test]
    fn key_binding_rejects_missing_or_multiple_keys() {
        assert!("ctrl".parse::<KeyBinding>().is_err());
        assert!("ctrl+r+g".parse::<KeyBinding>().is_err());
    }
}
