//! `/side` / `/btw` overlay: a frozen-context aside that does not write back.

mod command;
mod composer;
mod overlay;
mod snapshot;

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{layout::Rect, DefaultTerminal};

use super::{
    commands,
    composer_buffer::ComposerEditKey,
    panel_pointer::{PanelPointer, PanelPointerEffect, PanelPointerEvent},
    App, CommandId, CommandInvocation, ComposerMode, Entry, HistoryDirection,
};
use crate::app::side_chat::{spawn_side_chat, SideChatEvent, SideChatHandle, SideChatLaunch};
use crate::config::Config;
use rho_sdk::{
    model::{ContentBlock, Message},
    SessionId,
};

use command::{side_command_action, SideCommandAction};
use composer::SideEditFollowUp;
use overlay::{side_overlay_frame, side_scroll_metrics, SideOverlay, SideOverlayFrame};
use snapshot::frozen_parent_snapshot;

pub(super) struct SideChat {
    overlay: SideOverlay,
    handle: Option<SideChatHandle>,
}

impl SideChat {
    fn new(snapshot: String) -> Self {
        Self {
            overlay: SideOverlay::new(snapshot),
            handle: None,
        }
    }

    fn apply(&mut self, event: SideChatEvent) {
        match event {
            SideChatEvent::AssistantDelta(text) => self.overlay.append_assistant_delta(&text),
            SideChatEvent::AssistantReset => self.overlay.reset_assistant_stream(),
            SideChatEvent::ToolStarted(name) => self.overlay.push_tool(name),
            SideChatEvent::Finished => self.overlay.finish_assistant(),
            SideChatEvent::Cancelled => self.overlay.mark_cancelled(),
            SideChatEvent::Failed(message) => self.overlay.fail_run(message),
            SideChatEvent::Rejected(message) => self.overlay.push_notice(message),
        }
    }

    fn reject_if_busy(&mut self) -> bool {
        if !self.overlay.busy {
            return false;
        }
        self.overlay
            .push_notice("could not start side chat: a turn is already running".into());
        true
    }

    fn cancel(&self) {
        if !self.overlay.busy {
            return;
        }
        if let Some(handle) = &self.handle {
            handle.cancel();
        }
    }

    fn submit(&mut self, prompt: String, launch: Option<SideChatLaunch>) {
        if self.reject_if_busy() {
            return;
        }
        self.overlay.busy = true;
        if let Some(launch) = launch {
            self.handle = Some(spawn_side_chat(launch));
        }
        self.overlay.push_user(prompt.clone());
        if let Some(handle) = &self.handle {
            handle.submit(prompt);
        }
    }
}

impl App {
    pub(super) fn side_overlay_open(&self) -> bool {
        matches!(self.input_ui.composer(), ComposerMode::Side)
    }

    pub(super) fn side_chat_busy(&self) -> bool {
        self.side_chat
            .as_ref()
            .is_some_and(|side| side.overlay.busy)
    }

    pub(super) async fn execute_side_command(
        &mut self,
        invocation: CommandInvocation,
    ) -> anyhow::Result<()> {
        match side_command_action(self.side_overlay_open(), &invocation.args) {
            SideCommandAction::ToggleClose => self.close_side_chat(),
            SideCommandAction::Open => self.open_side_chat(),
            SideCommandAction::Submit(prompt) => {
                if !self.side_overlay_open() {
                    self.open_side_chat();
                }
                self.submit_side_prompt(prompt);
            }
        }
        Ok(())
    }

    pub(super) fn open_side_chat(&mut self) {
        if self.side_chat.is_none() {
            self.side_chat = Some(SideChat::new(self.frozen_parent_snapshot()));
        }
        self.input_ui.set_composer(ComposerMode::Side);
        self.set_status("side chat");
    }

