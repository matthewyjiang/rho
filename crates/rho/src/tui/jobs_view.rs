//! `/jobs`: every job the daemon runs, across projects, in one list, with
//! the keys to act on the selected one (#1668).
//!
//! The list is `rho/job/list`, which the daemon rail already polls; the
//! view takes each answer while it is open. A row's keys send their
//! requests on the followed session's connection and go on without
//! waiting; a request the daemon refuses shows as the view's notice. A
//! request another session waits on is answered by that session's id, with
//! no attachment to it, so the view answers prompts in any project.
//! Attaching switches the session this TUI follows, which only a session
//! of this project can be.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
    time::Instant,
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use rho_protocol::{
    methods::{SessionAnswer, SessionCancel, SessionNew, SessionPrompt, SessionSteer},
    Ask, ClientEnvironment, ContentBlock, ErrorCode, InputAnswer, InputRequest, JobId, JobKind,
    JobState, JobSummary, PromptAuthor, SessionAnswerParams, SessionCancelParams, SessionId,
    SessionNewParams, SessionPart, SessionPromptParams, SessionSteerParams,
};

use super::{
    approval::ApprovalPrompt,
    daemon_launch::{DaemonLaunch, DaemonTarget},
    overlay_panel::{reveal_panel_cursor, terminal_area, PanelBody, PanelKeyOutcome, PanelState},
    panel_text::indented_wrapped_lines,
    render::truncate_one_line,
    App, ComposerMode, DefaultTerminal, PanelOverlay, Theme,
};
use crate::{
    app::daemon_client::{Daemon, DaemonError},
    session::Session,
};

mod rows;

use rows::{groups, needs_input, Group, Row, Status};

const BROWSE_FOOTER: &str =
    "↑↓ · Enter attach · m message · c cancel · n new · / filter · Tab next ask · Esc close";
const ASK_FOOTER: &str =
    "y allow once · a allow for session · d deny · ↑↓ · Enter attach · Esc close";
const INPUT_FOOTER: &str = "Enter confirm · Esc cancel";

/// What the view's keys go to.
#[derive(Debug)]
enum Mode {
    /// The list: keys select a row and act on it.
    Browse,
    /// Typing the filter, which applies as it is typed.
    Filter,
    /// Typing a message for session `session`, which steers its turn when
    /// one is under way as the message is sent, and starts its next when
    /// none is.
    Message { session: SessionId, text: String },
    /// Typing the directory a new session works in.
    NewWorkspace { path: String },
    /// Typing the new session's first prompt.
    NewPrompt { workspace: PathBuf, text: String },
}

/// What closing the view asks of the TUI.
#[derive(Debug)]
pub(super) struct Attach {
    session: SessionId,
    /// The workspace the session's job runs in.
    workspace: PathBuf,
}

/// The `/jobs` overlay.
pub(super) struct JobsView {
    daemon: Arc<Daemon>,
    /// The session this TUI follows, whose job the view opens on.
    followed: SessionId,
    /// What this TUI opens its sessions with. A session started here opens
    /// with its setup, though always saved, and in this TUI's project as
    /// its agent too, so
    /// attaching finds the agent the launch named. Another project may not
    /// define that agent, so a session there runs as its default.
    launch: DaemonLaunch,
    /// The daemon's jobs as last listed; `None` before the first answer.
    jobs: Option<Vec<JobSummary>>,
    selected: Option<JobId>,
    filter: String,
    mode: Mode,
    /// Why the last request failed, until the next key.
    notice: Arc<Mutex<Option<String>>>,
    attach: Option<Attach>,
    panel: PanelState,
}

