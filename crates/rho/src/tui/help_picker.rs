use crate::keybindings::Keybindings;

use super::{
    picker_overlay::OverlayChrome, App, ComposerMode, PickerAction, PickerBadge, PickerBadgeTone,
    PickerItem, PickerLayout, UiPicker,
};

pub(super) fn help_picker(keybindings: &Keybindings) -> UiPicker {
    let items = help_items(keybindings);
    UiPicker::new("Keyboard shortcuts", items, PickerAction::Dismiss)
        .with_layout(PickerLayout::Overlay)
        .with_overlay_chrome(OverlayChrome {
            nav_label: " KEYS".into(),
            detail_label: Some(" DETAILS".into()),
            nav_keys_hint: "↑↓ keys".into(),
        })
        .with_confirm_verb("close")
}

fn help_items(keybindings: &Keybindings) -> Vec<PickerItem> {
    let reset = keybindings.reset_conversation.chrome_label();
    let editor = keybindings.open_editor.chrome_label();
    let jump = keybindings.jump_to_bottom.chrome_label();
    let toggle_tools = keybindings.toggle_tool_output.chrome_label();
    let newline = keybindings.insert_newline.chrome_label();
    let queue_prompt = keybindings.queue_prompt.chrome_label();
    let paste_image = keybindings.paste_image.chrome_label();
    let edit_pending = keybindings.edit_pending_input.chrome_label();
    let manage_pending = keybindings.manage_pending_input.chrome_label();
    let cycle_pinned = keybindings.cycle_pinned_model.chrome_label();
    let cycle_pinned_back = keybindings.cycle_pinned_model_back.chrome_label();

    vec![
        entry(
            "/",
            "Show commands",
            "Type / at the start of the composer to open the command palette. Keep typing to filter, tab to complete, and enter to run.",
        ),
        entry(
            "@",
            "Reference file",
            "Type @ to open workspace file path autocomplete. Keep typing to fuzzy-search, then tab or enter to insert an @path reference.",
        ),
        entry(
            "!",
            "Run shell",
            "Prefix a command with ! to run it in the configured inline shell. Output is shown in the transcript and included in model context.",
        ),
        entry(
            "!!",
            "Run local shell",
            "Prefix a command with !! to run it locally without adding the output to model context. The composer uses a distinct label for this mode.",
        ),
        entry(
            "Enter",
            "Send, run, steer",
            "Send the composer contents. While a model turn is running, enter queues a steering message for after the current assistant turn. In pickers, enter confirms the selection.",
        ),
        entry(
            "Esc",
            "Cancel or close",
            "Abort the current model response and restore queued work, cancel a running inline shell command, leave shell mode, or close an open palette or picker.",
        ),
        entry(
            "Shift+Tab",
            "Cycle reasoning",
            "Move through the configured reasoning levels for the active model and save the selection.",
        ),
        entry(
            cycle_pinned.clone(),
            "Cycle pinned models",
            format!(
                "Switch to the next pinned model. {cycle_pinned_back} goes backward, where the terminal reports it. Does nothing when no models are pinned. In a model picker, {cycle_pinned} pins or unpins the highlighted model."
            ),
        ),
        entry(
            "Ctrl+C",
            "Clear composer",
            "Clear the message box on the first press. While idle, press again to quit Rho.",
        ),
        entry(
            newline,
            "New line",
            format!(
                "Insert a newline in the composer without sending. Shift+Enter also inserts a newline. While idle, {queue_prompt} inserts a newline too."
            ),
        ),
        entry(
            "Shift+Enter",
            "New line",
            "Insert a newline in the composer without sending.",
        ),
        entry(
            queue_prompt.clone(),
            "Queue/newline",
            format!(
                "While idle, insert a newline. While a model turn is running, queue the current composer contents to run after the turn finishes. {} always works too, for terminals that reserve {queue_prompt} (Windows Terminal, Windows Alacritty, WezTerm).",
                Keybindings::queue_prompt_fallback().chrome_label()
            ),
        ),
        entry(
            paste_image,
            "Paste image",
            "Paste a clipboard image as an attachment when a supported host helper is available. alt+v is also accepted as a fallback.",
        ),
        entry(
            reset,
            "Reset chat",
            "Clear conversation history so the next message starts a new session. Unavailable while a model turn is running.",
        ),
        entry(
            editor,
            "External editor",
            "Open the composer contents in VISUAL, falling back to EDITOR when VISUAL is unset or empty. Warns with EDITOR is not set when neither is configured. Rho restores its screen when the editor exits.",
        ),
        entry(
            jump,
            "Jump to bottom",
            "Jump the transcript viewport back to the live bottom and resume following new output. Also available from the on-screen jump control when scrolled up.",
        ),
        entry(
            toggle_tools.clone(),
            "Toggle tools",
            format!(
                "Expand or collapse the latest tool output block when output is truncated. In a model picker, {toggle_tools} switches the list between all models and pinned models."
            ),
        ),
        entry(
            edit_pending,
            "Edit queued",
            "Pull the most recent queued or steering prompt back into the composer for editing.",
        ),
        entry(
            manage_pending,
            "Manage queue",
            "Open the pending-input panel to inspect, reorder, or remove queued prompts.",
        ),
        entry(
            "Up/Down",
            "History or nav",
            "In the composer, re-enter previous prompts. In pickers and palettes, move the selection.",
        ),
        entry(
            "PgUp/PgDn",
            "Scroll view",
            "Scroll the transcript viewport. In overlay pickers, page through the focused nav or detail pane.",
        ),
        entry(
            "mouse wheel",
            "Scroll view",
            "Scroll the transcript viewport with the mouse wheel.",
        ),
        entry(
            "click + drag",
            "Copy text",
            "Left-click and drag across transcript text to select it. Releasing copies the selection to the clipboard.",
        ),
    ]
}

fn entry(keys: impl Into<String>, summary: &str, detail: impl Into<String>) -> PickerItem {
    let keys = keys.into();
    PickerItem {
        section: None,
        label: keys.clone(),
        detail: Some(detail.into()),
        preview: None,
        badge: Some(PickerBadge {
            text: summary.into(),
            tone: PickerBadgeTone::Selected,
        }),
        value: keys,
        selection_verb: None,
    }
}

impl App {
    pub(super) fn execute_help_command(&mut self) -> anyhow::Result<()> {
        self.input_ui.set_composer(ComposerMode::Picker(help_picker(
            &self.info.runtime.keybindings,
        )));
        self.set_status("keyboard shortcuts");
        Ok(())
    }
}
