use std::{fmt, str::FromStr};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Configurable keyboard shortcuts used by the main TUI composer.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Keybindings {
    /// Starts a new session like `/new`. Unbound by default: a single chord
    /// that drops the conversation is too easy to hit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_conversation: Option<KeyBinding>,
    /// Opens a searchable picker over prompt history, like a shell's Ctrl+R.
    pub search_prompt_history: KeyBinding,
    pub open_editor: KeyBinding,
    pub jump_to_bottom: KeyBinding,
    pub toggle_tool_output: KeyBinding,
    /// Cycles cosmetic text streaming while idle or during a turn.
    pub cycle_streaming_mode: KeyBinding,
    /// Cycles permission modes for this session, queueing during a turn.
    pub cycle_permission_mode: KeyBinding,
    pub insert_newline: KeyBinding,
    /// Queues the composer contents as a follow-up while a turn is running.
    /// Ctrl+Enter is always accepted as a fallback because Windows Terminal,
    /// Windows Alacritty, and WezTerm bind Alt+Enter to fullscreen by default.
    pub queue_prompt: KeyBinding,
    pub paste_image: KeyBinding,
    pub edit_pending_input: KeyBinding,
    pub manage_pending_input: KeyBinding,
    /// Also pins/unpins the highlighted row while a model picker is open.
    pub cycle_pinned_model: KeyBinding,
    /// Needs a terminal that reports ctrl+shift (kitty keyboard protocol).
    pub cycle_pinned_model_back: KeyBinding,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            reset_conversation: None,
            search_prompt_history: KeyBinding::control('r'),
            open_editor: KeyBinding::control('g'),
            jump_to_bottom: KeyBinding::control_code(KeyCode::End),
            toggle_tool_output: KeyBinding::control('o'),
            cycle_streaming_mode: KeyBinding::alt(KeyCode::Char('s')),
            cycle_permission_mode: KeyBinding::alt(KeyCode::Char('m')),
            insert_newline: KeyBinding::control('j'),
            queue_prompt: KeyBinding::alt(KeyCode::Enter),
            paste_image: KeyBinding::control('v'),
            edit_pending_input: KeyBinding::alt(KeyCode::Up),
            manage_pending_input: KeyBinding::alt(KeyCode::Char('q')),
            cycle_pinned_model: KeyBinding::control('p'),
            cycle_pinned_model_back: KeyBinding::control_shift('p'),
        }
    }
}

impl Keybindings {
    /// Ctrl+Enter fallback for terminals that steal Alt+Enter (Windows Terminal,
    /// Windows Alacritty, WezTerm fullscreen). Always accepted in addition to
    /// the configured `queue_prompt` chord.
    pub fn queue_prompt_fallback() -> KeyBinding {
        KeyBinding::control_code(KeyCode::Enter)
    }

    pub fn reset_conversation_matches(&self, event: KeyEvent) -> bool {
        self.reset_conversation
            .as_ref()
            .is_some_and(|binding| binding.matches(event))
    }

