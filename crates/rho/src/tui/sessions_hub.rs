//! In-app `/sessions` hub: browse, resume, and delete saved sessions from
//! every directory, grouped by directory and by Git repository.

use std::path::{Path, PathBuf};

use ratatui::DefaultTerminal;

use super::sessions_hub_groups::{find_directory, DirectoryGroup, HubGroup};
use super::sessions_hub_tasks::{plural, DeleteOrigin, SessionsDelete};
use super::{
    picker::OverlayChrome, session_picker, statusline::path::compact_cwd, App, ComposerMode, Entry,
    InlineChoice, InlineChoiceModal, InlineChoiceOption, InlineChoicePending, InteractiveRuntime,
    PickerBadge, PickerBadgeTone, PickerCursor, PickerItem, PickerKeyHints, PickerLayout, UiPicker,
};
use crate::session::{is_cross_project, Session, SessionSummary, SessionTarget};

const TARGET_PREFIX: &str = "sessions-target:";

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

/// Open hub state. `groups` is the last loaded listing: drilling into a
/// directory and building prompts reads it instead of disk.
#[derive(Debug, Default)]
pub(super) struct SessionsHubState {
    location: SessionsLocation,
    targets: Vec<SessionsHubTarget>,
    root_targets: Vec<SessionsHubTarget>,
    groups: Vec<HubGroup>,
    /// The hub shows a placeholder until the first listing lands.
    loading: bool,
}

impl SessionsHubState {
    fn open_root(&mut self, targets: Vec<SessionsHubTarget>, groups: Vec<HubGroup>) {
        self.loading = false;
        self.location = SessionsLocation::Root;
        self.root_targets.clone_from(&targets);
        self.targets = targets;
        self.groups = groups;
    }

    fn directory(&self, cwd: &Path) -> Option<&DirectoryGroup> {
        find_directory(&self.groups, cwd)
    }

