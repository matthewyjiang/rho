//! Generic labeled rules for composer chrome.
//!
//! Feature code supplies captions. This module only fits them onto a rule.
//!
//! When both sides have fallbacks, keep the longest right caption that still
//! fits (advisor identity) and shrink the left side first. Then degrade the
//! right caption, then drop it, then drop the left.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::render::display_width;

/// Longest-first captions for one side of a divider.
pub(super) struct DividerCaption {
    candidates: Vec<String>,
    style: Style,
}

impl DividerCaption {
    pub(super) fn new(
        candidates: impl IntoIterator<Item = impl Into<String>>,
        style: Style,
    ) -> Option<Self> {
        let candidates: Vec<String> = candidates
            .into_iter()
            .map(Into::into)
            .filter(|text| !text.is_empty())
            .collect();
        (!candidates.is_empty()).then_some(Self { candidates, style })
    }
}

const RULE: &str = "─";
const LEFT_PREFIX: &str = "─ ";
const MIN_FILL: usize = 2;
const MIN_RIGHT_SUFFIX: usize = 1;

type FittedCaption<'a> = Option<(&'a str, Style)>;

/// Paint a full-width rule, with optional left and right captions.
///
/// Left captions keep the existing `─ label ────` shape. Right captions sit
/// flush against a short trailing rule: `──── label ─`. Width scarcity shrinks
/// the left caption first, then the right, then drops the right, then the left.
pub(super) fn labeled_divider_line(
    left: Option<DividerCaption>,
    right: Option<DividerCaption>,
    rule_style: Style,
    width: usize,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    let (left, right) = fit_captions(left.as_ref(), right.as_ref(), width);
    paint(left, right, rule_style, width)
}

fn fit_captions<'a>(
    left: Option<&'a DividerCaption>,
    right: Option<&'a DividerCaption>,
    width: usize,
) -> (FittedCaption<'a>, FittedCaption<'a>) {
    let lefts = caption_choices(left);
    let rights = caption_choices(right);
    for &(right, right_style) in &rights {
        for &(left, left_style) in &lefts {
            if min_width(Some(left), Some(right)) <= width {
                return (Some((left, left_style)), Some((right, right_style)));
            }
        }
    }
    for &(left, left_style) in &lefts {
        if min_width(Some(left), None) <= width {
            return (Some((left, left_style)), None);
        }
    }
    for &(right, right_style) in &rights {
        if min_width(None, Some(right)) <= width {
            return (None, Some((right, right_style)));
        }
    }
    (None, None)
}

fn caption_choices(caption: Option<&DividerCaption>) -> Vec<(&str, Style)> {
    let Some(caption) = caption else {
        return Vec::new();
    };
    caption
        .candidates
        .iter()
        .map(|text| (text.as_str(), caption.style))
        .collect()
}

fn left_fixed(label: &str) -> usize {
    display_width(LEFT_PREFIX) + display_width(label) + 1
}

fn right_fixed(label: &str) -> usize {
    1 + display_width(label) + 1 + MIN_RIGHT_SUFFIX
}

fn min_width(left: Option<&str>, right: Option<&str>) -> usize {
    match (left, right) {
        (None, None) => 0,
        (Some(left), None) => left_fixed(left) + MIN_FILL,
        (None, Some(right)) => MIN_FILL + right_fixed(right),
        (Some(left), Some(right)) => left_fixed(left) + MIN_FILL + right_fixed(right),
    }
}

fn paint(
    left: FittedCaption<'_>,
    right: FittedCaption<'_>,
    rule_style: Style,
    width: usize,
) -> Line<'static> {
    if left.is_none() && right.is_none() {
        return Line::styled(RULE.repeat(width), rule_style);
    }
    let fixed = left.map(|(label, _)| left_fixed(label)).unwrap_or(0)
        + right.map(|(label, _)| right_fixed(label)).unwrap_or(0);
    let fill = width.saturating_sub(fixed);
    let mut spans = Vec::new();
    if let Some((label, style)) = left {
        spans.push(Span::styled(LEFT_PREFIX.to_string(), rule_style));
        spans.push(Span::styled(format!("{label} "), style));
    }
    spans.push(Span::styled(RULE.repeat(fill), rule_style));
    if let Some((label, style)) = right {
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::styled(RULE.repeat(MIN_RIGHT_SUFFIX), rule_style));
    }
    Line::from(spans)
}

#[cfg(test)]
#[path = "divider_tests.rs"]
mod tests;
