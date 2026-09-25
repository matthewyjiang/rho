//! `/side` / `/btw` overlay: a frozen-context aside that does not write back.

mod command;
mod overlay;
mod snapshot;

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{layout::Rect, DefaultTerminal};

use super::{
    commands,
    line_editor::LineEditor,
    panel_pointer::{PanelPointer, PanelPointerEffect, PanelPointerEvent},
    App, CommandId, CommandInvocation, ComposerMode, Entry,
};
use crate::app::side_chat::{spawn_side_chat, SideChatEvent, SideChatHandle, SideChatLaunch};
use crate::config::Config;
use rho_sdk::{
    model::{ContentBlock, Message},
    SessionId,
};

use command::{side_command_action, SideCommandAction};
use overlay::{side_overlay_frame, side_scroll_metrics, SideOverlay};
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

    pub(super) fn side_overlay_frame(
        &self,
        area: Rect,
    ) -> Option<super::overlay_panel::OverlayPanelFrame> {
        let side = self.side_chat.as_ref()?;
        side_overlay_frame(&side.overlay, area).map(|(frame, _)| frame)
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
        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Esc) => {
                self.close_side_chat();
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                self.submit_side_composer();
            }
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                side.overlay.composer.backspace();
            }
            (KeyModifiers::NONE, KeyCode::Left) => {
                side.overlay.composer.move_cursor_left();
            }
            (KeyModifiers::NONE, KeyCode::Right) => {
                side.overlay.composer.move_cursor_right();
            }
            (KeyModifiers::NONE, KeyCode::Home) => {
                side.overlay.composer.move_cursor_home();
            }
            (KeyModifiers::NONE, KeyCode::End) => {
                side.overlay.composer.move_cursor_end();
            }
            (KeyModifiers::NONE, KeyCode::Up) if side.overlay.composer.is_empty() => {
                self.scroll_side_overlay(terminal, -1);
            }
            (KeyModifiers::NONE, KeyCode::Down) if side.overlay.composer.is_empty() => {
                self.scroll_side_overlay(terminal, 1);
            }
            (_, KeyCode::PageUp) => {
                self.scroll_side_overlay(terminal, -8);
            }
            (_, KeyCode::PageDown) => {
                self.scroll_side_overlay(terminal, 8);
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
                side.overlay.composer.insert_char(ch);
            }
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if side.overlay.composer.is_empty() {
                    side.cancel();
                } else {
                    side.overlay.composer.clear();
                }
            }
            _ => {}
        }
        true
    }

    fn submit_side_composer(&mut self) {
        let Some(text) = self.side_composer_mut().map(LineEditor::take_value) else {
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

    fn side_composer_mut(&mut self) -> Option<&mut LineEditor> {
        self.side_chat
            .as_mut()
            .map(|side| &mut side.overlay.composer)
    }

    #[cfg(test)]
    pub(super) fn side_composer_is_empty(&self) -> bool {
        self.side_chat
            .as_ref()
            .is_some_and(|side| side.overlay.composer.is_empty())
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
    /// hidden behind it. Left-button and wheel input follow the shared panel
    /// pointer (scrollbar drag, drag-to-copy, copy targets); right-click pastes.
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
        let Some((frame, metrics)) = side_overlay_frame(&side.overlay, screen) else {
            return;
        };
        match side.overlay.pointer.handle(event, column, row, &frame) {
            PanelPointerEffect::None => {}
            PanelPointerEffect::ScrollTo(line) => side.overlay.scroll_to(line, &metrics),
            PanelPointerEffect::ScrollBy(delta) => side.overlay.scroll_by(delta, &metrics),
            PanelPointerEffect::Copy(text) => self.copy_text(&text, now),
        }
    }

    pub(super) fn insert_side_paste(&mut self, text: &str) -> bool {
        if !self.side_overlay_open() {
            return false;
        }
        if let Some(composer) = self.side_composer_mut() {
            composer.insert_text(text);
        }
        true
    }
}
