//! Row matching for picker filters: regex, literal, and fuzzy modes.
//!
//! Each mode decides which [`PickerItem`] fields it searches. Hidden
//! `search_terms` join every mode so nested choices stay reachable.

use regex::Regex;

use super::{PickerDetail, PickerItem};

pub(super) fn picker_matching_indices_with_regex(
    items: &[PickerItem],
    filter: &str,
    regex: Option<&Regex>,
) -> Vec<usize> {
    if filter.is_empty() {
        return (0..items.len()).collect();
    }
    let Some(regex) = regex else {
        return Vec::new();
    };

    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| regex.is_match(&picker_haystack(item)).then_some(index))
        .collect()
}

/// Rows whose label, value, or search term contains `filter`, ignoring case,
/// in row order. Fields are checked one at a time so a match cannot span two
/// of them.
pub(super) fn literal_matching_indices(items: &[PickerItem], filter: &str) -> Vec<usize> {
    let filter = filter.to_lowercase();
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            [&item.label, &item.value]
                .into_iter()
                .chain(&item.search_terms)
                .any(|field| field.to_lowercase().contains(&filter))
        })
        .map(|(index, _)| index)
        .collect()
}

pub(in crate::tui) fn fuzzy_picker_matching_indices(
    items: &[PickerItem],
    filter: &str,
) -> Vec<usize> {
    let filter = filter.trim();
    if filter.is_empty() {
        return (0..items.len()).collect();
    }

    fuzzy_matching_indices(items, filter)
}

fn fuzzy_matching_indices(items: &[PickerItem], filter: &str) -> Vec<usize> {
    let mut matches = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| fuzzy_item_score(item, filter).map(|score| (index, score)))
        .collect::<Vec<_>>();
    matches.sort_by(|(left_index, left_score), (right_index, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_index.cmp(right_index))
    });
    matches.into_iter().map(|(index, _)| index).collect()
}

/// Best fuzzy score across the fields a user can see and reasonably type,
/// plus the row's hidden search terms, which stand in for nested choices the
/// user expects to reach from this row.
///
/// Long free text (detail, preview) stays out: subsequence matching over a
/// paragraph matches almost any filter and would drown the ranking.
fn fuzzy_item_score(item: &PickerItem, filter: &str) -> Option<i64> {
    [
        Some(item.label.as_str()),
        Some(item.value.as_str()),
        item.section.as_deref(),
        item.badge.as_ref().map(|badge| badge.text.as_str()),
    ]
    .into_iter()
    .flatten()
    .chain(item.search_terms.iter().map(String::as_str))
    .filter_map(|field| fuzzy_match_score(field, filter))
    .max()
}

fn picker_haystack(item: &PickerItem) -> String {
    let section = item.section.as_deref().unwrap_or_default();
    let detail = item
        .detail
        .as_ref()
        .map(PickerDetail::plain_text)
        .unwrap_or_default();
    let preview = item.preview.as_deref().unwrap_or_default();
    let badge = item
        .badge
        .as_ref()
        .map(|badge| badge.text.as_str())
        .unwrap_or_default();
    let search_terms = item.search_terms.join(" ");
    format!(
        "{} {} {} {} {} {} {}",
        item.label, item.value, section, detail, preview, badge, search_terms
    )
}

/// Fuzzy score for `needle` against `haystack`, or `None` when the needle is
/// not a subsequence of it.
///
/// Scoring wants the occurrence with the best bonus, but taking that occurrence
/// greedily can eat a character the rest of the needle still needs, and the
/// walk then reports no match for text the row plainly contains. A backward
/// pass fixes it by bounding each choice: [`fuzzy_latest_indices`] records, per
/// needle character, the latest haystack index it can take while still leaving
/// room for every character after it. The forward pass picks the best-scoring
/// occurrence at or before that bound, so it ranks freely and can never strand,
/// and every row is scored on one scale.
pub(in crate::tui) fn fuzzy_match_score(haystack: &str, needle: &str) -> Option<i64> {
    let haystack = haystack.to_lowercase().chars().collect::<Vec<_>>();
    let needle = needle.to_lowercase().chars().collect::<Vec<_>>();
    let latest = fuzzy_latest_indices(&haystack, &needle)?;

    let mut search_start = 0;
    let mut first_match = None;
    let mut previous_match = None;
    let mut score = 0;

    for (needle_index, needle_char) in needle.iter().enumerate() {
        // Always some: `haystack[latest[needle_index]] == *needle_char` and the
        // bound rises faster than `search_start`, so the range holds it.
        let index = (search_start..=latest[needle_index])
            .filter(|index| haystack[*index] == *needle_char)
            .max_by_key(|index| fuzzy_character_bonus(&haystack, *index, previous_match))?;
        first_match.get_or_insert(index);
        score += 10;
        score += fuzzy_character_bonus(&haystack, index, previous_match);
        previous_match = Some(index);
        search_start = index + 1;
    }

    let first_match = first_match.unwrap_or_default() as i64;
    let span = previous_match.unwrap_or_default() as i64 - first_match;
    Some(score - first_match - span)
}

/// Latest haystack index each needle character can occupy while leaving room
/// for the rest of the needle, or `None` when the needle is not a subsequence.
///
/// This is the definitive match test, so a non-matching row - most rows while
/// the user is typing - costs this one pass and no scoring work.
fn fuzzy_latest_indices(haystack: &[char], needle: &[char]) -> Option<Vec<usize>> {
    let mut latest = vec![0; needle.len()];
    let mut bound = haystack.len();
    for (needle_index, needle_char) in needle.iter().enumerate().rev() {
        bound = haystack[..bound]
            .iter()
            .rposition(|haystack_char| haystack_char == needle_char)?;
        latest[needle_index] = bound;
    }
    Some(latest)
}

fn fuzzy_character_bonus(haystack: &[char], index: usize, previous_match: Option<usize>) -> i64 {
    let mut bonus = 0;
    if previous_match.is_some_and(|previous| previous + 1 == index) {
        bonus += 12;
    }
    if index == 0 || is_word_boundary(haystack[index.saturating_sub(1)]) {
        bonus += 20;
    }
    bonus
}

fn is_word_boundary(ch: char) -> bool {
    matches!(ch, '/' | '\\' | '_' | '-' | '.' | ' ')
}
