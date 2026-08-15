//! Standalone overlay used by `rho attach` when no run id is given.

use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{widgets::Clear, DefaultTerminal, Frame};

use crate::subagent;

use super::super::{
    attach_picker::{self, AttachCandidate},
    picker_overlay::picker_overlay_frame,
    Theme, UiPicker,
};

pub(super) async fn select_running_run(
    terminal: &mut DefaultTerminal,
) -> anyhow::Result<Option<String>> {
    let candidates = tokio::task::spawn_blocking(subagent::list_running_runs).await??;
    if candidates.is_empty() {
        anyhow::bail!("no running subagents");
    }
    let candidates = candidates
        .into_iter()
        .map(AttachCandidate::from)
        .collect::<Vec<_>>();
    let mut picker = attach_picker::picker(&candidates);
    loop {
        terminal.draw(|frame| draw_picker(frame, &picker))?;
        let Some(event) = next_event().await? else {
            continue;
        };
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Esc => return Ok(None),
                KeyCode::Enter => {
                    return Ok(picker.selected_item().map(|item| item.value.clone()));
                }
                KeyCode::Up => picker.select_previous(),
                KeyCode::Down => picker.select_next(),
                KeyCode::Backspace => picker.pop_filter_char(),
                KeyCode::Char(ch) => picker.push_filter_char(ch),
                _ => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

fn draw_picker(frame: &mut Frame<'_>, picker: &UiPicker) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    if let Some(overlay) = picker_overlay_frame(picker, area) {
        frame.render_widget(
            ratatui::widgets::Paragraph::new(overlay.lines).style(Theme::text()),
            overlay.outer,
        );
        frame.set_cursor_position(overlay.cursor);
    }
}

async fn next_event() -> anyhow::Result<Option<Event>> {
    Ok(tokio::task::spawn_blocking(|| {
        if event::poll(Duration::from_millis(100))? {
            event::read().map(Some)
        } else {
            Ok(None)
        }
    })
    .await??)
}
