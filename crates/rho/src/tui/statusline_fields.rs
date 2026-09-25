//! Bottom-row field paint: spans, hover emphasis, and click hit spans.
//!
//! Hit spans are recorded while the spans are emitted, so pointer hits always
//! match the painted columns, including after rank drops and truncation.

use std::ops::Range;

use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

use super::{display_width, side_width, FieldKey, StatusField, Theme, FIELD_SEP};

/// Painted column span of one clickable field on [`super::FIELDS_ROW`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct StatusFieldHit {
    pub(super) key: FieldKey,
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
}

/// Click policy per field: open whatever inspects or changes what the field
/// shows. Ambient markers without a chooser stay inert.
pub(super) fn click_action(key: FieldKey) -> Option<StatusClick> {
    match key {
        FieldKey::Context | FieldKey::Cost | FieldKey::Rate => Some(StatusClick::Command("/info")),
        FieldKey::Permission => Some(StatusClick::PermissionMode),
        FieldKey::Computer => Some(StatusClick::Command("/computer")),
        FieldKey::Provider | FieldKey::Model | FieldKey::Reasoning => {
            Some(StatusClick::Command("/model"))
        }
        FieldKey::SignedOut | FieldKey::LoginHint => Some(StatusClick::Command("/login")),
        FieldKey::Zen | FieldKey::NotSaved => None,
    }
}

pub(super) fn status_fields_line(
    left: &[StatusField],
    right: &[StatusField],
    width: usize,
    hovered: Option<FieldKey>,
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
    hovered: Option<FieldKey>,
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
        for (index, field) in fields.iter().enumerate() {
            if index > 0 {
                self.push(Span::styled(FIELD_SEP.to_string(), Theme::dim()));
            }
            let start = self.column;
            // Only clickable fields react to hover. The lift keeps the
            // field's severity color and adds emphasis, so widths never move.
            let style = if field.action.is_some() && self.hovered == Some(field.key) {
                field
                    .style
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                field.style
            };
            self.push(Span::styled(field.text.clone(), style));
            if let Some(action) = field.action.filter(|_| self.column > start) {
                self.hits.push(StatusFieldHit {
                    key: field.key,
                    columns: start..self.column,
                    action,
                });
            }
        }
    }
}