impl std::fmt::Debug for JobsView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JobsView")
            .field("selected", &self.selected)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl JobsView {
    fn new(daemon: Arc<Daemon>, followed: SessionId, launch: DaemonLaunch) -> Self {
        Self {
            daemon,
            followed,
            launch,
            jobs: None,
            selected: None,
            filter: String::new(),
            mode: Mode::Browse,
            notice: Arc::default(),
            attach: None,
            panel: PanelState::default(),
        }
    }

    fn groups(&self) -> Vec<Group> {
        groups(self.jobs.as_deref().unwrap_or_default(), &self.filter)
    }

    fn job(&self, id: &JobId) -> Option<&JobSummary> {
        self.jobs.as_ref()?.iter().find(|job| &job.id == id)
    }

    fn selected_job(&self) -> Option<&JobSummary> {
        self.job(self.selected.as_ref()?)
    }

    /// Takes the daemon's latest list, keeping the selection on its job
    /// while that is listed. The first list selects the followed session's
    /// job. Returns whether the selection's rows may have moved or grown,
    /// so the view scrolls to them again.
    fn show(&mut self, jobs: Vec<JobSummary>) -> bool {
        let before = self.cursor_place();
        if self.jobs.is_none() {
            self.selected = jobs
                .iter()
                .find(|job| job.session.as_ref() == Some(&self.followed))
                .map(|job| job.id.clone());
        }
        self.jobs = Some(jobs);
        self.select_listed();
        before != self.cursor_place()
    }

    /// Where the selection is in the list, and whether its job asks
    /// something, which shows under its row.
    fn cursor_place(&self) -> (Option<usize>, bool) {
        let at = self
            .selected
            .as_ref()
            .and_then(|selected| self.listed().iter().position(|job| job == selected));
        (at, self.open_request().is_some())
    }

    /// Moves the selection to a listed row when it names none.
    fn select_listed(&mut self) {
        let listed: Vec<JobId> = self.listed();
        if !self
            .selected
            .as_ref()
            .is_some_and(|selected| listed.contains(selected))
        {
            self.selected = listed.into_iter().next();
        }
    }

    /// The listed jobs, in row order.
    fn listed(&self) -> Vec<JobId> {
        self.groups()
            .into_iter()
            .flat_map(|group| group.rows)
            .map(|row| row.job)
            .collect()
    }

    /// Moves the selection `by` rows, stopping at either end.
    fn step(&mut self, by: isize) {
        let listed = self.listed();
        let at = self
            .selected
            .as_ref()
            .and_then(|selected| listed.iter().position(|job| job == selected));
        let next = match at {
            Some(at) => at
                .saturating_add_signed(by)
                .min(listed.len().saturating_sub(1)),
            None => 0,
        };
        self.selected = listed.into_iter().nth(next);
    }

    /// Selects the next job after the selection that needs input, wrapping.
    fn jump_to_needs_input(&mut self) {
        let listed = self.listed();
        let start = self
            .selected
            .as_ref()
            .and_then(|selected| listed.iter().position(|job| job == selected))
            .map_or(0, |at| at + 1);
        let next = (0..listed.len())
            .map(|offset| &listed[(start + offset) % listed.len()])
            .find(|id| self.job(id).is_some_and(needs_input));
        match next {
            Some(next) => self.selected = Some(next.clone()),
            None => self.say("no job needs input"),
        }
    }

    fn say(&self, notice: impl Into<String>) {
        *self.notice.lock().unwrap_or_else(PoisonError::into_inner) = Some(notice.into());
    }

    fn notice(&self) -> Option<String> {
        self.notice
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Sends `request` without waiting; a refusal becomes the notice
    /// `could not <what>: <why>`.
    fn send<F>(&self, what: &'static str, request: impl FnOnce(Arc<Daemon>) -> F)
    where
        F: std::future::Future<Output = Result<(), DaemonError>> + Send + 'static,
    {
        let notice = Arc::clone(&self.notice);
        let request = request(Arc::clone(&self.daemon));
        tokio::spawn(async move {
            if let Err(error) = request.await {
                *notice.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(format!("could not {what}: {error}"));
            }
        });
    }

    /// The selected job's oldest open request, with the session it is
    /// answered on.
    fn open_request(&self) -> Option<(&SessionId, &InputRequest)> {
        let job = self.selected_job()?;
        let request = job.live.as_ref()?.inputs.first()?;
        Some((job.session.as_ref()?, request))
    }

    /// Answers the selected job's open approval with `answer`.
    fn answer(&self, answer: InputAnswer) {
        let Some((session, request)) = self.open_request() else {
            return;
        };
        let params = SessionAnswerParams {
            session: session.clone(),
            request: request.id,
            answer,
        };
        self.send("answer the request", |daemon| async move {
            daemon.request::<SessionAnswer>(&params).await.map(|_| ())
        });
    }

    /// The session a message, a cancel, or an attach for the selected job
    /// goes to: the job's own, or for a run, the session that started it.
    fn session_of_selected(&self) -> Option<(&JobSummary, &SessionId)> {
        let jobs = self.jobs.as_ref()?;
        let mut job = self.selected_job()?;
        // Each step goes up one job, so a chain is never longer than the
        // list.
        for _ in 0..jobs.len() {
            if job.kind == JobKind::Session {
                return Some((job, job.session.as_ref()?));
            }
            let parent = job.parent.as_ref()?;
            job = jobs.iter().find(|job| &job.id == parent)?;
        }
        None
    }

    fn cancel(&self) {
        let Some((_, session)) = self.session_of_selected() else {
            return self.say("could not cancel job: no session of the daemon's started it");
        };
        let params = SessionCancelParams {
            session: session.clone(),
        };
        self.send("cancel the turn", |daemon| async move {
            daemon.request::<SessionCancel>(&params).await.map(|_| ())
        });
    }

    fn begin_message(&mut self) {
        let Some((_, session)) = self.session_of_selected() else {
            return self.say("could not message job: no session of the daemon's started it");
        };
        self.mode = Mode::Message {
            session: session.clone(),
            text: String::new(),
        };
    }

    /// Whether `session`'s turn waits on a request of its own, as last
    /// listed.
    fn waits_on_request(&self, session: &SessionId) -> bool {
        self.jobs.iter().flatten().any(|job| {
            job.kind == JobKind::Session
                && job.session.as_ref() == Some(session)
                && job.state == JobState::WaitingForInput
        })
    }

    /// Whether `session`'s job has a turn under way, as last listed.
    fn turn_under_way(&self, session: &SessionId) -> bool {
        self.jobs.iter().flatten().any(|job| {
            job.kind == JobKind::Session
                && job.session.as_ref() == Some(session)
                && job.state.in_progress()
        })
    }

    fn begin_new_session(&mut self) {
        let workspace = self
            .selected_job()
            .map_or_else(|| self.launch.cwd.clone(), |job| job.workspace.clone());
        self.mode = Mode::NewWorkspace {
            path: workspace.to_string_lossy().into_owned(),
        };
    }

    /// Runs the input a mode held once Enter confirmed it.
    fn confirm(&mut self) {
        match std::mem::replace(&mut self.mode, Mode::Browse) {
            Mode::Browse | Mode::Filter => {}
            Mode::Message { text, .. } if text.trim().is_empty() => {}
            // A turn waiting on a request takes neither a steer nor a
            // prompt: the draft stays until the request is answered.
            Mode::Message { session, text } if self.waits_on_request(&session) => {
                self.mode = Mode::Message { session, text };
                self.say(
                    "could not send the message: the session waits on a request; answer it first",
                );
            }
            Mode::Message { session, text } => {
                let under_way = self.turn_under_way(&session);
                self.send("send the message", move |daemon| {
                    message(daemon, session, text, under_way)
                });
            }
            Mode::NewWorkspace { path } => {
                let workspace = PathBuf::from(path.trim());
                if workspace.is_absolute() {
                    self.mode = Mode::NewPrompt {
                        workspace,
                        text: String::new(),
                    };
                } else {
                    // Kept, so the path is fixed rather than typed again.
                    self.mode = Mode::NewWorkspace { path };
                    self.say("could not start session: the workspace must be an absolute path");
                }
            }
            Mode::NewPrompt { text, .. } if text.trim().is_empty() => {}
            Mode::NewPrompt { workspace, text } => {
                let mut setup = self.launch.setup.clone();
                // Saved even under `--no-save`: no client follows it, so
                // one kept in memory alone is discarded once idle, and
                // attaching finds a session by its file.
                setup.without.remove(&SessionPart::Saving);
                let agent = (workspace == self.launch.cwd)
                    .then(|| self.launch.agent.clone())
                    .flatten();
                self.send("start session", |daemon| async move {
                    let params = SessionNewParams {
                        cwd: workspace,
                        environment: ClientEnvironment::of_this_process(),
                        agent,
                        setup,
                        parent: None,
                        grant: None,
                    };
                    let session = daemon.request::<SessionNew>(&params).await?.session;
                    prompt(daemon, session, text).await
                });
            }
        }
    }

    /// The text a typing mode edits.
    fn input_mut(&mut self) -> Option<&mut String> {
        match &mut self.mode {
            Mode::Browse => None,
            Mode::Filter => Some(&mut self.filter),
            Mode::Message { text, .. } | Mode::NewPrompt { text, .. } => Some(text),
            Mode::NewWorkspace { path } => Some(path),
        }
    }

    fn typing_key(&mut self, key: KeyEvent) -> PanelKeyOutcome {
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => return PanelKeyOutcome::Unhandled,
            (_, KeyCode::Esc) => {
                if matches!(self.mode, Mode::Filter) {
                    self.filter.clear();
                }
                self.mode = Mode::Browse;
            }
            (_, KeyCode::Enter) => self.confirm(),
            (_, KeyCode::Backspace) => {
                if let Some(input) = self.input_mut() {
                    input.pop();
                }
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(typed)) => {
                if let Some(input) = self.input_mut() {
                    input.push(typed);
                }
            }
            _ => {}
        }
        self.select_listed();
        PanelKeyOutcome::Handled
    }

    fn browse_key(&mut self, key: KeyEvent) -> PanelKeyOutcome {
        let approval = matches!(
            self.open_request(),
            Some((
                _,
                InputRequest {
                    ask: Ask::Approval(_),
                    ..
                }
            ))
        );
        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Up | KeyCode::Char('k')) => self.step(-1),
            (KeyModifiers::NONE, KeyCode::Down | KeyCode::Char('j')) => self.step(1),
            (KeyModifiers::NONE, KeyCode::Tab) => self.jump_to_needs_input(),
            (KeyModifiers::NONE, KeyCode::Char('/')) => self.mode = Mode::Filter,
            (KeyModifiers::NONE, KeyCode::Char('m')) => self.begin_message(),
            (KeyModifiers::NONE, KeyCode::Char('n')) => self.begin_new_session(),
            (KeyModifiers::NONE, KeyCode::Char('c')) => self.cancel(),
            (KeyModifiers::NONE, KeyCode::Char('y')) if approval => {
                self.answer(InputAnswer::AllowOnce);
            }
            (KeyModifiers::NONE, KeyCode::Char('a')) if approval => {
                self.answer(InputAnswer::AllowForSession);
            }
            (KeyModifiers::NONE, KeyCode::Char('d')) if approval => {
                self.answer(InputAnswer::Deny { reason: None });
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                match self.session_of_selected() {
                    // Already followed: closing the view shows it.
                    Some((_, session)) if session == &self.followed => {}
                    Some((job, session)) => {
                        self.attach = Some(Attach {
                            session: session.clone(),
                            workspace: job.workspace.clone(),
                        });
                    }
                    None if self.selected.is_some() => {
                        self.say("could not attach job: no session of the daemon's started it");
                        return PanelKeyOutcome::Handled;
                    }
                    None => {}
                }
                // The shared keys close the view, which then attaches.
                return PanelKeyOutcome::Unhandled;
            }
            // A filter clears before the view closes.
            (KeyModifiers::NONE, KeyCode::Esc) if !self.filter.is_empty() => {
                self.filter.clear();
                self.select_listed();
            }
            // Close, page, and copy keys.
            _ => return PanelKeyOutcome::Unhandled,
        }
        PanelKeyOutcome::Handled
    }

    /// The body's rows at `width`, and the rows the selection takes: its
    /// own and those of what its job asks.
    fn rows(&self, width: usize) -> (Vec<Line<'static>>, Option<std::ops::Range<usize>>) {
        let mut lines = Vec::new();
        let mut selected_rows = None;
        let groups = self.groups();
        if groups.is_empty() {
            let empty = match (&self.jobs, self.filter.is_empty()) {
                (None, _) => "Loading jobs",
                (Some(_), true) => "No jobs are running",
                (Some(_), false) => "No jobs match the filter",
            };
            lines.extend(indented_wrapped_lines(empty, 1, width, Theme::dim()));
        }
        for group in groups {
            lines.push(Line::styled(
                format!(" {}", crate::paths::display(&group.workspace)),
                Theme::dim().add_modifier(Modifier::BOLD),
            ));
            for row in &group.rows {
                let selected = self.selected.as_ref() == Some(&row.job);
                let first = lines.len();
                lines.push(row_line(row, selected, width));
                if selected {
                    lines.extend(self.request_lines(width));
                    selected_rows = Some(first..lines.len());
                }
            }
            lines.push(Line::default());
        }
        (lines, selected_rows)
    }

    /// The rows that stay in view under the list: the filter, the field a
    /// typing mode fills, and why the last request failed.
    fn pinned(&self, width: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let filtering = matches!(self.mode, Mode::Filter);
        if filtering || !self.filter.is_empty() {
            lines.push(input_line("Filter", &self.filter, filtering));
        }
        match &self.mode {
            Mode::Browse | Mode::Filter => {}
            Mode::Message { session, text } => {
                let label = if self.turn_under_way(session) {
                    "Steer"
                } else {
                    "Message"
                };
                lines.push(input_line(label, text, /*typing*/ true));
            }
            Mode::NewWorkspace { path } => {
                lines.push(input_line("Workspace", path, /*typing*/ true));
            }
            Mode::NewPrompt { text, .. } => {
                lines.push(input_line("Prompt", text, /*typing*/ true));
            }
        }
        if let Some(notice) = self.notice() {
            lines.extend(indented_wrapped_lines(&notice, 1, width, Theme::warning()));
        }
        lines
    }

    /// What the selected job's open request asks, under its row.
    fn request_lines(&self, width: usize) -> Vec<Line<'static>> {
        let Some((_, request)) = self.open_request() else {
            return Vec::new();
        };
        let asks = match &request.ask {
            Ask::Approval(ask) => ApprovalPrompt::of_daemon_ask(ask).summary(),
            Ask::PlanApproval(_) => {
                vec!["A plan waits for approval. Enter attach to read it".into()]
            }
            Ask::Question(question) => {
                vec![format!("{} Enter attach to answer", question.title)]
            }
            // `Ask` is non-exhaustive.
            _ => vec!["A request this rho cannot show. Enter attach".into()],
        };
        asks.iter()
            .flat_map(|line| indented_wrapped_lines(line, 5, width, Theme::text()))
            .collect()
    }
}

