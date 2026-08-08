//! In-app `/sessions` hub: browse, resume, and delete saved sessions from
//! every directory, grouped by the directory they belong to.

use std::path::{Path, PathBuf};

use ratatui::DefaultTerminal;

use super::{
    picker_overlay::OverlayChrome, session_picker, statusline::path::compact_cwd, App,
    ComposerMode, Entry, InlineChoice, InlineChoiceModal, InlineChoiceOption, InlineChoicePending,
    InteractiveRuntime, PickerAction, PickerBadge, PickerBadgeTone, PickerItem, PickerKeyHints,
    PickerLayout, Session, SessionDeleteReopen, UiPicker,
};
use crate::session::{is_cross_project, DeleteOptions, SessionSummary};

const DIRECTORY_PREFIX: &str = "dir:";
const SESSION_PREFIX: &str = "session:";

/// Sessions of one workspace directory, newest first, plus its display path.
pub(super) struct DirectoryGroup {
    pub(super) cwd: PathBuf,
    pub(super) display: String,
    pub(super) sessions: Vec<SessionSummary>,
}

/// Groups sessions by directory, keeping the input's newest-first order both
/// across groups and inside each group. The current directory always sorts
/// first.
pub(super) fn directory_groups(
    sessions: Vec<SessionSummary>,
    current_cwd: &Path,
) -> Vec<DirectoryGroup> {
    let mut groups: Vec<DirectoryGroup> = Vec::new();
    for session in sessions {
        match groups.iter_mut().find(|group| group.cwd == session.cwd) {
            Some(group) => group.sessions.push(session),
            None => groups.push(DirectoryGroup {
                display: compact_cwd(&session.cwd),
                cwd: session.cwd.clone(),
                sessions: vec![session],
            }),
        }
    }
    if let Some(position) = groups
        .iter()
        .position(|group| !is_cross_project(&group.cwd, current_cwd))
    {
        let current = groups.remove(position);
        groups.insert(0, current);
    }
    groups
}

fn count_label(count: usize) -> String {
    if count == 1 {
        "1 session".to_string()
    } else {
        format!("{count} sessions")
    }
}

fn directory_row(group: &DirectoryGroup, current_cwd: &Path, now: u64) -> PickerItem {
    let is_current = !is_cross_project(&group.cwd, current_cwd);
    let newest = group
        .sessions
        .iter()
        .map(|session| session.updated_at)
        .max()
        .unwrap_or_default();
    let counted = count_label(group.sessions.len());
    let updated = session_picker::format_updated_ago(newest, now);
    PickerItem {
        section: Some(group.display.clone()),
        label: format!("All sessions · {}", group.sessions.len()),
        detail: Some(format!(
            "{}\n{counted} · newest {updated}\nEnter shows only this directory. Press d to delete every session here.",
            group.display
        )),
        preview: None,
        badge: is_current.then(|| PickerBadge {
            text: "current dir".into(),
            tone: PickerBadgeTone::Selected,
        }),
        value: format!("{DIRECTORY_PREFIX}{}", group.cwd.display()),
        selection_verb: Some("browse"),
    }
}

fn session_row(
    section: Option<&str>,
    session: &SessionSummary,
    current_session_id: Option<&str>,
    now: u64,
) -> PickerItem {
    let is_current = current_session_id == Some(session.id.as_str());
    let short_id = session_picker::short_session_id(&session.id);
    let first_user_preview = session
        .first_user_message
        .as_deref()
        .map(session_picker::preview_text);
    let title = session
        .title
        .as_deref()
        .map(session_picker::preview_text)
        .or_else(|| first_user_preview.clone())
        .unwrap_or_else(|| format!("session {short_id}"));
    let preview = session
        .title
        .as_ref()
        .and(first_user_preview.clone())
        .filter(|preview| preview != &title);
    let updated = session_picker::format_updated_ago(session.updated_at, now);
    let mut detail = format!("updated {updated} · id {short_id}");
    if let Some(last_user) = session
        .last_user_message
        .as_deref()
        .map(session_picker::preview_text)
        .filter(|last_user| last_user != &title)
    {
        detail.push_str(&format!("\nlast: {last_user}"));
    }
    detail.push_str(if is_current {
        "\nThis is the current session."
    } else {
        "\nEnter resumes this session. Press d to delete."
    });
    PickerItem {
        section: section.map(str::to_owned),
        label: title,
        detail: Some(detail),
        preview,
        badge: is_current.then(|| PickerBadge {
            text: "current".into(),
            tone: PickerBadgeTone::Selected,
        }),
        value: format!("{SESSION_PREFIX}{}", session.id),
        selection_verb: Some(if is_current { "close" } else { "resume" }),
    }
}

