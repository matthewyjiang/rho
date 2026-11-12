//! Settings the interactive TUI owns.
//!
//! `~/.rho/clients/tui.toml` holds the `tui` client's settings, declared in
//! [`TUI_SETTINGS`] with the same mechanics as the agent registry. Following a
//! daemon session, the TUI registers them with the daemon, which stores them
//! there and serves them back (see `daemon_settings`). `~/.rho/tui.toml` holds
//! the theme and `[keybindings]` chords, which stay hand-edited. Missing files
//! are the defaults.

use std::{collections::BTreeMap, path::Path};

use rho_protocol::{SettingChange, SettingEntry, SettingKey, SettingSpec};

use serde::Deserialize;

use crate::config::{
    any_value, load_settings, unknown_keys, write_changes, AnySetting, Apply, ConfigDocument,
    ConfigError, IntegerSpec, Setting, SettingMeta, StreamingMode,
};
use crate::keybindings::{EditingMode, Keybindings};

/// Sent composer prompts kept by default.
pub(crate) const DEFAULT_PROMPT_HISTORY_LIMIT: usize = 1000;
/// Ceiling for `prompt_history_limit`; the 2.x loader clamped to it.
pub(crate) const MAX_PROMPT_HISTORY_LIMIT: usize = 10_000;

/// Everything the TUI reads from its two files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TuiSettings {
    /// Hide tool cards, reasoning, and activity chrome so only message text remains.
    pub(crate) zen_mode: bool,
    pub(crate) show_reasoning_output: bool,
    /// Reveal received text live, by paragraph, or at response completion.
    pub(crate) output_streaming: StreamingMode,
    /// Show the keyboard hint block in the session header. Signed-out
    /// sessions keep their `/login` hints regardless.
    pub(crate) show_header_hints: bool,
    /// Notify through the terminal (OSC 9 or BEL) when a turn finishes or
    /// needs the user while the terminal is unfocused. Ignored under Herdr.
    pub(crate) notifications: bool,
    /// Show a transcript notice after a turn that re-billed a large uncached prompt.
    pub(crate) cache_miss_notices: bool,
    pub(crate) max_tool_output_lines: usize,
    /// Sent composer prompts kept in the shared prompt-history database.
    /// `0` disables persistence.
    pub(crate) prompt_history_limit: usize,
    pub(crate) check_for_updates: bool,
    /// Color theme id (`terminal`, built-in, or custom file stem), from `tui.toml`.
    pub(crate) theme: String,
    /// Chords from `tui.toml`; `editing_mode` comes from the client file.
    pub(crate) keybindings: Keybindings,
}

impl Default for TuiSettings {
    fn default() -> Self {
        Self {
            zen_mode: false,
            show_reasoning_output: true,
            output_streaming: StreamingMode::default(),
            show_header_hints: true,
            notifications: true,
            cache_miss_notices: false,
            max_tool_output_lines: 10,
            prompt_history_limit: DEFAULT_PROMPT_HISTORY_LIMIT,
            check_for_updates: true,
            theme: "terminal".into(),
            keybindings: Keybindings::default(),
        }
    }
}

