use std::time::{SystemTime, UNIX_EPOCH};

use super::{PickerAction, PickerItem, UiPicker};
use crate::session::SessionSummary;

pub(super) fn session_picker(
    sessions: Vec<SessionSummary>,
    current_session_id: Option<&str>,
) -> UiPicker {
    let now = now_unix_secs();
    UiPicker::new(
        "resume session",
        sessions
            .into_iter()
            .filter(|session| current_session_id != Some(session.id.as_str()))
            .map(|session| session_item(session, now))
            .collect(),
        PickerAction::ResumeSession,
    )
    .with_key_hints(super::PickerKeyHints {
        tab_complete: true,
        row_delete: true,
        ..Default::default()
    })
    .with_confirm_verb("resume")
}

fn session_item(session: SessionSummary, now: u64) -> PickerItem {
    let short_id = short_session_id(&session.id);
    let first_user_preview = session.first_user_message.as_deref().map(preview_text);
    let title = session
        .title
        .as_deref()
        .map(preview_text)
        .or_else(|| first_user_preview.clone())
        .unwrap_or_else(|| format!("session {short_id}"));
    let preview = session
        .title
        .as_ref()
        .and(first_user_preview)
        .filter(|preview| preview != &title);
    let last_user = session.last_user_message.as_deref().map(preview_text);
    let updated = format_updated_ago(session.updated_at, now);
    let detail = match last_user {
        Some(last_user) if last_user != title => {
            format!("updated {updated} · last: {last_user} · id {short_id}")
        }
        Some(_) | None => format!("updated {updated} · id {short_id}"),
    };
    PickerItem {
        section: None,
        label: title,
        detail: Some(detail),
        preview,
        badge: None,
        value: session.id,
        selection_verb: None,
    }
}

pub(super) fn short_session_id(id: &str) -> String {
    id.chars().take(8).collect()
}

pub(super) fn preview_text(text: &str) -> String {
    let text = text.replace('\n', " ");
    if text.chars().count() <= 80 {
        return text;
    }
    let mut preview = text.chars().take(79).collect::<String>();
    preview.push('…');
    preview
}

pub(super) fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// Relative age for resume-picker recency, e.g. `2h ago`.
pub(super) fn format_updated_ago(updated_at: u64, now: u64) -> String {
    let age_secs = now.saturating_sub(updated_at);
    if age_secs < 60 {
        return format!("{age_secs}s ago");
    }
    let minutes = age_secs / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 48 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    format!("{days}d ago")
}

#[cfg(test)]
#[path = "session_picker_tests.rs"]
mod tests;