    fn missing_directories(&self) -> &[DirectoryGroup] {
        self.groups
            .iter()
            .find_map(|group| match group {
                HubGroup::Missing(missing) => Some(missing.as_slice()),
                HubGroup::Directory(_) | HubGroup::Repo { .. } => None,
            })
            .unwrap_or_default()
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

fn directory_count_label(count: usize) -> String {
    if count == 1 {
        "1 directory".to_string()
    } else {
        format!("{count} directories")
    }
}

fn target_value(index: usize) -> String {
    format!("{TARGET_PREFIX}{index}")
}

/// `label` names the row: "All sessions" under a directory's own section, or
/// the directory's short name inside a shared section.
fn directory_row(
    section: &str,
    label: &str,
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
    let updated = session_picker::format_updated_ago(newest, now);
    PickerItem {
        section: Some(section.to_owned()),
        label: format!("{label} · {}", group.sessions.len()),
        detail: None,
        preview: Some(format!("newest {updated}")),
        badge: is_current.then(|| PickerBadge {
            text: "current dir".into(),
            tone: PickerBadgeTone::Selected,
        }),
        value,
        selection_verb: Some("browse"),
        allow_filter_completion: true,
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
    let title = session
        .title
        .as_deref()
        .or(session.first_user_message.as_deref())
        .map(session_picker::preview_text)
        .unwrap_or_else(|| format!("session {short_id}"));
    // Row preview: age, then the latest prompt when it adds something new.
    let updated = session_picker::format_updated_ago(session.updated_at, now);
    let preview = match session
        .last_user_message
        .as_deref()
        .map(session_picker::preview_text)
        .filter(|last_user| last_user != &title)
    {
        Some(last_user) => format!("{updated} · last: {last_user}"),
        None => updated,
    };
    PickerItem {
        section: section.map(str::to_owned),
        label: title,
        detail: None,
        preview: Some(preview),
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
        allow_filter_completion: true,
    }
}

fn cleanup_missing_workspaces_row(missing: &[DirectoryGroup], value: String) -> PickerItem {
    let session_count = missing.iter().map(|group| group.sessions.len()).sum();
    let directory_count = missing.len();
    PickerItem {
        section: Some("CLEAN UP".into()),
        label: "Delete sessions for missing directories".into(),
        detail: None,
        preview: Some(directory_count_label(directory_count)),
        badge: Some(PickerBadge {
            text: plural(session_count, "session"),
            tone: PickerBadgeTone::Warning,
        }),
        value,
        selection_verb: Some("clean up"),
        allow_filter_completion: true,
    }
}

fn manage_sessions_picker(title: impl Into<String>, items: Vec<PickerItem>) -> UiPicker {
    UiPicker::manage_sessions(title, items)
        .with_key_hints(PickerKeyHints {
            tab_complete: false,
            row_delete: true,
            ..Default::default()
        })
        .with_layout(PickerLayout::Overlay)
        .with_overlay_chrome(OverlayChrome {
            nav_label: " SESSIONS".into(),
            detail_label: None,
            nav_keys_hint: "↑↓ items".into(),
        })
        .with_restore_status("sessions")
}

/// Section title for directories that no longer exist.
const MISSING_SECTION: &str = "MISSING DIRECTORIES";

/// Root list builder: section rows plus the target behind each row.
struct HubRows<'a> {
    items: Vec<PickerItem>,
    targets: Vec<SessionsHubTarget>,
    current_session: Option<&'a SessionTarget>,
    current_cwd: &'a Path,
    now: u64,
}

impl HubRows<'_> {
    fn next_value(&mut self, target: SessionsHubTarget) -> String {
        self.targets.push(target);
        target_value(self.targets.len() - 1)
    }

    fn directory(&mut self, section: &str, label: &str, group: &DirectoryGroup) {
        let value = self.next_value(SessionsHubTarget::Directory(group.cwd.clone()));
        let row = directory_row(section, label, group, self.current_cwd, self.now, value);
        self.items.push(row);
    }

    fn sessions(&mut self, section: &str, group: &DirectoryGroup) {
        for session in &group.sessions {
            let value = self.next_value(SessionsHubTarget::Session(session.target()));
            let row = session_row(
                Some(section),
                session,
                self.current_session,
                self.current_cwd,
                self.now,
                value,
            );
            self.items.push(row);
        }
    }
}

/// Root list: one section per group, current directory first. Single
/// directories list their sessions inline; multi-worktree repositories inline
/// only the current directory; missing directories get a cleanup row and
/// never inline.
pub(super) fn hub_picker(
    groups: &[HubGroup],
    current_session: Option<&SessionTarget>,
    current_cwd: &Path,
    now: u64,
) -> SessionsPickerBuild {
    let mut rows = HubRows {
        items: Vec::new(),
        targets: Vec::new(),
        current_session,
        current_cwd,
        now,
    };
    // Cleanup leads the list so stale directories are noticed first.
    let missing = groups.iter().find_map(|group| match group {
        HubGroup::Missing(missing) => Some(missing),
        HubGroup::Directory(_) | HubGroup::Repo { .. } => None,
    });
    if let Some(missing) = missing {
        let value = rows.next_value(SessionsHubTarget::CleanupMissingWorkspaces);
        rows.items
            .push(cleanup_missing_workspaces_row(missing, value));
    }
    for group in groups {
        match group {
            HubGroup::Directory(directory) => {
                rows.directory(&directory.display, "All sessions", directory);
                rows.sessions(&directory.display, directory);
            }
            HubGroup::Repo { display, worktrees } => {
                for worktree in worktrees {
                    rows.directory(display, &worktree.name, &worktree.directory);
                    if !is_cross_project(&worktree.directory.cwd, current_cwd) {
                        rows.sessions(display, &worktree.directory);
                    }
                }
            }
            HubGroup::Missing(missing) => {
                for directory in missing {
                    rows.directory(MISSING_SECTION, &directory.display, directory);
                }
            }
        }
    }
    SessionsPickerBuild {
        picker: manage_sessions_picker("Sessions", rows.items),
        targets: rows.targets,
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

    /// `/sessions`: open the hub at once and list sessions in the background.
    pub(super) fn execute_sessions_command(&mut self) {
        if self.refuse_while_sessions_delete_runs() {
            return;
        }
        self.sessions_hub_state.clear();
        self.sessions_hub_state.loading = true;
        self.input_ui.set_composer(ComposerMode::Picker(
            manage_sessions_picker("Sessions", Vec::new()).with_empty_message("loading sessions…"),
        ));
        self.set_status("loading sessions");
        self.start_sessions_load();
    }

    /// Fill the loading hub with the listing. Dropped when the user closed
    /// the hub first, so a late listing never reopens it.
    pub(super) fn fill_sessions_hub(&mut self, groups: anyhow::Result<Vec<HubGroup>>) {
        let hub_waiting = self.sessions_hub_state.loading
            && matches!(self.input_ui.composer(), ComposerMode::Picker(picker) if picker.is_manage_sessions());
        if !hub_waiting {
            return;
        }
        match groups {
            Ok(groups) => {
                // Keep anything typed into the search box while loading.
                let cursor = match self.input_ui.composer() {
                    ComposerMode::Picker(picker) => Some(picker.cursor()),
                    _ => None,
                };
                self.show_sessions_hub(groups);
                if let (Some(cursor), ComposerMode::Picker(picker)) =
                    (cursor.as_ref(), self.input_ui.composer_mut())
                {
                    picker.restore_cursor(cursor);
                }
            }
            Err(error) => {
                self.sessions_hub_state.clear();
                self.input_ui.set_composer(ComposerMode::Input);
                self.insert_entry(&Entry::Error(format!("could not open sessions: {error}")));
                self.set_status("sessions failed");
            }
        }
    }

    /// Show the root hub for `groups`.
    fn show_sessions_hub(&mut self, groups: Vec<HubGroup>) {
        if groups.is_empty() {
            self.sessions_hub_state.clear();
            self.input_ui.set_composer(ComposerMode::Input);
            self.set_status("no saved sessions");
            return;
        }
        let current = self.current_session_target();
        let build = hub_picker(
            &groups,
            current.as_ref(),
            &self.info.runtime.cwd,
            session_picker::now_unix_secs(),
        );
        self.sessions_hub_state.open_root(build.targets, groups);
        self.input_ui
            .set_composer(ComposerMode::Picker(build.picker));
        self.set_status("sessions");
    }

    /// Rebuild the open hub from a fresh listing after a delete, keeping the
    /// open directory and cursor.
    pub(super) fn refresh_sessions_hub(&mut self, groups: Vec<HubGroup>) {
        let location = self.sessions_hub_state.location();
        let cursor = match self.input_ui.composer() {
            ComposerMode::Picker(picker) => Some(picker.cursor()),
            _ => None,
        };
        self.show_sessions_hub(groups);
        if let SessionsLocation::Directory(cwd) = location {
            if matches!(self.input_ui.composer(), ComposerMode::Picker(_)) {
                self.open_directory_sessions(&cwd, cursor.as_ref());
            }
        } else if let (Some(cursor), ComposerMode::Picker(picker)) =
            (cursor.as_ref(), self.input_ui.composer_mut())
        {
            picker.restore_cursor(cursor);
        }
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
                // The session may be in the batch being deleted.
                if self.refuse_while_sessions_delete_runs() {
                    return Ok(());
                }
                self.submit_resume_target(&target, terminal, agent).await
            }
            SessionsHubTarget::Directory(cwd) => {
                self.open_directory_sessions(&cwd, None);
                Ok(())
            }
        }
    }