/// Hands `text` to `session`: steers its turn when one is `under_way`,
/// else starts its next. The list that said which is up to a poll old, so
/// a turn that ended or began since takes the message the other way.
async fn message(
    daemon: Arc<Daemon>,
    session: SessionId,
    text: String,
    under_way: bool,
) -> Result<(), DaemonError> {
    let steer = |daemon: Arc<Daemon>, session: SessionId, text: String| async move {
        let params = SessionSteerParams { session, text };
        daemon.request::<SessionSteer>(&params).await.map(|_| ())
    };
    let refused_as = |error: &DaemonError, code: ErrorCode| matches!(error, DaemonError::Request(error) if error.error_code() == Some(code));
    if under_way {
        match steer(Arc::clone(&daemon), session.clone(), text.clone()).await {
            Err(error) if refused_as(&error, ErrorCode::NotSteerable) => {
                prompt(daemon, session, text).await
            }
            steered => steered,
        }
    } else {
        match prompt(Arc::clone(&daemon), session.clone(), text.clone()).await {
            Err(error) if refused_as(&error, ErrorCode::SessionBusy) => {
                steer(daemon, session, text).await
            }
            prompted => prompted,
        }
    }
}

/// Starts a turn of `session` on `text`, as the person at this TUI.
async fn prompt(daemon: Arc<Daemon>, session: SessionId, text: String) -> Result<(), DaemonError> {
    let params = SessionPromptParams {
        session,
        content: vec![ContentBlock::Text { text }],
        timeout_ms: None,
        environment: ClientEnvironment::of_this_process(),
        idempotency_key: None,
        grant: None,
        author: PromptAuthor::Person,
    };
    daemon.request::<SessionPrompt>(&params).await.map(|_| ())
}

