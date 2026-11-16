//! The surfaces in the running TUI: showing what `ui` calls carry, moving
//! each surface from its live card to the transcript, and the keyboard.

use std::{collections::HashMap, sync::Arc};

use crossterm::event::KeyEvent;
use rho_a2ui::{Action, Effect, Pointer, RendererMessage};
use rho_sdk::ToolCallId;
use serde_json::Value;

use super::{
    focus::{is_focus_chord, EditKey, NavKey},
    tree::{Control, FieldKind, Focusable, SurfaceTree, Target},
    Editing, Keyboard, SurfaceEntry, SurfaceFocus, SurfaceView, Surfaces,
};
use crate::tui::{line_editor::LineEditor, tool_call_batch::LiveToolKey, App, ComposerMode, Entry};

impl App {
    /// Shows the surface a `ui` call carries. Every update of the call
    /// repeats it; only the first is applied.
    pub(in crate::tui) fn show_surface(
        &mut self,
        call: ToolCallId,
        surface: String,
        messages: &[Value],
        replayed: bool,
    ) {
        if !self.surfaces.store.apply(&call, &surface, messages) {
            return;
        }
        let entry = SurfaceEntry {
            call,
            surface,
            view: None,
        };
        if self.is_live_call(&entry.call) {
            // A call that shows its surface while still open waits for the
            // user to act on it, so the keyboard goes there unless the user
            // is busy in the composer. A replay shows every call open
            // first, whether or not it waited.
            let composer_free = matches!(self.input_ui.composer(), ComposerMode::Input)
                && !self.input_ui.has_pending_draft();
            if !replayed && composer_free && !self.surfaces.has_keyboard() {
                self.surfaces.focus = SurfaceFocus::Navigating {
                    surface: entry.surface.clone(),
                    index: 0,
                };
            }
            self.surfaces.live.push(entry);
        } else {
            self.insert_entry(&Entry::Surface(entry));
        }
        self.sync_surfaces();
    }

    /// Moves the surface of each call that left the live tool batch to
    /// the transcript, where the call's card just went.
    pub(in crate::tui) fn settle_surfaces(&mut self) {
        let (live, settled) = std::mem::take(&mut self.surfaces.live)
            .into_iter()
            .partition(|entry| self.is_live_call(&entry.call));
        self.surfaces.live = live;
        let settled: Vec<SurfaceEntry> = settled;
        for entry in settled {
            self.insert_entry(&Entry::Surface(entry));
        }
    }

    /// Forgets every surface, as a reset replay starts the session over.
    pub(in crate::tui) fn clear_surfaces(&mut self) {
        self.surfaces = Surfaces::default();
    }

    fn is_live_call(&self, call: &ToolCallId) -> bool {
        self.turn
            .tool_calls()
            .live_cards()
            .any(|(key, _)| matches!(&key, LiveToolKey::Running(running) if running == call))
    }

    /// Takes a new snapshot of each surface for the entry that draws it,
    /// after anything that changes one: messages, an edit, the keyboard
    /// moving. Entries whose snapshot is unchanged keep their lines.
    fn sync_surfaces(&mut self) {
        let mut trees: HashMap<String, Option<Arc<SurfaceTree>>> = HashMap::new();
        let store = &self.surfaces.store;
        let mut tree_of = |surface: &str| {
            trees
                .entry(surface.to_owned())
                .or_insert_with(|| store.tree(surface).map(Arc::new))
                .clone()
        };
        // The keyboard stays on a surface only while it has that control.
        if let Some((surface, index)) = self.surfaces.focus.place() {
            let controls = tree_of(surface).map_or(0, |tree| tree.focusables.len());
            if index >= controls {
                self.surfaces.focus = SurfaceFocus::Idle;
            }
        }
        let reachable = store
            .latest_first()
            .find(|surface| tree_of(surface).is_some_and(|tree| !tree.focusables.is_empty()))
            .map(str::to_owned);
        let focus = &self.surfaces.focus;
        let mut view_of = |entry: &SurfaceEntry| {
            if store.drawn_under(&entry.surface) != Some(&entry.call) {
                return None;
            }
            let keyboard = match focus {
                SurfaceFocus::Navigating { surface, index } if *surface == entry.surface => {
                    Keyboard::On {
                        index: *index,
                        editing: None,
                    }
                }
                SurfaceFocus::Editing {
                    surface,
                    index,
                    editor,
                    invalid,
                } if *surface == entry.surface => Keyboard::On {
                    index: *index,
                    editing: Some(Editing {
                        text: editor.value.clone(),
                        cursor: editor.cursor,
                        invalid: *invalid,
                    }),
                },
                SurfaceFocus::Idle
                | SurfaceFocus::Navigating { .. }
                | SurfaceFocus::Editing { .. } => Keyboard::Away {
                    reachable: reachable.as_deref() == Some(entry.surface.as_str()),
                },
            };
            Some(SurfaceView {
                tree: tree_of(&entry.surface)?,
                keyboard,
            })
        };
        for entry in &mut self.surfaces.live {
            entry.view = view_of(entry);
        }
        let mut redraw = Vec::new();
        let mut rebuild_from = None;
        for (index, entry) in self.history.entries_mut().iter_mut().enumerate() {
            let Entry::Surface(entry) = entry else {
                continue;
            };
            let view = view_of(entry);
            if entry.view == view {
                continue;
            }
            // An entry that starts or stops drawing changes which rows
            // exist, which only a rebuild from there accounts for.
            if entry.view.is_some() == view.is_some() {
                redraw.push(index);
            } else {
                rebuild_from.get_or_insert(index);
            }
            entry.view = view;
        }
        if let Some(index) = rebuild_from {
            self.history.invalidate_from(index);
        }
        self.history.lines_mut().resplice_entries(redraw);
    }