    pub(super) fn close_side_chat(&mut self) {
        if self.side_overlay_open() {
            self.input_ui.set_composer(ComposerMode::Input);
        }
        // The aside outlives the overlay; its hover and selection do not.
        if let Some(side) = self.side_chat.as_mut() {
            side.overlay.pointer = PanelPointer::default();
            side.overlay.composer.settle_pointer();
        }
    }

    pub(super) fn discard_side_chat(&mut self) {
        self.close_side_chat();
        self.side_chat = None;
    }

    fn submit_side_prompt(&mut self, prompt: String) {
        let (busy, needs_spawn, snapshot) = {
            let Some(side) = self.side_chat.as_ref() else {
                return;
            };
            (
                side.overlay.busy,
                side.handle.is_none(),
                side.overlay.snapshot.clone(),
            )
        };
        if busy {
            if let Some(side) = self.side_chat.as_mut() {
                side.reject_if_busy();
            }
            return;
        }
        let launch = needs_spawn.then(|| self.side_chat_launch(snapshot));
        if let Some(side) = self.side_chat.as_mut() {
            side.submit(prompt, launch);
        }
    }

    fn side_chat_launch(&self, snapshot: String) -> SideChatLaunch {
        SideChatLaunch {
            config: self.side_chat_config(),
            config_path: self
                .info
                .services
                .config_repository
                .configured_path()
                .unwrap_or_else(|_| self.info.runtime.cwd.join(".rho").join("config.toml")),
            cwd: self.info.runtime.cwd.clone(),
            parent_session_id: self.side_chat_parent_session_id(),
            snapshot,
        }
    }

    pub(super) fn poll_side_chat(&mut self) -> bool {
        let Some(side) = self.side_chat.as_mut() else {
            return false;
        };
        let mut changed = false;
        while let Some(event) = side.handle.as_mut().and_then(SideChatHandle::try_recv) {
            side.apply(event);
            changed = true;
        }
        changed
    }

    /// Prepare the painted overlay and retain its composer row window and
    /// wrap width. Pointer and scroll queries use the read-only projection
    /// instead, so a speculative frame never moves the window.
    pub(super) fn prepare_side_overlay_for_paint(
        &mut self,
        area: Rect,
    ) -> Option<super::overlay_panel::OverlayPanelFrame> {
        let side = self.side_chat.as_mut()?;
        let prepared = side_overlay_frame(&side.overlay, area)?;
        let composer = &mut side.overlay.composer;
        composer.buffer.set_view_start(prepared.composer.view_start);
        composer.set_painted_width(prepared.composer.text_width);
        Some(prepared.frame)
    }

    /// Pointer state of the side overlay, for painting hover and selection.
    pub(super) fn side_overlay_pointer(&self) -> Option<PanelPointer> {
        self.side_chat.as_ref().map(|side| side.overlay.pointer)
    }