fn manage_sessions_picker(title: impl Into<String>, items: Vec<PickerItem>) -> UiPicker {
    UiPicker::new(title, items, PickerAction::ManageSessions)
        .with_key_hints(PickerKeyHints {
            pin_toggle: false,
            tab_complete: false,
            row_delete: true,
        })
        .with_layout(PickerLayout::Overlay)
        .with_overlay_chrome(OverlayChrome {
            nav_label: " SESSIONS".into(),
            detail_label: Some(" DETAILS".into()),
            nav_keys_hint: "↑↓ items".into(),
        })
}

/// Root list: every directory with its sessions, current directory first.
pub(super) fn hub_picker(
    groups: &[DirectoryGroup],
    current_session_id: Option<&str>,
    current_cwd: &Path,
    now: u64,
) -> UiPicker {
    let mut items = Vec::new();
    for group in groups {
        items.push(directory_row(group, current_cwd, now));
        for session in &group.sessions {
            items.push(session_row(
                Some(group.display.as_str()),
                session,
                current_session_id,
                now,
            ));
        }
    }
    manage_sessions_picker("sessions", items)
}

/// Child list scoped to one directory's sessions.
pub(super) fn directory_picker(
    group: &DirectoryGroup,
    current_session_id: Option<&str>,
    now: u64,
) -> UiPicker {
    let items = group
        .sessions
        .iter()
        .map(|session| session_row(None, session, current_session_id, now))
        .collect();
    manage_sessions_picker(group.display.clone(), items)
}

impl App {
    pub(super) fn execute_sessions_command(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        // The first listing can parse many transcripts to build the session
        // index, so show progress before the synchronous work.
        self.set_status("loading sessions");
        terminal.draw(|frame| self.draw(frame))?;
        self.open_sessions_hub_or_report();
        Ok(())
    }

    pub(super) fn open_sessions_hub_or_report(&mut self) {
        if let Err(error) = self.open_sessions_hub() {
            self.input_ui.set_composer(ComposerMode::Input);
            self.insert_entry(&Entry::Error(format!("could not open sessions: {error}")));
            self.set_status("sessions failed");
        }
    }

    pub(super) fn open_sessions_hub(&mut self) -> anyhow::Result<()> {
        let sessions = Session::list_all()?;
        if sessions.is_empty() {
            self.input_ui.set_composer(ComposerMode::Input);
            self.set_status("no saved sessions");
            return Ok(());
        }
        let groups = directory_groups(sessions, &self.info.runtime.cwd);
        let picker = hub_picker(
            &groups,
            self.info.session.session_id.as_deref(),
            &self.info.runtime.cwd,
            session_picker::now_unix_secs(),
        );
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("sessions");
        Ok(())
    }

    pub(super) async fn submit_sessions_selection(
        &mut self,
        value: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if let Some(session_id) = value.strip_prefix(SESSION_PREFIX) {
            if self.info.session.session_id.as_deref() == Some(session_id) {
                self.input_ui.set_composer(ComposerMode::Input);
                self.set_status("already in this session");
                return Ok(());
            }
            return self
                .submit_resume_selection(session_id, terminal, agent)
                .await;
        }
        if let Some(dir) = value.strip_prefix(DIRECTORY_PREFIX) {
            return self.open_directory_sessions(Path::new(dir));
        }
        self.input_ui.set_composer(ComposerMode::Input);
        self.insert_entry(&Entry::Error(format!(
            "unknown sessions selection '{value}'"
        )));
        self.set_status("sessions selection failed");
        Ok(())
    }