fn input_line(label: &str, text: &str, typing: bool) -> Line<'static> {
    let cursor = if typing { "█" } else { "" };
    Line::from(vec![
        Span::styled(format!(" {label}: "), Theme::dim()),
        Span::styled(format!("{text}{cursor}"), Theme::text()),
    ])
}

fn row_line(row: &Row, selected: bool, width: usize) -> Line<'static> {
    let marker = if selected { "▸" } else { " " };
    let nesting = match row.depth {
        0 => String::new(),
        depth => format!("{}└ ", "  ".repeat(depth - 1)),
    };
    let status_style = match row.status {
        Status::NeedsInput => Theme::warning().add_modifier(Modifier::BOLD),
        Status::Running => Theme::accent(),
        Status::Queued | Status::Idle | Status::Ended => Theme::dim(),
    };
    let title_style = if selected {
        Theme::text_strong().add_modifier(Modifier::BOLD)
    } else {
        Theme::text()
    };
    let lead = format!(" {marker} {:<11} ", row.status.label());
    let facts: String = row.facts.iter().map(|fact| format!(" · {fact}")).collect();
    let used = super::display_width(&lead) + super::display_width(&facts);
    let title = truncate_one_line(
        &format!("{nesting}{}", row.title),
        width.saturating_sub(used).max(1),
    );
    Line::from(vec![
        Span::styled(lead, status_style),
        Span::styled(title, title_style),
        Span::styled(facts, Theme::dim()),
    ])
}