    pub(super) fn handle_side_chat_key(
        &mut self,
        key: KeyEvent,
        terminal: &DefaultTerminal,
    ) -> bool {
        if !self.side_overlay_open() {
            return false;
        }
        let Some(side) = self.side_chat.as_mut() else {
            return true;
        };
        side.overlay.composer.cancel_click_sequence();
        let keybindings = &self.info.runtime.keybindings;
        // Idle, the main composer's queue chord inserts a newline; an aside
        // has nothing to queue behind, so it always does.
        let newline_chord =
            keybindings.insert_newline.matches(key) || keybindings.queue_prompt_matches(key);
        if newline_chord {
            side.overlay.composer.apply_edit(ComposerEditKey::Newline);
            side.overlay.reveal_composer();
            self.input_ui.clear_paste_burst();
            return true;
        }
        if let Some(edit) = ComposerEditKey::from_key(key) {
            if edit == ComposerEditKey::Newline {
                self.input_ui.clear_paste_burst();
            }
            match side.overlay.composer.apply_edit(edit) {
                SideEditFollowUp::None => side.overlay.reveal_composer(),
                SideEditFollowUp::ScrollTranscript(HistoryDirection::Previous) => {
                    self.scroll_side_overlay(terminal, -1);
                }
                SideEditFollowUp::ScrollTranscript(HistoryDirection::Next) => {
                    self.scroll_side_overlay(terminal, 1);
                }
            }
            return true;
        }
        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Esc) => {
                self.close_side_chat();
            }
            (_, KeyCode::Enter) => {
                self.submit_side_composer();
            }
            (_, KeyCode::PageUp) => {
                self.scroll_side_overlay(terminal, -8);
            }
            (_, KeyCode::PageDown) => {
                self.scroll_side_overlay(terminal, 8);
            }
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if side.overlay.composer.buffer.is_empty() {
                    side.cancel();
                } else {
                    side.overlay.composer.clear();
                }
            }
            _ => self.input_ui.clear_paste_burst(),
        }
        true
    }

    fn submit_side_composer(&mut self) {
        let Some(text) = self
            .side_chat
            .as_mut()
            .map(|side| side.overlay.composer.take_submission())
        else {
            return;
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Ok(Some(invocation)) = commands::parse_command(&text) {
            if invocation.id == CommandId::Side {
                match side_command_action(/*overlay_open*/ true, &invocation.args) {
                    SideCommandAction::Open | SideCommandAction::ToggleClose => {
                        self.close_side_chat();
                    }
                    SideCommandAction::Submit(prompt) => self.submit_side_prompt(prompt),
                }
                return;
            }
        }
        self.submit_side_prompt(text);
    }

    fn frozen_parent_snapshot(&self) -> String {
        let mut messages = Vec::new();
        for entry in self.history.entries() {
            match entry {
                Entry::User(text) if !text.is_empty() => {
                    messages.push(Message::User(vec![ContentBlock::Text(text.clone())]));
                }
                Entry::Assistant(assistant) if !assistant.text.is_empty() => {
                    messages.push(Message::Assistant(vec![ContentBlock::Text(
                        assistant.text.clone(),
                    )]));
                }
                _ => {}
            }
        }
        let live = self.streams.assistant_stream.emitted_text();
        if !live.is_empty() {
            messages.push(Message::Assistant(vec![ContentBlock::Text(
                live.to_string(),
            )]));
        }
        frozen_parent_snapshot(&messages)
    }

    fn side_chat_config(&self) -> Config {
        let mut config = self
            .info
            .services
            .config_repository
            .load()
            .unwrap_or_default();
        config.provider.clone_from(&self.info.runtime.provider);
        config.model.clone_from(&self.info.runtime.model);
        config.auth.clone_from(&self.info.runtime.auth);
        config.reasoning = self.info.runtime.reasoning;
        config
    }

    fn side_chat_parent_session_id(&self) -> SessionId {
        self.info
            .session
            .session_id
            .as_deref()
            .and_then(|id| id.parse().ok())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(super) fn side_composer_is_empty(&self) -> bool {
        self.side_chat
            .as_ref()
            .is_some_and(|side| side.overlay.composer.buffer.is_empty())
    }

    fn scroll_side_overlay(&mut self, terminal: &DefaultTerminal, delta: isize) {
        let Ok(size) = terminal.size() else {
            return;
        };
        let area = Rect::new(0, 0, size.width, size.height);
        let Some(side) = self.side_chat.as_mut() else {
            return;
        };
        if let Some(metrics) = side_scroll_metrics(&side.overlay, area) {
            side.overlay.scroll_by(delta, &metrics);
        }
    }

    /// Pointer input while the side overlay is open. The overlay owns every
    /// event so clicks, drags, and releases never reach transcript controls
    /// hidden behind it. Left presses on the composer place the caret, select
    /// words on double click, and drag-select its text. Other left-button and
    /// wheel input follow the shared panel pointer (scrollbar drag,
    /// drag-to-copy, copy targets); right-click pastes.
    /// Motion needs no work here: paint resolves hover from the app's last
    /// pointer cell.
    pub(super) fn handle_side_overlay_mouse(
        &mut self,
        kind: MouseEventKind,
        screen: Rect,
        column: u16,
        row: u16,
        now: Instant,
    ) {
        self.clear_selections();
        self.clear_hovered_copy_buttons();
        self.clear_rail_pointer_state();
        self.history.set_scrollbar_drag(None);
        if kind == MouseEventKind::Down(MouseButton::Right) {
            self.paste_clipboard_text();
            return;
        }
        // Building the frame renders the whole transcript; skip it for motion.
        let Some(event) = PanelPointerEvent::from_kind(kind) else {
            return;
        };
        let Some(side) = self.side_chat.as_mut() else {
            return;
        };
        // Hit-test against the frame the user sees, then mutate the overlay.
        // The pointer never changes the body, so the frame's metrics stay
        // valid for the scroll it asks for.
        let Some(prepared) = side_overlay_frame(&side.overlay, screen) else {
            return;
        };
        // The composer owns presses on its rows and the drag they start,
        // like the main composer: place the caret or select text instead
        // of starting a transcript selection.
        let composer = &mut side.overlay.composer;
        match kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = prepared.composer_index_at(composer, column, row, false) {
                    side.overlay.pointer.clear_selection();
                    self.input_ui.clear_transient_edit_state();
                    composer.pointer_press(index, now, column, row);
                    return;
                }
                composer.cancel_pointer();
            }
            MouseEventKind::Drag(MouseButton::Left) if composer.buffer.selection_dragging() => {
                if let Some(index) = prepared.composer_index_at(composer, column, row, true) {
                    composer.pointer_drag(index);
                }
                return;
            }
            MouseEventKind::Up(MouseButton::Left) if composer.buffer.selection_dragging() => {
                let index = prepared.composer_index_at(composer, column, row, true);
                composer.pointer_release(index);
                return;
            }
            _ => {}
        }
        let SideOverlayFrame { frame, metrics, .. } = prepared;
        match side.overlay.pointer.handle(event, column, row, &frame) {
            PanelPointerEffect::None => {}
            PanelPointerEffect::ScrollTo(line) => side.overlay.scroll_to(line, &metrics),
            PanelPointerEffect::ScrollBy(delta) => side.overlay.scroll_by(delta, &metrics),
            PanelPointerEffect::Copy(text) => self.copy_text(&text, now),
        }
    }

    /// Expanded side composer text while the overlay is open, for the
    /// external editor.
    pub(super) fn side_composer_text(&self) -> Option<String> {
        if !self.side_overlay_open() {
            return None;
        }
        self.side_chat
            .as_ref()
            .map(|side| side.overlay.composer.buffer.expanded_text())
    }

    pub(super) fn replace_side_composer_text(&mut self, text: String) {
        if let Some(side) = self.side_chat.as_mut() {
            side.overlay.composer.replace_text(text);
            side.overlay.reveal_composer();
        }
    }

    /// A paste burst's Enter: a newline in the draft, never a submit.
    pub(super) fn insert_side_paste_newline(&mut self) {
        if let Some(side) = self.side_chat.as_mut() {
            side.overlay.composer.apply_edit(ComposerEditKey::Newline);
            side.overlay.reveal_composer();
        }
    }

    /// Focus loss ends any side composer drag without a release.
    pub(super) fn settle_side_composer_pointer(&mut self) {
        if let Some(side) = self.side_chat.as_mut() {
            side.overlay.composer.settle_pointer();
        }
    }

    pub(super) fn insert_side_paste(&mut self, text: &str) -> bool {
        if !self.side_overlay_open() {
            return false;
        }
        let collapsed = self.side_chat.as_mut().and_then(|side| {
            side.overlay.reveal_composer();
            side.overlay.composer.insert_paste(text)
        });
        // A collapsed paste hides its content behind a marker; confirm it
        // like the main composer does.
        if let Some(paste) = collapsed {
            self.notify_status(paste.toast());
        }
        true
    }
}