    pub fn queue_prompt_matches(&self, event: KeyEvent) -> bool {
        self.queue_prompt.matches(event) || Self::queue_prompt_fallback().matches(event)
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PartialKeybindings {
    reset_conversation: Option<KeyBinding>,
    search_prompt_history: Option<KeyBinding>,
    open_editor: Option<KeyBinding>,
    jump_to_bottom: Option<KeyBinding>,
    toggle_tool_output: Option<KeyBinding>,
    cycle_streaming_mode: Option<KeyBinding>,
    cycle_permission_mode: Option<KeyBinding>,
    insert_newline: Option<KeyBinding>,
    queue_prompt: Option<KeyBinding>,
    paste_image: Option<KeyBinding>,
    edit_pending_input: Option<KeyBinding>,
    manage_pending_input: Option<KeyBinding>,
    cycle_pinned_model: Option<KeyBinding>,
    cycle_pinned_model_back: Option<KeyBinding>,
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
        // Rho saves every binding, so a config written before history search
        // carries the old `reset_conversation = "ctrl+r"` default. Drop it so
        // Ctrl+R reaches history search.
        let legacy_reset_shortcut = KeyBinding::control('r');
        let migrate_legacy_reset = partial.search_prompt_history.is_none()
            && partial.reset_conversation.as_ref() == Some(&legacy_reset_shortcut);
        let defaults = Self::default();
        let keybindings = Self {
            reset_conversation: partial.reset_conversation.filter(|_| !migrate_legacy_reset),
            search_prompt_history: partial
                .search_prompt_history
                .unwrap_or(defaults.search_prompt_history),
            open_editor: partial.open_editor.unwrap_or(defaults.open_editor),
            jump_to_bottom: if migrate_legacy_jump {
                defaults.jump_to_bottom
            } else {
                partial.jump_to_bottom.unwrap_or(defaults.jump_to_bottom)
            },
            toggle_tool_output: partial
                .toggle_tool_output
                .unwrap_or(defaults.toggle_tool_output),
            cycle_streaming_mode: partial
                .cycle_streaming_mode
                .unwrap_or(defaults.cycle_streaming_mode),
            cycle_permission_mode: partial
                .cycle_permission_mode
                .unwrap_or(defaults.cycle_permission_mode),
            insert_newline: partial.insert_newline.unwrap_or(defaults.insert_newline),
            queue_prompt: partial.queue_prompt.unwrap_or(defaults.queue_prompt),
            paste_image: partial.paste_image.unwrap_or(defaults.paste_image),
            edit_pending_input: partial
                .edit_pending_input
                .unwrap_or(defaults.edit_pending_input),
            manage_pending_input: partial
                .manage_pending_input
                .unwrap_or(defaults.manage_pending_input),
            cycle_pinned_model: partial
                .cycle_pinned_model
                .unwrap_or(defaults.cycle_pinned_model),
            cycle_pinned_model_back: partial
                .cycle_pinned_model_back
                .unwrap_or(defaults.cycle_pinned_model_back),
        };
        if keybindings.open_editor == keybindings.jump_to_bottom {
            return Err(serde::de::Error::custom(
                "open_editor and jump_to_bottom must use different keys",
            ));
        }
        if keybindings.reset_conversation.as_ref() == Some(&keybindings.search_prompt_history) {
            return Err(serde::de::Error::custom(
                "reset_conversation and search_prompt_history must use different keys",
            ));
        }
        for (name, binding) in [
            (
                "reset_conversation",
                keybindings.reset_conversation.as_ref(),
            ),
            (
                "search_prompt_history",
                Some(&keybindings.search_prompt_history),
            ),
            ("open_editor", Some(&keybindings.open_editor)),
            ("jump_to_bottom", Some(&keybindings.jump_to_bottom)),
            ("toggle_tool_output", Some(&keybindings.toggle_tool_output)),
            (
                "cycle_streaming_mode",
                Some(&keybindings.cycle_streaming_mode),
            ),
            ("insert_newline", Some(&keybindings.insert_newline)),
            ("queue_prompt", Some(&keybindings.queue_prompt)),
            ("paste_image", Some(&keybindings.paste_image)),
            ("edit_pending_input", Some(&keybindings.edit_pending_input)),
            (
                "manage_pending_input",
                Some(&keybindings.manage_pending_input),
            ),
            ("cycle_pinned_model", Some(&keybindings.cycle_pinned_model)),
            (
                "cycle_pinned_model_back",
                Some(&keybindings.cycle_pinned_model_back),
            ),
        ] {
            if binding == Some(&keybindings.cycle_permission_mode) {
                return Err(serde::de::Error::custom(format!(
                    "cycle_permission_mode and {name} must use different keys"
                )));
            }
        }
        let binding = &keybindings.cycle_permission_mode;
        if let Some(reserved) =
            ReservedComposerKey::from_key(KeyEvent::new(binding.code, binding.modifiers))
        {
            return Err(serde::de::Error::custom(format!(
                "cycle_permission_mode = \"{binding}\" conflicts with a reserved composer key: {}",
                reserved.reason()
            )));
        }
        Ok(keybindings)
    }
}

/// Fixed composer key classes that a configurable shortcut must not shadow.
/// Shared with dispatch so modifier-insensitive handlers stay in sync with validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReservedComposerKey {
    ReasoningCycle,
    TabCompletion,
    HistoryPageUp,
    HistoryPageDown,
    TextInput(char),
    QueuePromptFallback,
    PasteImageFallback,
}

