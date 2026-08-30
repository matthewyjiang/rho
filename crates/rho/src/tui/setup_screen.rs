//! Full-screen setup shown on a first launch.
//!
//! A first launch has nothing to read and nothing to say, so the session
//! chrome is noise: history is empty, the statusline names a model the user
//! never chose, and the hints describe a composer they cannot use yet. The
//! setup screen replaces all of it with two steps, sign in and choose a model,
//! and hands off to the normal session once they are done.
//!
//! The steps drive the existing pickers rather than a parallel UI. Login and
//! model selection keep their own flows; this module owns only where the user
//! is in the sequence and how the screen is laid out.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

use super::{
    exclusive_screen::ExclusiveOccupant,
    first_run::SetupEntry,
    render::{display_width, truncate_one_line},
    theme::Theme,
    App, ComposerMode,
};

/// Widest content column the screen uses. Wider terminals centre this rather
/// than stretching the copy across the full width.
const CONTENT_WIDTH: u16 = 88;

/// Rows of empty space above the welcome block.
const TOP_PADDING: u16 = 2;

/// Where the user is in the first-launch sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SetupStep {
    /// Choosing a provider and finishing its login.
    SignIn,
    /// Choosing the model the session starts with.
    ChooseModel,
}

impl SetupStep {
    fn index(self) -> usize {
        match self {
            Self::SignIn => 0,
            Self::ChooseModel => 1,
        }
    }
}

/// One row of the step list, and how far the user has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepState {
    Done,
    Current,
    Pending,
}

impl StepState {
    fn marker(self) -> &'static str {
        match self {
            Self::Done => "✓",
            Self::Current => "▸",
            Self::Pending => " ",
        }
    }

    fn style(self) -> Style {
        match self {
            Self::Done => Theme::success(),
            Self::Current => Theme::accent(),
            Self::Pending => Theme::dim(),
        }
    }
}

const STEP_LABELS: [&str; 2] = ["Sign in to a provider", "Choose a model"];

/// Shown when Esc backs out of setup. Hidden while a login overlay owns Esc.
const SETUP_SKIP_HINT: &str = "Esc to skip setup";

impl App {
    pub(super) fn setup_step(&self) -> Option<SetupStep> {
        self.exclusive.setup_step()
    }

    fn enter_setup(&mut self, step: SetupStep) {
        self.exclusive = ExclusiveOccupant::Setup(step);
    }

    fn leave_setup(&mut self) {
        if matches!(self.exclusive, ExclusiveOccupant::Setup(_)) {
            self.exclusive = ExclusiveOccupant::Session;
        }
    }

    /// Open the setup screen at the step this launch asked for.
    ///
    /// [`SetupEntry::Auto`] picks the first step that can do anything: models
    /// come from the credentials available to the session, stored or from the
    /// environment, so a launch with none to offer starts at sign-in, and one
    /// that can already list models skips a login that is done. The named
    /// entries override that so either step can be opened on demand.
    pub(super) fn start_setup_screen(&mut self, terminal: &mut super::DefaultTerminal) {
        let Some(entry) = self.info.services.first_run else {
            return;
        };
        let step = match entry {
            SetupEntry::SignIn => SetupStep::SignIn,
            SetupEntry::ChooseModel => SetupStep::ChooseModel,
            SetupEntry::Auto if self.setup_model_picker().is_some() => SetupStep::ChooseModel,
            SetupEntry::Auto => SetupStep::SignIn,
        };
        self.enter_setup(step);
        match step {
            SetupStep::SignIn => self.open_login_picker(),
            SetupStep::ChooseModel => self.open_setup_model_picker(terminal),
        }
    }

    /// Move from sign-in to the model step once a login succeeds.
    pub(super) fn advance_setup_screen_after_login(
        &mut self,
        terminal: &mut super::DefaultTerminal,
    ) {
        match self.setup_step() {
            Some(SetupStep::SignIn) => {
                self.enter_setup(SetupStep::ChooseModel);
                self.open_setup_model_picker(terminal);
            }
            // A login from the model step or from a normal session changes
            // credentials, not which step the user is on.
            Some(SetupStep::ChooseModel) | None => {}
        }
    }

    /// Close the screen once a model is live. The session takes over from here.
    ///
    /// The status is left as the caller set it, so a config-save failure during
    /// the model switch still reaches the user.
    pub(super) fn finish_setup_screen(&mut self) {
        match self.setup_step() {
            Some(SetupStep::ChooseModel) => self.leave_setup(),
            Some(SetupStep::SignIn) | None => {}
        }
    }

    /// Leave setup when the user backs out of its picker.
    ///
    /// Called from the one place a picker collapses to the plain composer, so
    /// Esc always leads somewhere instead of stranding an empty screen. Every
    /// step exits the same way, so this needs no per-step handling.
    ///
    /// Distinct from [`Self::restore_after_cancelled_login`]: Esc on the
    /// picker leaves setup, Esc on a pending login stays here and reopens
    /// this step's picker.
    pub(super) fn dismiss_setup_screen(&mut self) {
        self.leave_setup();
    }

    /// Reopen the current setup step after a login overlay is cancelled.
    ///
    /// Escaping a pending login, API-key prompt, or custom-host step is not
    /// the same as escaping the picker: the picker Esc path calls
    /// [`Self::dismiss_setup_screen`] and leaves setup entirely. This path
    /// keeps `exclusive` as Setup and restores that step's picker. A normal
    /// `/login` (no setup step) still returns to the plain composer.
    pub(super) fn restore_after_cancelled_login(&mut self) {
        match self.setup_step() {
            Some(SetupStep::SignIn) => self.open_login_picker(),
            Some(SetupStep::ChooseModel) => self.show_setup_model_picker(),
            None => {
                self.input_ui.set_composer(ComposerMode::Input);
                self.set_status("login cancelled");
            }
        }
    }

