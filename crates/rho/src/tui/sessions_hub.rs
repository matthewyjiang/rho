//! In-app `/sessions` hub: browse, resume, and delete saved sessions from
//! every directory, grouped by the directory they belong to.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use ratatui::DefaultTerminal;

use super::{
    picker_overlay::OverlayChrome, session_picker, statusline::path::compact_cwd, App,
    ComposerMode, Entry, InlineChoice, InlineChoiceModal, InlineChoiceOption, InlineChoicePending,
    InteractiveRuntime, PickerAction, PickerBadge, PickerBadgeTone, PickerItem, PickerKeyHints,
    PickerLayout, Session, UiPicker,
};
use crate::session::{is_cross_project, DeleteOptions, SessionSummary, SessionTarget};

const TARGET_PREFIX: &str = "sessions-target:";

/// Sessions of one workspace directory, newest first, plus its display path.
pub(super) struct DirectoryGroup {
    pub(super) cwd: PathBuf,
    pub(super) display: String,
    pub(super) sessions: Vec<SessionSummary>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum SessionsLocation {
    #[default]
    Root,
    Directory(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SessionsHubTarget {
    CleanupMissingWorkspaces,
    Directory(PathBuf),
    Session(SessionTarget),
}

#[derive(Debug)]
pub(super) struct SessionsPickerBuild {
    pub(super) picker: UiPicker,
    pub(super) targets: Vec<SessionsHubTarget>,
}

#[derive(Debug, Default)]
pub(super) struct SessionsHubState {
    location: SessionsLocation,
    targets: Vec<SessionsHubTarget>,
    root_targets: Vec<SessionsHubTarget>,
}

impl SessionsHubState {
    fn open_root(&mut self, targets: Vec<SessionsHubTarget>) {
        self.location = SessionsLocation::Root;
        self.root_targets.clone_from(&targets);
        self.targets = targets;
    }

    fn open_directory(&mut self, cwd: PathBuf, targets: Vec<SessionsHubTarget>) {
        self.location = SessionsLocation::Directory(cwd);
        self.targets = targets;
    }

    pub(super) fn navigate_back(&mut self) {
        self.location = SessionsLocation::Root;
        self.targets.clone_from(&self.root_targets);
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    fn target(&self, value: &str) -> Option<SessionsHubTarget> {
        let index = value.strip_prefix(TARGET_PREFIX)?.parse::<usize>().ok()?;
        self.targets.get(index).cloned()
    }

    fn location(&self) -> SessionsLocation {
        self.location.clone()
    }
}

/// Groups sessions by directory, keeping the input's newest-first order both
/// across groups and inside each group. The current directory always sorts
/// first.
pub(super) fn directory_groups(
    sessions: Vec<SessionSummary>,
    current_cwd: &Path,
) -> Vec<DirectoryGroup> {
    let mut groups: Vec<DirectoryGroup> = Vec::new();
    let mut indexes = HashMap::<PathBuf, usize>::new();
    for session in sessions {
        if let Some(index) = indexes.get(&session.cwd).copied() {
            groups[index].sessions.push(session);
            continue;
        }
        let index = groups.len();
        indexes.insert(session.cwd.clone(), index);
        groups.push(DirectoryGroup {
            display: compact_cwd(&session.cwd),
            cwd: session.cwd.clone(),
            sessions: vec![session],
        });
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

fn target_value(index: usize) -> String {
    format!("{TARGET_PREFIX}{index}")
}

fn directory_row(
    group: &DirectoryGroup,
    current_cwd: &Path,
    now: u64,
    value: String,
) -> PickerItem {
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
        value,
        selection_verb: Some("browse"),
    }
}

fn session_row(
    section: Option<&str>,
    session: &SessionSummary,
    current_session: Option<&SessionTarget>,
    current_cwd: &Path,
    now: u64,
    value: String,
) -> PickerItem {
    let target = session.target();
    let is_current = current_session == Some(&target);
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
    } else if is_cross_project(&session.cwd, current_cwd) {
        "\nStart Rho in this directory to resume. Press d to delete."
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
        value,
        selection_verb: Some(if is_current {
            "close"
        } else if is_cross_project(&session.cwd, current_cwd) {
            "unavailable"
        } else {
            "resume"
        }),
    }
}

fn cleanup_missing_workspaces_row(
    session_count: usize,
    directory_count: usize,
    value: String,
) -> PickerItem {
    PickerItem {
        section: Some("CLEAN UP".into()),
        label: "Delete sessions for missing directories".into(),
        detail: Some(format!(
            "Delete {} saved for {} that no longer exist.\nTranscripts and related run artifacts are removed. Usage history is kept.",
            count_label(session_count),
            if directory_count == 1 {
                "1 directory".to_string()
            } else {
                format!("{directory_count} directories")
            }
        )),
        preview: None,
        badge: Some(PickerBadge {
            text: count_label(session_count),
            tone: PickerBadgeTone::Warning,
        }),
        value,
        selection_verb: Some("clean up"),
    }
}

fn manage_sessions_picker(title: impl Into<String>, items: Vec<PickerItem>) -> UiPicker {
    UiPicker::new(title, items, PickerAction::ManageSessions)
        .with_key_hints(PickerKeyHints {
            tab_complete: false,
            row_delete: true,
            ..Default::default()
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
    current_session: Option<&SessionTarget>,
    current_cwd: &Path,
    now: u64,
    missing: Option<(usize, usize)>,
) -> SessionsPickerBuild {
    let mut items = Vec::new();
    let mut targets = Vec::new();
    if let Some((session_count, directory_count)) = missing {
        targets.push(SessionsHubTarget::CleanupMissingWorkspaces);
        items.push(cleanup_missing_workspaces_row(
            session_count,
            directory_count,
            target_value(targets.len() - 1),
        ));
    }
    for group in groups {
        targets.push(SessionsHubTarget::Directory(group.cwd.clone()));
        items.push(directory_row(
            group,
            current_cwd,
            now,
            target_value(targets.len() - 1),
        ));
        for session in &group.sessions {
            targets.push(SessionsHubTarget::Session(session.target()));
            items.push(session_row(
                Some(group.display.as_str()),
                session,
                current_session,
                current_cwd,
                now,
                target_value(targets.len() - 1),
            ));
        }
    }
    SessionsPickerBuild {
        picker: manage_sessions_picker("sessions", items),
        targets,
    }
}

/// Child list scoped to one directory's sessions.
pub(super) fn directory_picker(
    group: &DirectoryGroup,
    current_session: Option<&SessionTarget>,
    current_cwd: &Path,
    now: u64,
) -> SessionsPickerBuild {
    let targets = group
        .sessions
        .iter()
        .map(SessionSummary::target)
        .map(SessionsHubTarget::Session)
        .collect::<Vec<_>>();
    let items = group
        .sessions
        .iter()
        .enumerate()
        .map(|(index, session)| {
            session_row(
                None,
                session,
                current_session,
                current_cwd,
                now,
                target_value(index),
            )
        })
        .collect();
    SessionsPickerBuild {
        picker: manage_sessions_picker(group.display.clone(), items),
        targets,
    }
}

impl App {
    pub(super) fn current_session_target(&self) -> Option<SessionTarget> {
        self.info
            .session
            .session_id
            .as_ref()
            .map(|id| SessionTarget::new(id.clone(), self.info.runtime.cwd.clone()))
    }

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
            self.sessions_hub_state.clear();
            self.input_ui.set_composer(ComposerMode::Input);
            self.insert_entry(&Entry::Error(format!("could not open sessions: {error}")));
            self.set_status("sessions failed");
        }
    }

    pub(super) fn open_sessions_hub(&mut self) -> anyhow::Result<()> {
        let sessions = Session::list_all()?;
        if sessions.is_empty() {
            self.sessions_hub_state.clear();
            self.input_ui.set_composer(ComposerMode::Input);
            self.set_status("no saved sessions");
            return Ok(());
        }
        let groups = directory_groups(sessions, &self.info.runtime.cwd);
        let mut missing_session_count = 0usize;
        let mut missing_directory_count = 0usize;
        for group in &groups {
            if Session::workspace_directory_is_missing(&group.cwd)? {
                missing_session_count += group.sessions.len();
                missing_directory_count += 1;
            }
        }
        let missing =
            (missing_session_count > 0).then_some((missing_session_count, missing_directory_count));
        let current = self.current_session_target();
        let build = hub_picker(
            &groups,
            current.as_ref(),
            &self.info.runtime.cwd,
            session_picker::now_unix_secs(),
            missing,
        );
        self.sessions_hub_state.open_root(build.targets);
        self.input_ui
            .set_composer(ComposerMode::Picker(build.picker));
        self.set_status("sessions");
        Ok(())
    }

    pub(super) async fn submit_sessions_selection(
        &mut self,
        value: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(target) = self.sessions_hub_state.target(value) else {
            self.input_ui.set_composer(ComposerMode::Input);
            self.insert_entry(&Entry::Error(
                "sessions selection expired; reopen /sessions".into(),
            ));
            self.set_status("sessions selection failed");
            return Ok(());
        };
        match target {
            SessionsHubTarget::CleanupMissingWorkspaces => {
                self.prompt_cleanup_missing_session_directories()
            }
            SessionsHubTarget::Session(target) => {
                if self.current_session_target().as_ref() == Some(&target) {
                    self.input_ui.set_composer(ComposerMode::Input);
                    self.sessions_hub_state.clear();
                    self.set_status("already in this session");
                    return Ok(());
                }
                if is_cross_project(&target.cwd, &self.info.runtime.cwd) {
                    self.set_status("start Rho in that directory to resume this session");
                    return Ok(());
                }
                self.submit_resume_target(&target, terminal, agent).await
            }
            SessionsHubTarget::Directory(cwd) => self.open_directory_sessions(&cwd),
        }
    }

    fn open_directory_sessions(&mut self, cwd: &Path) -> anyhow::Result<()> {
        self.open_directory_sessions_restored(cwd, None)
    }

    pub(super) fn open_directory_sessions_restored(
        &mut self,
        cwd: &Path,
        cursor: Option<&super::PickerCursor>,
    ) -> anyhow::Result<()> {
        let sessions = Session::list(cwd)?;
        if sessions.is_empty() {
            self.sessions_hub_state.navigate_back();
            self.set_status("no saved sessions for this directory");
            return Ok(());
        }
        let group = DirectoryGroup {
            display: compact_cwd(cwd),
            cwd: cwd.to_path_buf(),
            sessions,
        };
        let current = self.current_session_target();
        let mut build = directory_picker(
            &group,
            current.as_ref(),
            &self.info.runtime.cwd,
            session_picker::now_unix_secs(),
        );
        if let Some(cursor) = cursor {
            build.picker.restore_cursor(cursor);
        }
        self.sessions_hub_state
            .open_directory(cwd.to_path_buf(), build.targets);
        self.open_child_picker(build.picker);
        Ok(())
    }

    pub(super) fn prompt_delete_selected_sessions_item(&mut self) -> anyhow::Result<()> {
        let Some(value) = self.selected_sessions_item_value() else {
            return Ok(());
        };
        let Some(target) = self.sessions_hub_state.target(&value) else {
            self.set_status("sessions selection expired; reopen /sessions");
            return Ok(());
        };
        match target {
            SessionsHubTarget::CleanupMissingWorkspaces => {
                self.prompt_cleanup_missing_session_directories()
            }
            SessionsHubTarget::Session(target) => {
                if self.current_session_target().as_ref() == Some(&target) {
                    self.set_status("cannot delete the current session");
                    return Ok(());
                }
                self.prompt_delete_session(target)
            }
            SessionsHubTarget::Directory(cwd) => self.prompt_delete_directory_sessions(&cwd),
        }
    }

    fn prompt_delete_directory_sessions(&mut self, cwd: &Path) -> anyhow::Result<()> {
        let targets = Session::list(cwd)?
            .into_iter()
            .map(|session| session.target())
            .collect::<Vec<_>>();
        let count = targets.len();
        let display = compact_cwd(cwd);
        let choice = InlineChoice::new(
            format!("Delete all sessions in {display}?"),
            format!(
                "Removes {} saved in this directory, with transcripts, cached web content, and their subagent runs. The current session is kept. Usage history is kept.",
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
        self.open_session_choice(
            choice,
            InlineChoicePending::DeleteDirectorySessions {
                cwd: cwd.to_path_buf(),
                targets,
            },
            "confirm delete directory sessions",
        )
    }

    fn prompt_cleanup_missing_session_directories(&mut self) -> anyhow::Result<()> {
        let candidates = Session::list_missing_workspaces()?;
        if candidates.is_empty() {
            self.set_status("no sessions need cleanup");
            return Ok(());
        }
        let directory_count = candidates
            .iter()
            .map(|session| session.cwd.clone())
            .collect::<HashSet<_>>()
            .len();
        let choice = InlineChoice::new(
            "Delete sessions for missing directories?",
            format!(
                "Permanently removes {} saved for {} that no longer exist, with transcripts and parent-linked subagent runs. Usage history is kept.",
                count_label(candidates.len()),
                if directory_count == 1 {
                    "1 directory".to_string()
                } else {
                    format!("{directory_count} directories")
                }
            ),
            vec![
                InlineChoiceOption::available(
                    "delete",
                    'd',
                    "Delete all",
                    "Remove every saved session whose workspace directory no longer exists",
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
        self.open_session_choice(
            choice,
            InlineChoicePending::CleanupMissingSessionDirectories {
                targets: candidates
                    .into_iter()
                    .map(|session| session.target())
                    .collect(),
            },
            "confirm session cleanup",
        )
    }

    pub(super) fn open_session_choice(
        &mut self,
        choice: InlineChoice,
        pending: InlineChoicePending,
        status: &'static str,
    ) -> anyhow::Result<()> {
        let previous = self.input_ui.take_composer();
        let ComposerMode::Picker(parent) = previous else {
            self.input_ui.set_composer(previous);
            anyhow::bail!("session confirmation requires an active picker");
        };
        self.input_ui
            .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                choice,
                pending,
                parent_picker: Some(Box::new(parent)),
            }));
        self.set_status(status);
        Ok(())
    }

    pub(super) fn submit_cleanup_missing_session_directories_choice(
        &mut self,
        value: &str,
        targets: &[SessionTarget],
        parent: Option<Box<UiPicker>>,
    ) -> anyhow::Result<()> {
        if value != "delete" {
            self.restore_session_choice_parent(parent);
            return Ok(());
        }
        let outcome = Session::cleanup_missing_targets(
            targets,
            DeleteOptions {
                force: false,
                protected_session: self.current_session_target(),
            },
        )?;
        for failure in &outcome.failures {
            self.insert_entry(&Entry::Error(format!(
                "could not delete session {} ({}): {}",
                session_picker::short_session_id(&failure.id),
                compact_cwd(&failure.cwd),
                failure.error
            )));
        }
        let mut notice = format!("cleaned up {}", count_label(outcome.deleted.len()));
        if !outcome.failures.is_empty() {
            notice.push_str(&format!(", {} failed", outcome.failures.len()));
        }
        if outcome.restored_workspaces > 0 {
            notice.push_str(&format!(
                ", {} skipped after restore",
                outcome.restored_workspaces
            ));
        }
        self.refresh_sessions_location(parent.as_deref())?;
        self.set_status(notice);
        Ok(())
    }

    pub(super) fn submit_delete_directory_sessions_choice(
        &mut self,
        value: &str,
        cwd: &Path,
        targets: &[SessionTarget],
        parent: Option<Box<UiPicker>>,
    ) -> anyhow::Result<()> {
        if value != "delete" {
            self.restore_session_choice_parent(parent);
            return Ok(());
        }

        let display = compact_cwd(cwd);
        let outcome = Session::delete_targets(
            targets,
            DeleteOptions {
                force: false,
                protected_session: self.current_session_target(),
            },
        )?;
        for failure in &outcome.failures {
            self.insert_entry(&Entry::Error(format!(
                "could not delete session {}: {}",
                session_picker::short_session_id(&failure.id),
                failure.error
            )));
        }
        let mut notice = format!(
            "deleted {} in {display}",
            count_label(outcome.deleted.len())
        );
        if !outcome.kept_protected.is_empty() {
            notice.push_str(", kept the current session");
        }
        if !outcome.failures.is_empty() {
            notice.push_str(&format!(", {} failed", outcome.failures.len()));
        }
        self.refresh_sessions_location(parent.as_deref())?;
        self.set_status(notice);
        Ok(())
    }

    pub(super) fn restore_session_choice_parent(&mut self, parent: Option<Box<UiPicker>>) {
        let Some(parent) = parent else {
            self.input_ui.set_composer(ComposerMode::Input);
            self.sessions_hub_state.clear();
            return;
        };
        let action = parent.action;
        self.input_ui.set_composer(ComposerMode::Picker(*parent));
        self.set_status(match action {
            PickerAction::ManageSessions => "sessions",
            PickerAction::ResumeSession => "select session",
            _ => "ready",
        });
    }

    pub(super) fn refresh_sessions_location(
        &mut self,
        previous: Option<&UiPicker>,
    ) -> anyhow::Result<()> {
        let location = self.sessions_hub_state.location();
        let cursor = previous.map(UiPicker::cursor);
        self.open_sessions_hub()?;
        if let SessionsLocation::Directory(cwd) = location {
            if matches!(self.input_ui.composer(), ComposerMode::Picker(_)) {
                self.open_directory_sessions_restored(&cwd, cursor.as_ref())?;
            }
        } else if let (Some(cursor), ComposerMode::Picker(picker)) =
            (cursor.as_ref(), self.input_ui.composer_mut())
        {
            picker.restore_cursor(cursor);
        }
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
