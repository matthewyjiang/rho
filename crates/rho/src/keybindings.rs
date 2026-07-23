use std::{fmt, str::FromStr};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Configurable keyboard shortcuts used by the main TUI composer.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Keybindings {
    pub reset_conversation: KeyBinding,
    pub open_editor: KeyBinding,
    pub jump_to_bottom: KeyBinding,
    pub toggle_tool_output: KeyBinding,
    pub insert_newline: KeyBinding,
    pub paste_image: KeyBinding,
    pub edit_pending_input: KeyBinding,
    pub manage_pending_input: KeyBinding,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            reset_conversation: KeyBinding::control('r'),
            open_editor: KeyBinding::control('g'),
            jump_to_bottom: KeyBinding::control_code(KeyCode::End),
            toggle_tool_output: KeyBinding::control('o'),
            insert_newline: KeyBinding::control('j'),
            paste_image: KeyBinding::control('v'),
            edit_pending_input: KeyBinding::alt(KeyCode::Up),
            manage_pending_input: KeyBinding::alt(KeyCode::Char('q')),
        }
    }
}

#[derive(Deserialize, Default)]
struct PartialKeybindings {
    reset_conversation: Option<KeyBinding>,
    open_editor: Option<KeyBinding>,
    jump_to_bottom: Option<KeyBinding>,
    toggle_tool_output: Option<KeyBinding>,
    insert_newline: Option<KeyBinding>,
    paste_image: Option<KeyBinding>,
    edit_pending_input: Option<KeyBinding>,
    manage_pending_input: Option<KeyBinding>,
}

impl<'de> Deserialize<'de> for Keybindings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let partial = PartialKeybindings::deserialize(deserializer)?;
        let legacy_jump_shortcut = KeyBinding::control('g');
        let migrate_legacy_jump = partial.open_editor.is_none()
            && partial.jump_to_bottom.as_ref() == Some(&legacy_jump_shortcut);
        let defaults = Self::default();
        let keybindings = Self {
            reset_conversation: partial
                .reset_conversation
                .unwrap_or(defaults.reset_conversation),
            open_editor: partial.open_editor.unwrap_or(defaults.open_editor),
            jump_to_bottom: if migrate_legacy_jump {
                defaults.jump_to_bottom
            } else {
                partial.jump_to_bottom.unwrap_or(defaults.jump_to_bottom)
            },
            toggle_tool_output: partial
                .toggle_tool_output
                .unwrap_or(defaults.toggle_tool_output),
            insert_newline: partial.insert_newline.unwrap_or(defaults.insert_newline),
            paste_image: partial.paste_image.unwrap_or(defaults.paste_image),
            edit_pending_input: partial
                .edit_pending_input
                .unwrap_or(defaults.edit_pending_input),
            manage_pending_input: partial
                .manage_pending_input
                .unwrap_or(defaults.manage_pending_input),
        };
        if keybindings.open_editor == keybindings.jump_to_bottom {
            return Err(serde::de::Error::custom(
                "open_editor and jump_to_bottom must use different keys",
            ));
        }
        Ok(keybindings)
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

    const fn control_code(code: KeyCode) -> Self {
        Self {
            modifiers: KeyModifiers::CONTROL,
            code,
        }
    }

    const fn alt(code: KeyCode) -> Self {
        Self {
            modifiers: KeyModifiers::ALT,
            code,
        }
    }

    pub fn matches(&self, event: KeyEvent) -> bool {
        self.modifiers == event.modifiers && key_codes_match(self.code, event.code)
    }
}

fn key_codes_match(configured: KeyCode, received: KeyCode) -> bool {
    match (configured, received) {
        (KeyCode::Char(configured), KeyCode::Char(received)) => {
            configured.eq_ignore_ascii_case(&received)
        }
        (configured, received) => configured == received,
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
            KeyCode::Char(ch) if self.modifiers.contains(KeyModifiers::SHIFT) => {
                ch.to_ascii_lowercase().to_string()
            }
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
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::KeyBinding;

    #[test]
    fn key_binding_round_trips() {
        for value in ["ctrl+r", "alt+enter", "ctrl+shift+g", "pageup"] {
            let binding: KeyBinding = value.parse().unwrap();
            assert_eq!(binding.to_string(), value);
        }
    }

    #[test]
    fn shifted_character_binding_matches_terminal_event() {
        let binding: KeyBinding = "ctrl+shift+g".parse().unwrap();
        let event = KeyEvent::new(
            KeyCode::Char('G'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );

        assert!(binding.matches(event));
        assert_eq!(binding.to_string(), "ctrl+shift+g");
    }

    #[test]
    fn key_binding_rejects_missing_or_multiple_keys() {
        assert!("ctrl".parse::<KeyBinding>().is_err());
        assert!("ctrl+r+g".parse::<KeyBinding>().is_err());
    }
}