impl TuiSettings {
    /// Reads `clients/tui.toml` and `tui.toml` under `rho_dir`, reporting
    /// every bad key in either.
    pub(crate) fn load(rho_dir: &Path) -> anyhow::Result<Self> {
        let mut settings = Self::default();

        let local_path = rho_dir.join("tui.toml");
        let local = match std::fs::read_to_string(&local_path) {
            Ok(text) => toml::from_str::<TuiLocal>(&text).map_err(|error| {
                anyhow::anyhow!(
                    "invalid {}: {}",
                    local_path.display(),
                    error.to_string().trim_end()
                )
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => TuiLocal::default(),
            Err(error) => {
                return Err(anyhow::Error::new(error)
                    .context(format!("could not read {}", local_path.display())))
            }
        };
        if let Some(theme) = local.theme {
            if theme.trim().is_empty() {
                anyhow::bail!("invalid {}: `theme` is empty", local_path.display());
            }
            settings.theme = theme.trim().to_owned();
        }
        if let Some(keybindings) = local.keybindings {
            settings.keybindings = keybindings;
        }

        let client_path = rho_dir.join("clients").join("tui.toml");
        let doc = ConfigDocument::read(&client_path)?;
        let mut errors = unknown_keys(&doc, TUI_SETTINGS, &[]);
        load_settings(TUI_SETTINGS, &doc, &mut settings, &mut errors);
        if !errors.is_empty() {
            anyhow::bail!(
                "invalid {}:\n{}",
                client_path.display(),
                errors
                    .iter()
                    .map(ConfigError::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        Ok(settings)
    }
}

impl TuiSettings {
    /// These settings with every `tui` client setting taken from `values`,
    /// as the daemon serves them; the theme and chords stay. A value the
    /// codec refuses keeps this one's.
    pub(crate) fn with_client_values(&self, values: &BTreeMap<SettingKey, SettingEntry>) -> Self {
        let changes = values
            .iter()
            .filter_map(|(key, entry)| {
                Some(SettingChange::Set {
                    key: key.clone(),
                    value: entry.value.clone()?,
                })
            })
            .collect::<Vec<_>>();
        let mut doc = ConfigDocument::default();
        // A refused change leaves its key out of `doc`, so it keeps this
        // one's value below.
        let _ = write_changes(&mut doc, &tui_setting_specs(), &changes);
        let mut settings = self.clone();
        load_settings(TUI_SETTINGS, &doc, &mut settings, &mut Vec::new());
        settings
    }
}

/// The `tui` setting the streaming shortcut changes without saving.
pub(super) const OUTPUT_STREAMING_KEY: &str = "output_streaming";

/// The `tui` setting whose lowering deletes saved prompts.
pub(super) const PROMPT_HISTORY_LIMIT_KEY: &str = "prompt_history_limit";

/// The `tui` client's settings, as the TUI registers them with the daemon.
pub(crate) fn tui_setting_specs() -> Vec<SettingSpec> {
    TUI_SETTINGS.iter().map(|setting| setting.spec()).collect()
}

/// `~/.rho/tui.toml`.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TuiLocal {
    theme: Option<String>,
    keybindings: Option<Keybindings>,
}

/// Every `tui` client setting, in schema order.
pub(crate) static TUI_SETTINGS: &[&dyn AnySetting<TuiSettings>] = &[
    &ZEN_MODE,
    &SHOW_REASONING_OUTPUT,
    &OUTPUT_STREAMING,
    &SHOW_HEADER_HINTS,
    &NOTIFICATIONS,
    &CACHE_MISS_NOTICES,
    &MAX_TOOL_OUTPUT_LINES,
    &PROMPT_HISTORY_LIMIT,
    &EDITING_MODE,
    &CHECK_FOR_UPDATES,
];

const fn meta(label: &'static str, help: &'static str) -> SettingMeta {
    SettingMeta {
        category: "display",
        section: None,
        label,
        help,
        applies: Apply::Immediately,
        overridable_per_project: false,
    }
}

static ZEN_MODE: Setting<TuiSettings, bool> = Setting {
    key: "zen_mode",
    meta: meta(
        "Zen mode",
        "Show only message text. Hides tool cards, reasoning, and the Thinking... placeholder.",
    ),
    spec: (),
    get: |settings| settings.zen_mode,
    set: |settings, enabled| settings.zen_mode = enabled,
    validate: any_value,
};

static SHOW_REASONING_OUTPUT: Setting<TuiSettings, bool> = Setting {
    key: "show_reasoning_output",
    meta: meta(
        "Show reasoning output",
        "Show model reasoning text in the transcript.",
    ),
    spec: (),
    get: |settings| settings.show_reasoning_output,
    set: |settings, shown| settings.show_reasoning_output = shown,
    validate: any_value,
};

static OUTPUT_STREAMING: Setting<TuiSettings, StreamingMode> = Setting {
    key: OUTPUT_STREAMING_KEY,
    meta: meta(
        "Output streaming",
        "Reveal text live, by paragraph, or at message boundaries.",
    ),
    spec: (),
    get: |settings| settings.output_streaming,
    set: |settings, mode| settings.output_streaming = mode,
    validate: any_value,
};

static SHOW_HEADER_HINTS: Setting<TuiSettings, bool> = Setting {
    key: "show_header_hints",
    meta: meta(
        "Header hints",
        "Show the keyboard hint block in the session header. Sign-in hints stay while signed out.",
    ),
    spec: (),
    get: |settings| settings.show_header_hints,
    set: |settings, shown| settings.show_header_hints = shown,
    validate: any_value,
};

static NOTIFICATIONS: Setting<TuiSettings, bool> = Setting {
    key: "notifications",
    meta: meta(
        "Notifications",
        "Desktop notification, or a terminal bell, when a turn finishes or needs you while the terminal is unfocused.",
    ),
    spec: (),
    get: |settings| settings.notifications,
    set: |settings, enabled| settings.notifications = enabled,
    validate: any_value,
};

static CACHE_MISS_NOTICES: Setting<TuiSettings, bool> = Setting {
    key: "cache_miss_notices",
    meta: meta(
        "Cache miss notices",
        "Show a transcript notice after a turn that re-billed a large uncached prompt.",
    ),
    spec: (),
    get: |settings| settings.cache_miss_notices,
    set: |settings, enabled| settings.cache_miss_notices = enabled,
    validate: any_value,
};

static MAX_TOOL_OUTPUT_LINES: Setting<TuiSettings, u64> = Setting {
    key: "max_tool_output_lines",
    meta: meta(
        "Max tool output lines",
        "Maximum collapsed tool output lines shown in the transcript.",
    ),
    // The 2.x loader raised anything lower to 1.
    spec: IntegerSpec {
        min: 1,
        max: None,
        unit: Some(rho_protocol::IntegerUnit::Lines),
    },
    get: |settings| settings.max_tool_output_lines as u64,
    set: |settings, lines| settings.max_tool_output_lines = lines as usize,
    validate: any_value,
};

static PROMPT_HISTORY_LIMIT: Setting<TuiSettings, u64> = Setting {
    key: PROMPT_HISTORY_LIMIT_KEY,
    meta: meta(
        "Prompt history limit",
        "Saved composer prompts kept for up-arrow recall across sessions. 0 disables saving.",
    ),
    spec: IntegerSpec {
        min: 0,
        max: Some(MAX_PROMPT_HISTORY_LIMIT as u64),
        unit: None,
    },
    get: |settings| settings.prompt_history_limit as u64,
    set: |settings, limit| settings.prompt_history_limit = limit as usize,
    validate: any_value,
};

static EDITING_MODE: Setting<TuiSettings, EditingMode> = Setting {
    key: "editing_mode",
    meta: meta(
        "Editing mode",
        "Vim adds a normal mode to the composer; Esc enters it.",
    ),
    spec: (),
    get: |settings| settings.keybindings.editing_mode,
    set: |settings, mode| settings.keybindings.editing_mode = mode,
    validate: any_value,
};

static CHECK_FOR_UPDATES: Setting<TuiSettings, bool> = Setting {
    key: "check_for_updates",
    meta: meta(
        "Check for updates",
        "Check GitHub releases at startup and show an update notice when available.",
    ),
    spec: (),
    get: |settings| settings.check_for_updates,
    set: |settings, enabled| settings.check_for_updates = enabled,
    validate: any_value,
};

#[cfg(test)]
#[path = "client_settings_tests.rs"]
mod tests;