    /// Moves the keyboard from the composer to the latest surface that
    /// has a control, if `key` is the chord for it.
    pub(in crate::tui) fn handle_surface_focus_chord(&mut self, key: KeyEvent) -> bool {
        if !is_focus_chord(key) {
            return false;
        }
        let store = &self.surfaces.store;
        let surface = store
            .latest_first()
            .find(|surface| {
                store
                    .tree(surface)
                    .is_some_and(|tree| !tree.focusables.is_empty())
            })
            .map(str::to_owned);
        match surface {
            Some(surface) => {
                self.surfaces.focus = SurfaceFocus::Navigating { surface, index: 0 };
                self.sync_surfaces();
                // The latest surface is near the end of the transcript.
                self.scroll_history_to_bottom();
            }
            None => self.notify_status("no surface to interact with"),
        }
        true
    }

    /// Pastes `text` into the field being edited, if one is.
    pub(in crate::tui) fn paste_into_surface(&mut self, text: &str) -> bool {
        let SurfaceFocus::Editing { editor, .. } = &mut self.surfaces.focus else {
            return false;
        };
        editor.insert_text(text);
        self.write_edited_field();
        self.sync_surfaces();
        true
    }

    /// Handles `key` while the keyboard is on a surface. Keys a surface
    /// has no use for, such as the scroll keys and Ctrl+C, are left to
    /// the rest of the TUI.
    pub(in crate::tui) async fn handle_surface_key(&mut self, key: KeyEvent) -> bool {
        if !matches!(self.input_ui.composer(), ComposerMode::Input) {
            return false;
        }
        let Some((surface, index)) = self.surfaces.focus.place() else {
            return false;
        };
        let surface = surface.to_owned();
        let Some(Focusable { target, control }) =
            self.surfaces.store.tree(&surface).and_then(|mut tree| {
                (index < tree.focusables.len()).then(|| tree.focusables.remove(index))
            })
        else {
            self.surfaces.focus = SurfaceFocus::Idle;
            self.sync_surfaces();
            return false;
        };
        let handled = if matches!(self.surfaces.focus, SurfaceFocus::Editing { .. }) {
            self.edit_field(key, &control)
        } else {
            match NavKey::of(key) {
                Some(nav) => {
                    self.navigate(nav, &surface, index, &target, control).await;
                    true
                }
                None => false,
            }
        };
        if handled {
            self.input_ui.clear_paste_burst();
            self.ctrl_c_streak = 0;
            self.sync_surfaces();
        }
        handled
    }

    fn edit_field(&mut self, key: KeyEvent, control: &Control) -> bool {
        let newline = self.info.runtime.keybindings.insert_newline.matches(key);
        let SurfaceFocus::Editing {
            surface,
            index,
            editor,
            ..
        } = &mut self.surfaces.focus
        else {
            return false;
        };
        let kind = match control {
            Control::Field { kind, .. } => *kind,
            _ => FieldKind::Short,
        };
        let outcome = if newline && kind == FieldKind::Long {
            let at = editor
                .value
                .char_indices()
                .nth(editor.cursor)
                .map_or(editor.value.len(), |(at, _)| at);
            editor.value.insert(at, '\n');
            editor.cursor += 1;
            Some(EditKey::Edited)
        } else {
            EditKey::apply(key, editor, |ch| kind.accepts(ch))
        };
        match outcome {
            None => return false,
            Some(EditKey::Edited) => self.write_edited_field(),
            Some(EditKey::Done) => {
                self.surfaces.focus = SurfaceFocus::Navigating {
                    surface: std::mem::take(surface),
                    index: *index,
                };
            }
            Some(EditKey::DoneThen(nav)) => {
                let (surface, index) = (std::mem::take(surface), *index);
                self.surfaces.focus = SurfaceFocus::Navigating {
                    surface: surface.clone(),
                    index,
                };
                self.move_focus(&surface, index, nav == NavKey::Next);
            }
        }
        true
    }

