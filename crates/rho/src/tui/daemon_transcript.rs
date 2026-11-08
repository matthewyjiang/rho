//! Shows a daemon session's [`DaemonView`]s through the TUI's own transcript
//! paths (#1667): history entries and the live tool batch, fed the same
//! view-model events an in-process turn feeds them. Also holds the composer
//! gate for prompts, which daemon sessions do not take from the TUI yet.

use super::{
    daemon_events::DaemonView, event_adapter::ViewModelEvent, App, DefaultTerminal, Entry,
};

impl App {
    /// Applies what the followed daemon session changed.
    pub(super) fn show_daemon_views(
        &mut self,
        views: Vec<DaemonView>,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        for view in views {
            match view {
                DaemonView::Clear => {
                    self.set_history_entries(Vec::new());
                    self.turn.clear_tool_calls();
                    self.set_todo_list(None);
                }
                DaemonView::User(text) => self.insert_entry(&Entry::User(text)),
                DaemonView::Text {
                    kind,
                    text,
                    opens: true,
                } => self.open_transcript_row(kind.entry(text)),
                DaemonView::Text {
                    kind,
                    text,
                    opens: false,
                } => self.push_transcript_entry(kind.entry(text)),
                DaemonView::ToolRunning { call, card } => {
                    self.handle_agent_event(
                        ViewModelEvent::ToolStarted {
                            call_id: call,
                            card,
                        },
                        terminal,
                    )?;
                }
                DaemonView::ToolFinished { call, presentation } => {
                    self.handle_agent_event(
                        ViewModelEvent::ToolFinished {
                            call_id: call,
                            presentation,
                            // Images a call returned stay in its wire
                            // content, which cards do not show yet.
                            image_asset: None,
                        },
                        terminal,
                    )?;
                }
                DaemonView::Plan(list) => {
                    self.set_todo_list(list);
                }
                DaemonView::Context {
                    used_tokens,
                    window_tokens,
                } => {
                    self.handle_agent_event(
                        ViewModelEvent::ContextUsage(
                            rho_sdk::model::ContextUsage::provider_reported(
                                used_tokens,
                                window_tokens,
                            ),
                        ),
                        terminal,
                    )?;
                }
                DaemonView::Status(text) => self.set_status(text),
                DaemonView::Notice(text) => self.insert_entry(&Entry::Notice(text)),
                DaemonView::Error(text) => self.insert_entry(&Entry::Error(text)),
            }
        }
        Ok(())
    }

    /// Appends `entry` as a row of its own, even after a row of its kind
    /// that it would otherwise extend: a new message starts a new row.
    fn open_transcript_row(&mut self, entry: Entry) {
        let index = self.history.len();
        self.history.lines_mut().invalidate_from(index);
        self.history.push(entry);
        self.mark_markdown_images_dirty_from(index);
    }

    /// Whether the composer keeps a prompt it would send, because this TUI
    /// follows a daemon session, which takes prompts from the TUI only once
    /// T1c lands. Says so on the status line; the composer keeps the text.
    pub(super) fn holds_prompt_for_daemon(&mut self) -> bool {
        if self.info.session.daemon.is_none() {
            return false;
        }
        self.notify_status("sending prompts to a daemon session is not supported yet");
        true
    }
}