impl PanelBody for JobsView {
    fn state(&self) -> &PanelState {
        &self.panel
    }

    fn state_mut(&mut self) -> &mut PanelState {
        &mut self.panel
    }

    fn title(&self) -> &str {
        "Jobs"
    }

    fn footer(&self) -> &str {
        match &self.mode {
            Mode::Browse => match self.open_request() {
                Some((
                    _,
                    InputRequest {
                        ask: Ask::Approval(_),
                        ..
                    },
                )) => ASK_FOOTER,
                Some(_) | None => BROWSE_FOOTER,
            },
            Mode::Filter
            | Mode::Message { .. }
            | Mode::NewWorkspace { .. }
            | Mode::NewPrompt { .. } => INPUT_FOOTER,
        }
    }

    fn body_lines(&self, width: usize, _now: Instant) -> Vec<Line<'static>> {
        self.rows(width).0
    }

    fn pinned_lines(&self, width: usize) -> Vec<Line<'static>> {
        self.pinned(width)
    }

    fn cursor_rows(&self, width: usize) -> Option<std::ops::Range<usize>> {
        self.rows(width).1
    }

    fn handle_key(&mut self, key: KeyEvent) -> PanelKeyOutcome {
        // A notice is of the request before this key.
        *self.notice.lock().unwrap_or_else(PoisonError::into_inner) = None;
        match self.mode {
            Mode::Browse => self.browse_key(key),
            Mode::Filter
            | Mode::Message { .. }
            | Mode::NewWorkspace { .. }
            | Mode::NewPrompt { .. } => self.typing_key(key),
        }
    }

    fn close(self: Box<Self>, app: &mut App) {
        app.jobs_view_attach = self.attach;
    }
}