    /// Writes the text being edited to the field's place in the data
    /// model, so checks and bindings follow each keystroke. Text the
    /// field's kind does not allow is kept out of the model.
    fn write_edited_field(&mut self) {
        let SurfaceFocus::Editing {
            surface,
            index,
            editor,
            invalid,
        } = &mut self.surfaces.focus
        else {
            return;
        };
        let Some(Control::Field { binding, kind }) = self
            .surfaces
            .store
            .tree(surface)
            .and_then(|mut tree| {
                (*index < tree.focusables.len()).then(|| tree.focusables.remove(*index))
            })
            .map(|focusable| focusable.control)
        else {
            return;
        };
        *invalid = kind.problem(&editor.value);
        if invalid.is_none() {
            let value = Value::String(editor.value.clone());
            self.surfaces.store.write(surface, &binding, value);
        }
    }

    async fn navigate(
        &mut self,
        nav: NavKey,
        surface: &str,
        index: usize,
        target: &Target,
        control: Control,
    ) {
        match (nav, control) {
            (NavKey::Leave, _) => self.surfaces.focus = SurfaceFocus::Idle,
            (NavKey::Next, _) => self.move_focus(surface, index, /*forward*/ true),
            (NavKey::Previous, _) => self.move_focus(surface, index, /*forward*/ false),
            (
                NavKey::Left | NavKey::Right,
                Control::Slider {
                    binding,
                    min,
                    max,
                    step,
                },
            ) => {
                let value = self.read_number(surface, &binding).unwrap_or(min);
                let step = if nav == NavKey::Right { step } else { -step };
                let value = (value + step).clamp(min, max);
                self.surfaces
                    .store
                    .write(surface, &binding, number_value(value));
            }
            (NavKey::Left | NavKey::Right | NavKey::Activate, Control::Tabs { count })
                if count > 0 =>
            {
                let active = self.surfaces.store.local(surface, target).min(count - 1);
                let next = if nav == NavKey::Left {
                    (active + count - 1) % count
                } else {
                    (active + 1) % count
                };
                self.surfaces.store.set_local(surface, target, next);
            }
            (NavKey::Left, _) => self.move_focus(surface, index, /*forward*/ false),
            (NavKey::Right, _) => self.move_focus(surface, index, /*forward*/ true),
            (
                NavKey::Activate,
                Control::Button {
                    action: Some(action),
                },
            ) => {
                self.run_surface_action(surface, target, &action).await;
            }
            (NavKey::Activate, Control::Button { action: None }) => {
                self.notify_status("the button is disabled until its checks pass");
            }
            (NavKey::Activate, Control::Modal) => {
                let open = self.surfaces.store.local(surface, target) != 0;
                self.surfaces
                    .store
                    .set_local(surface, target, usize::from(!open));
            }
            (NavKey::Activate, Control::CheckBox { binding }) => {
                let checked =
                    self.surfaces.store.read(surface, &binding) == Some(&Value::Bool(true));
                self.surfaces
                    .store
                    .write(surface, &binding, Value::Bool(!checked));
            }
            (
                NavKey::Activate,
                Control::Choice {
                    binding,
                    value,
                    multiple,
                },
            ) => {
                let mut selected: Vec<Value> = self
                    .surfaces
                    .store
                    .read(surface, &binding)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let value = Value::String(value);
                let was_selected = selected.contains(&value);
                if !multiple {
                    selected = vec![value];
                } else if was_selected {
                    selected.retain(|other| *other != value);
                } else {
                    selected.push(value);
                }
                self.surfaces
                    .store
                    .write(surface, &binding, Value::Array(selected));
            }
            (NavKey::Activate, Control::Field { binding, .. }) => {
                self.start_editing(surface, index, &binding, None);
            }
            (NavKey::Type(ch), Control::Field { binding, kind }) if kind.accepts(ch) => {
                self.start_editing(surface, index, &binding, Some(ch));
            }
            (NavKey::Activate, Control::Slider { .. } | Control::Tabs { .. })
            | (NavKey::Type(_), _) => {}
        }
    }