    /// The model picker for this session, or `None` when the available
    /// credentials offer no models to choose between.
    fn setup_model_picker(&mut self) -> Option<super::UiPicker> {
        self.refresh_available_auths();
        let picker = self.conversation_model_picker();
        (!picker.items.is_empty()).then_some(picker)
    }

    /// Open the model picker without the `/model` command's loading redraw,
    /// which would paint session chrome over the setup screen.
    fn open_setup_model_picker(&mut self, terminal: &mut super::DefaultTerminal) {
        self.show_setup_model_picker();
        let _ = terminal.draw(|frame| self.draw(frame));
    }

    fn show_setup_model_picker(&mut self) {
        let Some(picker) = self.setup_model_picker() else {
            // Nothing to choose between: keep the configured model rather than
            // showing an empty step. Setup is gone, so the composer must
            // return to Input instead of keeping a cancelled login overlay.
            self.leave_setup();
            self.input_ui.set_composer(ComposerMode::Input);
            self.set_status("ready");
            return;
        };
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("select model");
    }

    pub(super) fn draw_setup_screen(&mut self, frame: &mut Frame<'_>, area: Rect, step: SetupStep) {
        frame.render_widget(Clear, area);
        frame.render_widget(
            ratatui::widgets::Paragraph::new("").style(Theme::surface()),
            area,
        );
        let column = content_column(area);
        if column.height == 0 {
            return;
        }
        let width = column.width as usize;

        let origin = setup_composer_origin(area, step);
        let body_row = origin.y.saturating_sub(column.y);
        let mut lines = welcome_lines(width);
        lines.extend(step_lines(step, width));
        lines.push(Line::raw(""));
        lines.extend(self.setup_body_lines(width, origin.height));
        if let Some(hint) = setup_skip_hint(self.input_ui.composer()) {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                truncate_one_line(hint, width),
                Theme::dim(),
            )));
        }

        frame.render_widget(Paragraph::new(lines).style(Theme::surface()), column);
        if let Some(position) = self.setup_filter_cursor(column, body_row) {
            frame.set_cursor_position(position);
        }
    }

    /// The active picker, or a progress line while a login is in flight.
    fn setup_body_lines(&mut self, width: usize, height: u16) -> Vec<Line<'static>> {
        match self.input_ui.composer() {
            // Between two pickers the composer is briefly plain. Report what
            // the session is doing instead of the "type a message" prompt.
            ComposerMode::Input => vec![Line::from(Span::styled(
                truncate_one_line(self.status(), width),
                Theme::dim(),
            ))],
            _ => self.composer_frame(width, height as usize).lines,
        }
    }

    /// The picker's filter cursor, placed on the first body row.
    fn setup_filter_cursor(
        &self,
        column: Rect,
        body_row: u16,
    ) -> Option<ratatui::layout::Position> {
        let ComposerMode::Picker(picker) = self.input_ui.composer() else {
            return None;
        };
        let offset = display_width(&picker.filter).saturating_add(2);
        Some(ratatui::layout::Position {
            x: column
                .x
                .saturating_add(offset.min(column.width.saturating_sub(1) as usize) as u16),
            y: column.y.saturating_add(body_row),
        })
    }
}

/// Skip-setup footer, or none while a login overlay owns Esc.
pub(super) fn setup_skip_hint(composer: &ComposerMode) -> Option<&'static str> {
    composer
        .setup_escape_leaves_setup()
        .then_some(SETUP_SKIP_HINT)
}

/// Where the composer body is painted on the setup screen.
pub(super) fn setup_composer_origin(area: Rect, step: SetupStep) -> Rect {
    let column = content_column(area);
    let width = column.width as usize;
    let body_row = (welcome_lines(width).len() + step_lines(step, width).len() + 1) as u16;
    Rect {
        x: column.x,
        y: column.y.saturating_add(body_row),
        width: column.width,
        height: column.height.saturating_sub(body_row),
    }
}

/// Centre the content column so wide terminals do not stretch the copy.
fn content_column(area: Rect) -> Rect {
    let width = area.width.min(CONTENT_WIDTH);
    Rect {
        x: area.x.saturating_add(area.width.saturating_sub(width) / 2),
        y: area.y.saturating_add(TOP_PADDING),
        width,
        height: area.height.saturating_sub(TOP_PADDING),
    }
}

fn welcome_lines(width: usize) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled("rho", Theme::brand()),
            Span::styled("  v", Theme::dim()),
            Span::styled(super::smoke_injection::display_version(), Theme::success()),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            truncate_one_line("Welcome. Two steps and you are ready to work.", width),
            Theme::text_strong(),
        )),
        Line::raw(""),
    ]
}

fn step_lines(step: SetupStep, width: usize) -> Vec<Line<'static>> {
    STEP_LABELS
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let state = match index.cmp(&step.index()) {
                std::cmp::Ordering::Less => StepState::Done,
                std::cmp::Ordering::Equal => StepState::Current,
                std::cmp::Ordering::Greater => StepState::Pending,
            };
            Line::from(Span::styled(
                truncate_one_line(&format!("{} {label}", state.marker()), width),
                state.style(),
            ))
        })
        .collect()
}

#[cfg(test)]
#[path = "setup_screen_tests.rs"]
mod tests;
