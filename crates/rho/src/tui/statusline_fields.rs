//! Bottom-row field paint: spans, hover emphasis, and click hit spans.
//!
//! Hit spans are recorded while the spans are emitted, so pointer hits always
//! match the painted columns, including after rank drops and truncation.
//! Neighboring fields with the same click action form one target: one hit
//! spans both and the separator between them, and hover lifts them together.

use std::ops::Range;

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::{display_width, side_width, FieldKey, StatusField, Theme, FIELD_SEP};

/// Painted column span of one click target on [`super::FIELDS_ROW`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct StatusFieldHit {
    /// Row-relative display columns, clipped to the row width.
    pub(in crate::tui) columns: Range<usize>,
    pub(in crate::tui) action: StatusClick,
}

/// What a click on a status field does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum StatusClick {
    /// Run a slash command as if typed and submitted.
    Command(&'static str),
    /// Open the config picker on the permission mode row. `/permissions`
    /// without an argument only prints the mode, so it is not a chooser.
    PermissionMode,
    /// Step the reasoning effort to the next level the model accepts.
    CycleReasoning,
}

/// Click policy per field: open whatever inspects or changes what the field
/// shows. Provider and model share the model picker, so they read as one
/// target; reasoning changes in place. Ambient markers without a chooser stay
/// inert.
pub(super) fn click_action(key: FieldKey) -> Option<StatusClick> {
    match key {
        FieldKey::Context | FieldKey::Cost | FieldKey::Rate => Some(StatusClick::Command("/info")),
        FieldKey::Permission => Some(StatusClick::PermissionMode),
        FieldKey::Provider | FieldKey::Model => Some(StatusClick::Command("/model")),
        FieldKey::Reasoning => Some(StatusClick::CycleReasoning),
        FieldKey::SignedOut | FieldKey::LoginHint => Some(StatusClick::Command("/login")),
        FieldKey::Zen | FieldKey::NotSaved => None,
    }
}

pub(super) fn status_fields_line(
    left: &[StatusField],
    right: &[StatusField],
    width: usize,
    hovered: Option<StatusClick>,
) -> (Line<'static>, Vec<StatusFieldHit>) {
    let mut row = FieldRow {
        hovered,
        ..FieldRow::default()
    };
    row.push_fields(left);
    if !right.is_empty() {
        let gap = width.saturating_sub(side_width(left) + side_width(right));
        row.push(Span::styled(" ".repeat(gap), Theme::dim()));
        row.push_fields(right);
    }
    let hits = row
        .hits
        .into_iter()
        .filter_map(|mut hit| {
            hit.columns = hit.columns.start.min(width)..hit.columns.end.min(width);
            (!hit.columns.is_empty()).then_some(hit)
        })
        .collect();
    (Line::from(row.spans), hits)
}

/// Spans of one status row plus the clickable spans recorded while emitting.
#[derive(Default)]
struct FieldRow {
    hovered: Option<StatusClick>,
    spans: Vec<Span<'static>>,
    column: usize,
    hits: Vec<StatusFieldHit>,
}

impl FieldRow {
    fn push(&mut self, span: Span<'static>) {
        self.column += display_width(span.content.as_ref());
        self.spans.push(span);
    }

    fn push_fields(&mut self, fields: &[StatusField]) {
        let mut previous: Option<StatusClick> = None;
        for (index, field) in fields.iter().enumerate() {
            let joined = field.action.is_some() && field.action == previous;
            if index > 0 {
                let style = if joined {
                    self.hover_style(Theme::dim(), field.action)
                } else {
                    Theme::dim()
                };
                self.push(Span::styled(FIELD_SEP.to_string(), style));
            }
            let start = self.column;
            self.push(Span::styled(
                field.text.clone(),
                self.hover_style(field.style, field.action),
            ));
            previous = field.action;
            let Some(action) = field.action.filter(|_| self.column > start) else {
                continue;
            };
            match self.hits.last_mut() {
                Some(hit) if joined && hit.action == action => hit.columns.end = self.column,
                _ => self.hits.push(StatusFieldHit {
                    columns: start..self.column,
                    action,
                }),
            }
        }
    }

    /// Hover lift for a span of the target with `action`. It keeps the
    /// span's severity color and adds emphasis, so widths never move.
    fn hover_style(&self, style: Style, action: Option<StatusClick>) -> Style {
        if action.is_some() && self.hovered == action {
            style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            style
        }
    }
}
