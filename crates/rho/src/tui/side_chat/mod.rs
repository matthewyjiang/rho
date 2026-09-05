//! `/side` / `/btw` overlay: a frozen-context aside that does not write back.

mod command;
mod overlay;
mod snapshot;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{layout::Rect, DefaultTerminal};

use super::{
    commands, line_editor::LineEditor, App, CommandId, CommandInvocation, ComposerMode, Entry,
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
                if let Some(composer) = self.side_composer_mut() {
                    composer.backspace();
                }
                true
            }
            (KeyModifiers::NONE, KeyCode::Left) => {
                if let Some(composer) = self.side_composer_mut() {
                    composer.move_cursor_left();
                }
                true
            }
            (KeyModifiers::NONE, KeyCode::Right) => {
                if let Some(composer) = self.side_composer_mut() {
                    composer.move_cursor_right();
                }
                true
            }
            (KeyModifiers::NONE, KeyCode::Home) => {
                if let Some(composer) = self.side_composer_mut() {
                    composer.move_cursor_home();
                }
                true
            }
            (KeyModifiers::NONE, KeyCode::End) => {
                if let Some(composer) = self.side_composer_mut() {
                    composer.move_cursor_end();
                }
                true
            }
            (KeyModifiers::NONE, KeyCode::Up) if self.side_composer_is_empty() => {
                self.scroll_side_overlay(terminal, -1);
                true
            }
            (KeyModifiers::NONE, KeyCode::Down) if self.side_composer_is_empty() => {
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
                if let Some(composer) = self.side_composer_mut() {
                    composer.insert_char(ch);
                }
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

    pub(super) fn side_composer_is_empty(&self) -> bool {
        self.side_chat
            .as_ref()
            .is_some_and(|side| side.overlay.composer.is_empty())
    }

    fn scroll_side_overlay(&mut self, terminal: &DefaultTerminal, delta: isize) {
        let Ok(size) = terminal.size() else {
            return;
        };
        self.scroll_side_overlay_area(Rect::new(0, 0, size.width, size.height), delta);
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
        self.scroll_side_overlay_area(Rect::new(0, 0, width, height), delta);
        true
    }

    fn scroll_side_overlay_area(&mut self, area: Rect, delta: isize) {
        let Some(side) = self.side_chat.as_mut() else {
            return;
        };
        let Some(metrics) = side_scroll_metrics(&side.overlay, area) else {
            return;
        };
        side.overlay.scroll_by(delta, &metrics);
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