    /// Drill into one directory's sessions from the loaded listing.
    fn open_directory_sessions(&mut self, cwd: &Path, cursor: Option<&PickerCursor>) {
        let current = self.current_session_target();
        let Some(group) = self.sessions_hub_state.directory(cwd) else {
            self.sessions_hub_state.navigate_back();
            self.set_status("no saved sessions for this directory");
            return;
        };
        let mut build = directory_picker(
            group,
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
    }

    pub(super) fn prompt_delete_selected_sessions_item(&mut self) -> anyhow::Result<()> {
        let Some(value) = self.selected_sessions_item_value() else {
            return Ok(());
        };
        if self.refuse_while_sessions_delete_runs() {
            return Ok(());
        }
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
        // List fresh so "all sessions" includes any saved since the hub
        // loaded. One workspace is an index query, well under a millisecond.
        let targets = Session::list(cwd)?
            .iter()
            .map(SessionSummary::target)
            .collect::<Vec<_>>();
        let count = targets.len();
        let display = compact_cwd(cwd);
        let choice = InlineChoice::new(
            format!("Delete all sessions in {display}?"),
            format!(
                "Removes {} saved in this directory, with transcripts, cached web content, and their subagent runs. The current session is kept. Usage history is kept.",
                plural(count, "session")
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
        if self.refuse_while_sessions_delete_runs() {
            return Ok(());
        }
        let missing = self.sessions_hub_state.missing_directories();
        let targets = missing
            .iter()
            .flat_map(|group| group.sessions.iter().map(SessionSummary::target))
            .collect::<Vec<_>>();
        let directory_count = missing.len();
        if targets.is_empty() {
            self.set_status("no sessions need cleanup");
            return Ok(());
        }
        let choice = InlineChoice::new(
            "Delete sessions for missing directories?",
            format!(
                "Permanently removes {} saved for {} that no longer exist, with transcripts, cached web content, and their subagent runs. Usage history is kept.",
                plural(targets.len(), "session"),
                directory_count_label(directory_count)
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
            InlineChoicePending::CleanupMissingSessionDirectories { targets },
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

    /// Answer a session delete confirmation. On "delete", the picker returns
    /// at once and the delete runs in the background.
    pub(super) fn submit_sessions_delete_choice(
        &mut self,
        value: &str,
        request: SessionsDelete,
        parent: Option<Box<UiPicker>>,
    ) {
        let origin = match parent.as_deref() {
            Some(picker) if picker.is_resume_session() => DeleteOrigin::Resume,
            _ => DeleteOrigin::Hub,
        };
        self.restore_session_choice_parent(parent);
        if value != "delete" {
            return;
        }
        // Prompts refuse while a delete runs; this catches one confirmed
        // after a delete started behind the open prompt.
        if self.refuse_while_sessions_delete_runs() {
            return;
        }
        self.start_sessions_delete(request, origin);
    }

    pub(super) fn restore_session_choice_parent(&mut self, parent: Option<Box<UiPicker>>) {
        let Some(parent) = parent else {
            self.input_ui.set_composer(ComposerMode::Input);
            self.sessions_hub_state.clear();
            return;
        };
        let status = parent.restore_status();
        self.input_ui.set_composer(ComposerMode::Picker(*parent));
        self.set_status(status);
    }

    fn selected_sessions_item_value(&self) -> Option<String> {
        match self.input_ui.composer() {
            ComposerMode::Picker(picker) if picker.is_manage_sessions() => {
                picker.selected_item().map(|item| item.value.clone())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "sessions_hub_tests.rs"]
mod tests;
