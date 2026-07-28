use super::{PickerAction, PickerItem, UiPicker};
use crate::session::SessionSummary;

pub(super) fn session_picker(
    sessions: Vec<SessionSummary>,
    current_session_id: Option<&str>,
) -> UiPicker {
    UiPicker::new(
        "resume session",
        "type regex filter, tab complete, up/down select, enter resume, d delete, esc cancel",
        sessions
            .into_iter()
            .filter(|session| current_session_id != Some(session.id.as_str()))
            .map(session_item)
            .collect(),
        PickerAction::ResumeSession,
    )
}

fn session_item(session: SessionSummary) -> PickerItem {
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
    let detail = match last_user {
        Some(last_user) if last_user != title => {
            format!(
                "updated {} · last: {last_user} · id {short_id}",
                session.updated_at
            )
        }
        Some(_) | None => format!("updated {} · id {short_id}", session.updated_at),
    };
    PickerItem {
        section: None,
        label: title,
        detail: Some(detail),
        preview,
        badge: None,
        value: session.id,
    }
}

pub(super) fn short_session_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn preview_text(text: &str) -> String {
    let text = text.replace('\n', " ");
    if text.chars().count() <= 80 {
        return text;
    }
    let mut preview = text.chars().take(79).collect::<String>();
    preview.push('…');
    preview
}

