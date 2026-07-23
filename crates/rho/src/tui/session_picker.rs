use super::{PickerAction, PickerItem, UiPicker};
use crate::session::SessionSummary;

pub(super) fn session_picker(
    sessions: Vec<SessionSummary>,
    current_session_id: Option<&str>,
) -> UiPicker {
    UiPicker::new(
        "resume session",
        "type regex filter, tab complete, up/down select, enter confirm, esc cancel",
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn picker_uses_first_user_message_when_title_is_missing() {
        let id = "bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee".to_string();
        let picker = session_picker(
            vec![SessionSummary {
                id,
                path: PathBuf::from("session.jsonl"),
                cwd: PathBuf::from("/tmp/project"),
                created_at: 10,
                updated_at: 20,
                message_count: 3,
                title: None,
                first_user_message: Some("implement interactive resume".into()),
                last_user_message: Some("latest follow up".into()),
            }],
            None,
        );

        let item = picker.selected_item().unwrap();
        assert_eq!(item.label, "implement interactive resume");
        assert!(!item.label.contains("3 msgs"));
        assert!(item.detail.as_ref().unwrap().contains("latest follow up"));
    }

    #[test]
    fn picker_excludes_current_session() {
        let id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".to_string();
        let picker = session_picker(
            vec![SessionSummary {
                id: id.clone(),
                path: PathBuf::from("session.jsonl"),
                cwd: PathBuf::from("/tmp/project"),
                created_at: 10,
                updated_at: 20,
                message_count: 3,
                title: Some("planned feature work".into()),
                first_user_message: Some("hello from the previous session".into()),
                last_user_message: Some("hello from the previous session".into()),
            }],
            Some(&id),
        );

        assert!(picker.items.is_empty());
    }

    #[test]
    fn picker_shows_title_and_first_message_preview() {
        let id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".to_string();
        let picker = session_picker(
            vec![SessionSummary {
                id: id.clone(),
                path: PathBuf::from("session.jsonl"),
                cwd: PathBuf::from("/tmp/project"),
                created_at: 10,
                updated_at: 20,
                message_count: 3,
                title: Some("planned feature work".into()),
                first_user_message: Some("hello from the previous session".into()),
                last_user_message: Some("hello from the previous session".into()),
            }],
            None,
        );

        let item = picker.selected_item().unwrap();
        assert_eq!(item.value, id);
        assert_eq!(item.label, "planned feature work");
        assert_eq!(
            item.preview.as_deref(),
            Some("hello from the previous session")
        );
        assert!(item.detail.as_ref().unwrap().contains("id aaaaaaaa"));
    }
}