impl App {
    /// Opens `/jobs`, which only a session the daemon runs has.
    pub(super) fn execute_jobs_command(&mut self) -> anyhow::Result<()> {
        let Some(session) = &self.info.session.daemon else {
            self.needs_daemon("jobs");
            return Ok(());
        };
        let view = JobsView::new(
            session.daemon(),
            SessionId::new(session.id()),
            session.launch().clone(),
        );
        self.input_ui
            .set_composer(ComposerMode::Panel(PanelOverlay::Jobs(Box::new(view))));
        // The first list is owed now, not at the rail's next poll.
        self.owe_daemon_jobs();
        self.set_status_quiet("jobs");
        Ok(())
    }

    pub(super) fn jobs_view_open(&self) -> bool {
        matches!(
            self.input_ui.composer(),
            ComposerMode::Panel(PanelOverlay::Jobs(_))
        )
    }

    /// Shows the daemon's `jobs` in an open `/jobs`, scrolled to the
    /// selection when the list moved it or its job began to ask.
    pub(super) fn show_jobs_view(&mut self, jobs: &[JobSummary], terminal: &DefaultTerminal) {
        if let ComposerMode::Panel(PanelOverlay::Jobs(view)) = self.input_ui.composer_mut() {
            if let (true, Some(area)) = (view.show(jobs.to_vec()), terminal_area(terminal)) {
                reveal_panel_cursor(view.as_mut(), area);
            }
        }
    }