    fn open_directory_sessions(&mut self, cwd: &Path) -> anyhow::Result<()> {
        let sessions = match Session::list(cwd) {
            Ok(sessions) => sessions,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not list sessions: {error}")));
                self.set_status("sessions failed");
                return Ok(());
            }
        };
        if sessions.is_empty() {
            self.set_status("no saved sessions for this directory");
            return Ok(());
        }
        let group = DirectoryGroup {
            display: compact_cwd(cwd),
            cwd: cwd.to_path_buf(),
            sessions,
        };
        let child = directory_picker(
            &group,
            self.info.session.session_id.as_deref(),
            session_picker::now_unix_secs(),
        );
        self.open_child_picker(child);
        Ok(())
    }

    pub(super) fn prompt_delete_selected_sessions_item(&mut self) -> anyhow::Result<()> {
        let Some(value) = self.selected_sessions_item_value() else {
            return Ok(());
        };
        if let Some(session_id) = value.strip_prefix(SESSION_PREFIX) {
            if self.info.session.session_id.as_deref() == Some(session_id) {
                self.set_status("cannot delete the current session");
                return Ok(());
            }
            return self
                .prompt_delete_session(session_id.to_owned(), SessionDeleteReopen::SessionsHub);
        }
        if let Some(dir) = value.strip_prefix(DIRECTORY_PREFIX) {
            return self.prompt_delete_directory_sessions(Path::new(dir));
        }
        Ok(())
    }

    fn prompt_delete_directory_sessions(&mut self, cwd: &Path) -> anyhow::Result<()> {
        let count = Session::list(cwd)
            .map(|sessions| sessions.len())
            .unwrap_or(0);
        let display = compact_cwd(cwd);
        let choice = InlineChoice::new(
            format!("Delete all sessions in {display}?"),
            format!(
                "Removes {} saved in this directory, with transcripts, web sidecars, and parent-linked subagent runs. The current session is kept. Usage history is kept.",
                count_label(count)
            ),
            vec![
                InlineChoiceOption::available(
                    "delete",
                    'd',
                    "Delete all",
                    "Permanently remove every saved session in this directory",
                ),
                InlineChoiceOption::available(
                    "cancel",
                    'c',
                    "Cancel",
                    "Keep the sessions and return to the picker",
                )
                .with_alternate_shortcut('n'),
            ],
        )?;
        self.input_ui
            .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                choice,
                pending: InlineChoicePending::DeleteDirectorySessions {
                    cwd: cwd.to_path_buf(),
                },
            }));
        self.set_status("confirm delete directory sessions");
        Ok(())
    }

    pub(super) fn submit_delete_directory_sessions_choice(
        &mut self,
        value: &str,
        cwd: &Path,
    ) -> anyhow::Result<()> {
        if value != "delete" {
            self.open_sessions_hub_or_report();
            return Ok(());
        }

        let display = compact_cwd(cwd);
        let sessions = match Session::list(cwd) {
            Ok(sessions) => sessions,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not list sessions: {error}")));
                self.open_sessions_hub_or_report();
                self.set_status("delete failed");
                return Ok(());
            }
        };

        let current_session_id = self.info.session.session_id.clone();
        let mut deleted = 0usize;
        let mut kept_current = false;
        let mut failures = Vec::new();
        for session in sessions {
            if current_session_id.as_deref() == Some(session.id.as_str()) {
                kept_current = true;
                continue;
            }
            match Session::delete_by_id(
                cwd,
                &session.id,
                DeleteOptions {
                    force: false,
                    protect_session_id: current_session_id.clone(),
                },
            ) {
                Ok(_) => deleted += 1,
                Err(error) => failures.push(error),
            }
        }

        let mut notice = format!("deleted {} in {display}", count_label(deleted));
        if kept_current {
            notice.push_str(", kept the current session");
        }
        if !failures.is_empty() {
            notice.push_str(&format!(", {} failed", failures.len()));
            for error in &failures {
                self.insert_entry(&Entry::Error(format!("could not delete session: {error}")));
            }
        }
        self.open_sessions_hub_or_report();
        self.set_status(notice);
        Ok(())
    }

    fn selected_sessions_item_value(&self) -> Option<String> {
        match self.input_ui.composer() {
            ComposerMode::Picker(picker) if picker.action == PickerAction::ManageSessions => {
                picker.selected_item().map(|item| item.value.clone())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "sessions_hub_tests.rs"]
mod tests;
