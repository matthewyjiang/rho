//! Generic labeled rules for composer chrome.
//!
//! Feature code supplies captions. This module only fits them onto a rule.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::render::display_width;

/// Longest-first captions for one side of a divider.
#[derive(Clone, Copy)]
pub(super) struct DividerCaption<'a> {
    pub(super) candidates: &'a [&'a str],
    pub(super) style: Style,
}

const RULE: &str = "─";
const LEFT_PREFIX: &str = "─ ";
const MIN_FILL: usize = 2;
const MIN_RIGHT_SUFFIX: usize = 1;

type FittedCaption<'a> = Option<(&'a str, Style)>;

/// Paint a full-width rule, with optional left and right captions.
///
/// Left captions keep the existing `─ label ────` shape. Right captions sit
/// flush against a short trailing rule: `──── label ─`. When both sides are
/// present and width is scarce, the right caption drops first.
pub(super) fn labeled_divider_line(
    left: Option<DividerCaption<'_>>,
    right: Option<DividerCaption<'_>>,
    rule_style: Style,
    width: usize,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    let (left, right) = fit_captions(left, right, width);
    match (left, right) {
        (None, None) => Line::styled(RULE.repeat(width), rule_style),
        (Some((left, left_style)), None) => paint_left(left, left_style, rule_style, width),
        (None, Some((right, right_style))) => paint_right(right, right_style, rule_style, width),
        (Some((left, left_style)), Some((right, right_style))) => {
            paint_both(left, left_style, right, right_style, rule_style, width)
        }
    }
}

fn fit_captions<'a>(
    left: Option<DividerCaption<'a>>,
    right: Option<DividerCaption<'a>>,
    width: usize,
) -> (FittedCaption<'a>, FittedCaption<'a>) {
    let lefts = caption_choices(left);
    let rights = caption_choices(right);
    for &(left, left_style) in &lefts {
        for &(right, right_style) in &rights {
            if both_needed(left, right) <= width {
                return (Some((left, left_style)), Some((right, right_style)));
            }
        }
    }
    for &(left, left_style) in &lefts {
        if left_needed(left) <= width {
            return (Some((left, left_style)), None);
        }
    }
    for &(right, right_style) in &rights {
        if right_needed(right) <= width {
            return (None, Some((right, right_style)));
        }
    }
    (None, None)
}

fn caption_choices(caption: Option<DividerCaption<'_>>) -> Vec<(&str, Style)> {
    let Some(caption) = caption else {
        return Vec::new();
    };
    caption
        .candidates
        .iter()
        .copied()
        .filter(|text| !text.is_empty())
        .map(|text| (text, caption.style))
        .collect()
}

fn left_needed(label: &str) -> usize {
    display_width(LEFT_PREFIX) + display_width(label) + 1 + MIN_FILL
}

fn right_needed(label: &str) -> usize {
    MIN_FILL + 1 + display_width(label) + 1 + MIN_RIGHT_SUFFIX
}

fn both_needed(left: &str, right: &str) -> usize {
    left_needed(left) + right_needed(right) - MIN_FILL
}

fn paint_left(label: &str, label_style: Style, rule_style: Style, width: usize) -> Line<'static> {
    let fill = width
        .saturating_sub(display_width(LEFT_PREFIX))
        .saturating_sub(display_width(label))
        .saturating_sub(1);
    Line::from(vec![
        Span::styled(LEFT_PREFIX.to_string(), rule_style),
        Span::styled(format!("{label} "), label_style),
        Span::styled(RULE.repeat(fill), rule_style),
    ])
}

fn paint_right(label: &str, label_style: Style, rule_style: Style, width: usize) -> Line<'static> {
    let fill = width
        .saturating_sub(display_width(label))
        .saturating_sub(1 + 1 + MIN_RIGHT_SUFFIX);
    Line::from(vec![
        Span::styled(RULE.repeat(fill), rule_style),
        Span::styled(format!(" {label} "), label_style),
        Span::styled(RULE.repeat(MIN_RIGHT_SUFFIX), rule_style),
    ])
}

fn paint_both(
    left: &str,
    left_style: Style,
    right: &str,
    right_style: Style,
    rule_style: Style,
    width: usize,
) -> Line<'static> {
    let fill = width
        .saturating_sub(display_width(LEFT_PREFIX))
        .saturating_sub(display_width(left))
        .saturating_sub(1)
        .saturating_sub(1)
        .saturating_sub(display_width(right))
        .saturating_sub(1 + MIN_RIGHT_SUFFIX);
    Line::from(vec![
        Span::styled(LEFT_PREFIX.to_string(), rule_style),
        Span::styled(format!("{left} "), left_style),
        Span::styled(RULE.repeat(fill), rule_style),
        Span::styled(format!(" {right} "), right_style),
        Span::styled(RULE.repeat(MIN_RIGHT_SUFFIX), rule_style),
    ])
}

#[cfg(test)]
#[path = "divider_tests.rs"]
mod tests;
