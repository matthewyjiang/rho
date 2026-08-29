//! `/side` / `/btw` overlay: a frozen-context aside that does not write back.

mod command;
mod overlay;
mod snapshot;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{layout::Rect, DefaultTerminal};

use super::{
    commands, overlay_panel::overlay_panel_layout, App, CommandId, CommandInvocation, ComposerMode,
    Entry,
};
use crate::app::side_chat::{spawn_side_chat, SideChatEvent, SideChatHandle, SideChatLaunch};
use crate::config::Config;
use rho_sdk::{
    model::{ContentBlock, Message},
    SessionId,
};

use command::{side_command_action, SideCommandAction};
use overlay::{side_overlay_frame, SideOverlay};
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
            SideChatEvent::Failed(message) => self.overlay.push_error(message),
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
    }

    pub(super) fn discard_side_chat(&mut self) {
        self.close_side_chat();
        self.side_chat = None;
    }

    fn submit_side_prompt(&mut self, prompt: String) {
        let Some(side) = self.side_chat.as_ref() else {
            return;
        };
        if side.overlay.busy {
            if let Some(side) = self.side_chat.as_mut() {
                side.overlay
                    .push_error("could not start side chat: a turn is already running".into());
            }
            return;
        }
        if side.handle.is_none() {
            let snapshot = side.overlay.snapshot.clone();
            let launch = SideChatLaunch {
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
            };
            if let Some(side) = self.side_chat.as_mut() {
                side.handle = Some(spawn_side_chat(launch));
            }
        }
        if let Some(side) = self.side_chat.as_mut() {
            side.overlay.busy = true;
            side.overlay.push_user(prompt.clone());
            if let Some(handle) = &side.handle {
                handle.submit(prompt);
            }
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
        side_overlay_frame(&side.overlay, area)
    }

    pub(super) fn handle_side_chat_key(
        &mut self,
        key: KeyEvent,
        terminal: &DefaultTerminal,
    ) -> bool {
        if !self.side_overlay_open() {
            return false;
        }
        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Esc) => {
                if self
                    .side_chat
                    .as_ref()
                    .is_some_and(|side| side.overlay.busy)
                {
                    if let Some(handle) = self
                        .side_chat
                        .as_ref()
                        .and_then(|side| side.handle.as_ref())
                    {
                        handle.cancel();
                    }
                } else {
                    self.close_side_chat();
                }
                true
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                self.submit_side_composer();
                true
            }
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                self.side_composer_mut()
                    .map(overlay::SideComposer::backspace);
                true
            }
            (KeyModifiers::NONE, KeyCode::Left) => {
                self.side_composer_mut()
                    .map(overlay::SideComposer::move_left);
                true
            }
            (KeyModifiers::NONE, KeyCode::Right) => {
                self.side_composer_mut()
                    .map(overlay::SideComposer::move_right);
                true
            }
            (KeyModifiers::NONE, KeyCode::Home) => {
                self.side_composer_mut()
                    .map(overlay::SideComposer::move_home);
                true
            }
            (KeyModifiers::NONE, KeyCode::End) => {
                self.side_composer_mut()
                    .map(overlay::SideComposer::move_end);
                true
            }
            (KeyModifiers::NONE, KeyCode::Up) | (KeyModifiers::NONE, KeyCode::Char('k'))
                if self.side_composer_is_empty() =>
            {
                self.scroll_side_overlay(terminal, -1);
                true
            }
            (KeyModifiers::NONE, KeyCode::Down) | (KeyModifiers::NONE, KeyCode::Char('j'))
                if self.side_composer_is_empty() =>
            {
                self.scroll_side_overlay(terminal, 1);
                true
            }
            (_, KeyCode::PageUp) => {
                self.scroll_side_overlay(terminal, -8);
                true
            }
            (_, KeyCode::PageDown) => {
                self.scroll_side_overlay(terminal, 8);
                true
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(ch)) => {
                self.side_composer_mut()
                    .map(|composer| composer.insert_char(ch));
                true
            }
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if let Some(composer) = self.side_composer_mut() {
                    if !composer.is_empty() {
                        composer.clear();
                        return true;
                    }
                }
                false
            }
            _ => true,
        }
    }

    fn submit_side_composer(&mut self) {
        let Some(text) = self
            .side_composer_mut()
            .map(overlay::SideComposer::take_text)
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
                    SideCommandAction::ToggleClose => {
                        self.close_side_chat();
                        return;
                    }
                    SideCommandAction::Submit(prompt) => {
                        self.submit_side_prompt(prompt);
                        return;
                    }
                    SideCommandAction::Open => {
                        self.close_side_chat();
                        return;
                    }
                }
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
            .unwrap_or_else(SessionId::new)
    }

    fn side_composer_mut(&mut self) -> Option<&mut overlay::SideComposer> {
        self.side_chat
            .as_mut()
            .map(|side| &mut side.overlay.composer)
    }

    fn side_composer_is_empty(&self) -> bool {
        self.side_chat
            .as_ref()
            .is_some_and(|side| side.overlay.composer.is_empty())
    }

    fn scroll_side_overlay(&mut self, terminal: &DefaultTerminal, delta: isize) {
        let Ok(size) = terminal.size() else {
            return;
        };
        let area = Rect::new(0, 0, size.width, size.height);
        let layout = overlay_panel_layout(area, 32);
        if let Some(side) = self.side_chat.as_mut() {
            let body_len = side.overlay.entries.len().saturating_add(4);
            side.overlay.scroll_by(delta, layout.body_rows, body_len);
        }
    }

    pub(super) fn scroll_side_overlay_wheel(
        &mut self,
        width: u16,
        height: u16,
        delta: isize,
    ) -> bool {
        if !self.side_overlay_open() {
            return false;
        }
        let area = Rect::new(0, 0, width, height);
        let layout = overlay_panel_layout(area, 32);
        if let Some(side) = self.side_chat.as_mut() {
            let body_len = side.overlay.entries.len().saturating_add(4);
            side.overlay.scroll_by(delta, layout.body_rows, body_len);
        }
        true
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
