use crate::keybindings::Keybindings;

use super::{
    picker_overlay::OverlayChrome, App, ComposerMode, PickerAction, PickerItem, PickerLayout,
    UiPicker,
};

pub(super) fn help_picker(keybindings: &Keybindings) -> UiPicker {
    let items = help_items(keybindings);
    UiPicker::new(
        "Keyboard shortcuts",
        "type regex filter, enter or esc closes",
        items,
        PickerAction::Dismiss,
    )
    .with_layout(PickerLayout::Overlay)
    .with_overlay_chrome(OverlayChrome {
        nav_label: " KEYS".into(),
        detail_label: Some(" DETAILS".into()),
        nav_keys_hint: "↑↓ keys".into(),
    })
    .with_confirm_verb("close")
}

fn help_items(keybindings: &Keybindings) -> Vec<PickerItem> {
    let reset = keybindings.reset_conversation.to_string();
    let editor = keybindings.open_editor.to_string();
    let jump = keybindings.jump_to_bottom.to_string();
    let toggle_tools = keybindings.toggle_tool_output.to_string();
    let newline = keybindings.insert_newline.to_string();
    let paste_image = keybindings.paste_image.to_string();
    let edit_pending = keybindings.edit_pending_input.to_string();
    let manage_pending = keybindings.manage_pending_input.to_string();

    vec![
        entry(
            "/",
            "Show available commands",
            "Type / at the start of the composer to open the command palette. Keep typing to filter, tab to complete, and enter to run.",
        ),
        entry(
            "@",
            "Reference a workspace file",
            "Type @ to open workspace file path autocomplete. Keep typing to fuzzy-search, then tab or enter to insert an @path reference.",
        ),
        entry(
            "!",
            "Run a shell command",
            "Prefix a command with ! to run it in the configured inline shell. Output is shown in the transcript and included in model context.",
        ),
        entry(
            "!!",
            "Run a local shell command",
            "Prefix a command with !! to run it locally without adding the output to model context. The composer uses a distinct label for this mode.",
        ),
        entry(
            "enter",
            "Send, run, or steer",
            "Send the composer contents. While a model turn is running, enter queues a steering message for after the current assistant turn. In pickers, enter confirms the selection.",
        ),
        entry(
            "esc",
            "Cancel or close",
            "Abort the current model response and restore queued work, cancel a running inline shell command, leave shell mode, or close an open palette or picker.",
        ),
        entry(
            "shift+tab",
            "Cycle reasoning level",
            "Move through the configured reasoning levels for the active model and save the selection.",
        ),
        entry(
            "ctrl+c",
            "Clear the composer",
            "Clear the message box on the first press. While idle, press again to quit Rho.",
        ),
        entry(
            newline,
            "Insert a newline",
            "Insert a newline in the composer without sending. shift+enter also inserts a newline. While idle, alt+enter inserts a newline too.",
        ),
        entry(
            "shift+enter",
            "Insert a newline",
            "Insert a newline in the composer without sending.",
        ),
        entry(
            "alt+enter",
            "Newline or queue prompt",
            "While idle, insert a newline. While a model turn is running, queue the current composer contents to run after the turn finishes.",
        ),
        entry(
            paste_image,
            "Paste an image",
            "Paste a clipboard image as an attachment when a supported host helper is available. alt+v is also accepted as a fallback.",
        ),
        entry(
            reset,
            "Reset conversation",
            "Clear conversation history so the next message starts a new session. Unavailable while a model turn is running.",
        ),
        entry(
            editor,
            "Edit composer in $EDITOR",
            "Open the composer contents in the program set by EDITOR. Rho restores its screen when the editor exits.",
        ),
        entry(
            jump,
            "Jump to bottom",
            "Jump the transcript viewport back to the live bottom and resume following new output. Also available from the on-screen jump control when scrolled up.",
        ),
        entry(
            toggle_tools,
            "Toggle tool output",
            "Expand or collapse the latest tool output block when output is truncated.",
        ),
        entry(
            edit_pending,
            "Edit queued prompt",
            "Pull the most recent queued or steering prompt back into the composer for editing.",
        ),
        entry(
            manage_pending,
            "Manage queued prompts",
            "Open the pending-input panel to inspect, reorder, or remove queued prompts.",
        ),
        entry(
            "up / down",
            "Prompt history or picker nav",
            "In the composer, re-enter previous prompts. In pickers and palettes, move the selection.",
        ),
        entry(
            "pageup / pagedown",
            "Scroll transcript or picker",
            "Scroll the transcript viewport. In overlay pickers, page through the focused nav or detail pane.",
        ),
        entry(
            "mouse wheel",
            "Scroll transcript",
            "Scroll the transcript viewport with the mouse wheel.",
        ),
        entry(
            "click + drag",
            "Copy transcript text",
            "Left-click and drag across transcript text to select it. Releasing copies the selection to the clipboard.",
        ),
    ]
}

fn entry(keys: impl Into<String>, summary: &str, detail: &str) -> PickerItem {
    let keys = keys.into();
    PickerItem {
        section: None,
        label: keys.clone(),
        detail: Some(format!("{summary}\n\n{detail}")),
        preview: None,
        badge: None,
        value: keys,
    }
}

impl App {
    pub(super) fn execute_help_command(&mut self) -> anyhow::Result<()> {
        self.input_ui.composer = ComposerMode::Picker(help_picker(&self.info.runtime.keybindings));
        self.status = "keyboard shortcuts".into();
        Ok(())
    }
}

#[cfg(test)]
#[path = "help_picker_tests.rs"]
mod tests;