impl ReservedComposerKey {
    pub(crate) fn from_key(key: KeyEvent) -> Option<Self> {
        match (key.modifiers, key.code) {
            (_, KeyCode::BackTab) => Some(Self::ReasoningCycle),
            (modifiers, KeyCode::Tab) if modifiers.contains(KeyModifiers::SHIFT) => {
                Some(Self::ReasoningCycle)
            }
            (_, KeyCode::Tab) => Some(Self::TabCompletion),
            (_, KeyCode::PageUp) => Some(Self::HistoryPageUp),
            (_, KeyCode::PageDown) => Some(Self::HistoryPageDown),
            (modifiers, KeyCode::Char(ch))
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                Some(Self::TextInput(ch))
            }
            _ if Keybindings::queue_prompt_fallback().matches(key) => {
                Some(Self::QueuePromptFallback)
            }
            (KeyModifiers::ALT, KeyCode::Char('v' | 'V')) => Some(Self::PasteImageFallback),
            _ => None,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::ReasoningCycle => "Shift+Tab and BackTab cycle reasoning",
            Self::TabCompletion => "Tab is reserved for composer completion",
            Self::HistoryPageUp | Self::HistoryPageDown => {
                "PageUp and PageDown scroll history regardless of modifiers"
            }
            Self::TextInput(_) => "characters without Ctrl or Alt are composer text input",
            Self::QueuePromptFallback => "Ctrl+Enter is the queue_prompt fallback",
            Self::PasteImageFallback => "Alt+V is the paste_image fallback",
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

    const fn control_shift(ch: char) -> Self {
        Self {
            modifiers: KeyModifiers::CONTROL.union(KeyModifiers::SHIFT),
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

    /// Capitalised form for picker footers, row details, and `/help` key labels (`Ctrl+P`).
    ///
    /// [`Display`] stays lowercase (`ctrl+p`) so config files keep the parseable spelling.
    pub fn chrome_label(&self) -> String {
        self.to_string()
            .split('+')
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join("+")
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
#[path = "permission_keybinding_tests.rs"]
mod permission_tests;

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{KeyBinding, Keybindings};

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

    // Covers: configs saved before history search keep their persisted
    // `reset_conversation = "ctrl+r"`, which must not shadow history search,
    // while deliberate reset bindings survive and collisions fail loudly.
    // Owner: keybinding config parsing.
    #[test]
    fn reset_binding_migrates_off_ctrl_r() {
        let ctrl_r: KeyBinding = "ctrl+r".parse().unwrap();
        let alt_r: KeyBinding = "alt+r".parse().unwrap();
        let alt_h: KeyBinding = "alt+h".parse().unwrap();
        let cases = [
            (
                "legacy saved default",
                r#"reset_conversation = "ctrl+r""#,
                Ok((None, ctrl_r.clone())),
            ),
            (
                "deliberate reset key",
                r#"reset_conversation = "alt+r""#,
                Ok((Some(alt_r), ctrl_r.clone())),
            ),
            (
                "ctrl+r reset after moving search",
                "reset_conversation = \"ctrl+r\"\nsearch_prompt_history = \"alt+h\"",
                Ok((Some(ctrl_r.clone()), alt_h)),
            ),
            (
                "collision",
                "reset_conversation = \"ctrl+r\"\nsearch_prompt_history = \"ctrl+r\"",
                Err(()),
            ),
        ];
        for (case, text, expected) in cases {
            let parsed = toml::from_str::<Keybindings>(text)
                .map(|keys| (keys.reset_conversation, keys.search_prompt_history))
                .map_err(|_| ());
            assert_eq!(parsed, expected, "{case}");
        }
    }

    #[test]
    fn saved_defaults_reload_unchanged() {
        let saved = toml::to_string(&Keybindings::default()).unwrap();
        assert_eq!(
            toml::from_str::<Keybindings>(&saved).unwrap(),
            Keybindings::default()
        );
    }

    #[test]
    fn queue_prompt_matches_configured_chord_and_ctrl_enter_fallback() {
        let keys = Keybindings::default();
        let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
        let ctrl_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL);
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        assert!(keys.queue_prompt_matches(alt_enter));
        assert!(keys.queue_prompt_matches(ctrl_enter));
        assert!(!keys.queue_prompt_matches(enter));

        let remapped = Keybindings {
            queue_prompt: "ctrl+k".parse().unwrap(),
            ..Keybindings::default()
        };
        let ctrl_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert!(remapped.queue_prompt_matches(ctrl_k));
        assert!(remapped.queue_prompt_matches(ctrl_enter));
        assert!(!remapped.queue_prompt_matches(alt_enter));
    }
}