    /// Follows the session a closed `/jobs` asked to attach to. Only
    /// between turns: a session switch leaves the turn under way behind.
    pub(super) async fn attach_from_jobs_view(&mut self, terminal: &mut DefaultTerminal) {
        let Some(Attach { session, workspace }) = self.jobs_view_attach.take() else {
            return;
        };
        // A saved session attaches as `/resume` does, with its checks. One
        // the daemon has open and has not saved yet, as a session just
        // started, has no file to find: it is followed by its id.
        let attached = match Session::workspace_of(session.as_str()) {
            Ok(Some(_)) => self.resume_daemon_session(session.as_str(), terminal).await,
            Ok(None) => {
                self.follow_unsaved_session(session, &workspace, terminal)
                    .await
            }
            // The store could not say, so no check below would hold.
            Err(error) => Err(error),
        };
        if let Err(error) = attached {
            self.set_status(format!("could not attach session: {error:#}"));
        }
    }

    /// Follows `session`, which the daemon has loaded in `workspace` and
    /// has not saved. A launch that named an agent follows only that
    /// agent's sessions, and an unsaved session's agent is recorded
    /// nowhere this TUI reads, so such a launch refuses it until it saves.
    async fn follow_unsaved_session(
        &mut self,
        session: SessionId,
        workspace: &std::path::Path,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.ensure_this_project(workspace)?;
        anyhow::ensure!(
            self.info.session.resume_launch.definition.is_none(),
            "the session has saved nothing yet, so its agent cannot be checked against --agent; \
             attach once its first turn saves"
        );
        let short = super::session_picker::short_session_id(session.as_str());
        self.switch_daemon_session(DaemonTarget::Resume(session), terminal)
            .await?;
        self.set_status(format!("attached session {short}"));
        Ok(())
    }

    /// Drops what a `/jobs` closed during a turn asked for, saying why.
    pub(super) fn refuse_jobs_view_attach(&mut self) {
        if self.jobs_view_attach.take().is_some() {
            self.set_status("could not attach session: a model turn is running here");
        }
    }
}