    fn move_focus(&mut self, surface: &str, index: usize, forward: bool) {
        let controls = self
            .surfaces
            .store
            .tree(surface)
            .map_or(0, |tree| tree.focusables.len());
        if controls == 0 {
            return;
        }
        let index = if forward {
            (index + 1) % controls
        } else {
            (index + controls - 1) % controls
        };
        self.surfaces.focus = SurfaceFocus::Navigating {
            surface: surface.to_owned(),
            index,
        };
    }

    fn start_editing(
        &mut self,
        surface: &str,
        index: usize,
        binding: &Pointer,
        typed: Option<char>,
    ) {
        let text = match self.surfaces.store.read(surface, binding) {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        let mut editor = LineEditor::new(text);
        if let Some(typed) = typed {
            editor.insert_char(typed);
        }
        self.surfaces.focus = SurfaceFocus::Editing {
            surface: surface.to_owned(),
            index,
            editor,
            invalid: None,
        };
        if typed.is_some() {
            self.write_edited_field();
        }
    }

    fn read_number(&self, surface: &str, binding: &Pointer) -> Option<f64> {
        self.surfaces.store.read(surface, binding)?.as_f64()
    }

    /// Runs the action of the button the user pressed: an event goes to
    /// the session, and the keyboard returns to the composer, where the
    /// model's reply is followed; a function runs here.
    async fn run_surface_action(&mut self, surface: &str, target: &Target, action: &Action) {
        let Some(dispatched) = self.surfaces.store.dispatch(surface, target, action) else {
            return;
        };
        for message in &dispatched.outbound {
            match message {
                RendererMessage::Action(_) => {
                    self.info
                        .session
                        .daemon
                        .surface_action(
                            surface.to_owned(),
                            message.to_json(),
                            dispatched.data_model.clone(),
                        )
                        .await;
                    self.surfaces.focus = SurfaceFocus::Idle;
                }
                RendererMessage::Error(error) => self.insert_entry(&Entry::Error(format!(
                    "could not run the surface action: {}",
                    error.message()
                ))),
                // Only an agent's function call is answered with one.
                RendererMessage::FunctionResponse(_) => {}
            }
        }
        for effect in dispatched.effects {
            match effect {
                // A terminal has no say over which browser opens what, so
                // the link is shown for the user to open.
                Effect::OpenUrl(url) => {
                    self.insert_entry(&Entry::Notice(format!("link from the surface: {url}")));
                }
                // The processor applied it to the data model.
                Effect::SetData { .. } => {}
            }
        }
    }
}

impl FieldKind {
    /// Whether typing `ch` into a field of this kind does anything.
    fn accepts(self, ch: char) -> bool {
        match self {
            Self::Number => ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+' | 'e' | 'E'),
            Self::DateTime { .. } => ch.is_ascii_digit() || matches!(ch, '-' | ':' | 'T' | 'Z'),
            Self::Short | Self::Long | Self::Obscured => true,
        }
    }

    /// Why `text` cannot be the field's value, if it cannot. An empty
    /// field is always allowed: whether it may stay empty is for the
    /// surface's own checks to say.
    pub(super) fn problem(self, text: &str) -> Option<&'static str> {
        use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
        let Self::DateTime { date, time } = self else {
            return None;
        };
        if text.is_empty() {
            return None;
        }
        let seconds = text.matches(':').count() == 2;
        match (date, time) {
            (true, true) | (false, false) => {
                let format = if seconds {
                    "%Y-%m-%dT%H:%M:%S"
                } else {
                    "%Y-%m-%dT%H:%M"
                };
                let padded = text.len() == if seconds { 19 } else { 16 };
                (!padded || NaiveDateTime::parse_from_str(text, format).is_err())
                    .then_some("enter a date and time as YYYY-MM-DDTHH:MM")
            }
            (true, false) => (text.len() != 10
                || NaiveDate::parse_from_str(text, "%Y-%m-%d").is_err())
            .then_some("enter a date as YYYY-MM-DD"),
            (false, true) => {
                let format = if seconds { "%H:%M:%S" } else { "%H:%M" };
                let padded = text.len() == if seconds { 8 } else { 5 };
                (!padded || NaiveTime::parse_from_str(text, format).is_err())
                    .then_some("enter a time as HH:MM")
            }
        }
    }
}

/// A slider's value as JSON: an integer when it is whole.
fn number_value(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        Value::from(value as i64)
    } else {
        Value::from(value)
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
