//! Picker widget: filtering, input, overlay layout, and shared rendering.
//!
//! Feature modules supply rows, labels, and a named constructor. This module
//! owns opening, closing, filtering, cursor movement, key handling, parent
//! stack, and overlay paint. Feature confirm/escape lives in
//! `crate::tui::picker_actions`.

use regex::{Regex, RegexBuilder};
use std::{
    cell::{Ref, RefCell},
    ops::Deref,
};

mod action;
mod input;
mod lifecycle;
mod overlay;
mod overlay_layout;
mod overlay_state;
mod rows;

pub(in crate::tui) use action::{ConfigParentRow, DuringTurnSelect, PickerAction, PickerTurn};
pub(in crate::tui) use input::{
    apply_picker_key, overlay_scroll_targets, PickerKeyEffect, PickerMouseEvent,
};
use overlay::{detail_content_line_count, overlay_detail_lines};
pub(in crate::tui) use overlay::{picker_overlay_frame, OverlayChrome};
use overlay_layout::DetailViewport;
pub(in crate::tui) use overlay_layout::{clamp_overlay_scroll, OverlayScrollbarState};
pub(in crate::tui) use overlay_state::{OverlayFocus, OverlayScrollbarDrag};
pub(in crate::tui) use rows::{
    label_column_width, picker_item_rows, scroll_window_start, RowLayout, RowWidthMode,
};

#[derive(Debug)]
pub(super) struct PickerMatches<'a>(Ref<'a, Vec<usize>>);

impl Deref for PickerMatches<'_> {
    type Target = Vec<usize>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PartialEq<Vec<usize>> for PickerMatches<'_> {
    fn eq(&self, other: &Vec<usize>) -> bool {
        self.0.as_slice() == other.as_slice()
    }
}

/// Optional key bindings advertised in footers and honored by picker input.
///
/// Renderers and input both read these fields so hints cannot drift from keys.
/// Model-list labels come from the live keybindings; flags stay for the
/// remaining hardcoded shortcuts (Tab, d/Delete).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct PickerKeyHints {
    /// Bound key that pins or unpins the selected model, when shown.
    pub(super) pin_toggle: Option<String>,
    /// Bound key that switches the model list between all and pinned.
    pub(super) scope_toggle: Option<String>,
    /// Tab fills the filter from the selected row.
    pub(super) tab_complete: bool,
    /// `d` / Delete removes the selected row (sessions, workflows).
    pub(super) row_delete: bool,
}

#[derive(Clone, Debug, Default)]
struct PickerMatchCache {
    initialized: bool,
    filter: String,
    _regex: Option<Regex>,
    invalid_regex: bool,
    indices: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
struct DetailWrapCache {
    selected: usize,
    width: usize,
    detail_len: usize,
    detail_ptr: usize,
    lines: Vec<String>,
}

/// Filter text and match-list index used to restore a refreshed picker.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct PickerCursor {
    pub(super) filter: String,
    /// Index into the filtered match list when the cursor was captured.
    pub(super) match_index: usize,
}

#[derive(Clone, Debug)]
pub(super) struct UiPicker {
    pub(super) title: String,
    pub(super) key_hints: PickerKeyHints,
    pub(super) items: Vec<PickerItem>,
    pub(super) selected: usize,
    pub(super) filter: String,
    pub(in crate::tui::picker) action: PickerAction,
    pub(super) layout: PickerLayout,
    pub(super) badge_placement: PickerBadgePlacement,
    /// Top visible detail line for overlay pickers.
    pub(super) detail_scroll: usize,
    /// Pane that keyboard scrolling targets in overlay pickers.
    pub(super) overlay_focus: OverlayFocus,
    /// Manual nav viewport offset (row space). Only used while
    /// `nav_follows_selection` is false.
    nav_scroll: usize,
    /// Whether the nav viewport tracks the selection (keyboard mode) or holds
    /// a manual wheel-scrolled offset.
    nav_follows_selection: bool,
    /// Active drag on an overlay scrollbar, if any.
    overlay_scrollbar_drag: Option<OverlayScrollbarDrag>,
    /// Nav row under the mouse pointer, in row space.
    hovered_nav_row: Option<usize>,
    pub(super) confirm_verb: Option<String>,
    /// Inventory-empty copy when the filter is blank. Filter misses stay
    /// "no matches".
    empty_message: Option<String>,
    /// Status set when this picker is restored as a parent.
    restore_status: &'static str,
    /// When set, overrides [`PickerAction::uses_regex_filter`] for this picker.
    pub(super) force_fuzzy_filter: bool,
    /// When set, Space confirms the row like Enter (toggle-style pickers).
    space_confirms: bool,
    pub(super) overlay_chrome: Option<OverlayChrome>,
    parent: Option<Box<UiPicker>>,
    matches: RefCell<PickerMatchCache>,
    detail_wrap_cache: RefCell<DetailWrapCache>,
}

#[derive(Clone, Debug)]
pub(super) struct PickerItem {
    pub(super) label: String,
    pub(super) section: Option<String>,
    pub(super) detail: Option<String>,
    pub(super) preview: Option<String>,
    pub(super) badge: Option<PickerBadge>,
    pub(super) value: String,
    /// Optional Enter verb for this row. Overrides action defaults and badge heuristics.
    pub(super) selection_verb: Option<&'static str>,
    /// When false, Tab does not copy this row into the filter.
    pub(super) allow_filter_completion: bool,
}

#[derive(Clone, Debug)]
pub(super) struct PickerBadge {
    pub(super) text: String,
    pub(super) tone: PickerBadgeTone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickerBadgeTone {
    Internal,
    Editable,
    Selected,
    Favorite,
    Healthy,
    Warning,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PickerBadgePlacement {
    #[default]
    Navigation,
    Detail,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PickerLayout {
    #[default]
    List,
    /// Large popup shell. Detail pane appears when items carry detail text.
    Overlay,
}

impl PickerLayout {
    pub(super) fn is_overlay(self) -> bool {
        matches!(self, Self::Overlay)
    }
}

pub(super) fn cmp_ascii_ignore_case(left: &str, right: &str) -> std::cmp::Ordering {
    left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase())
}

/// Sort picker rows by label (case-insensitive), breaking ties on value.
pub(super) fn sort_items_by_ascii_label(items: &mut [PickerItem]) {
    items.sort_by(|left, right| {
        cmp_ascii_ignore_case(&left.label, &right.label).then_with(|| left.value.cmp(&right.value))
    });
}

macro_rules! picker_ctors {
    ($($name:ident => $action:ident),+ $(,)?) => {
        $(
            pub(in crate::tui) fn $name(title: impl Into<String>, items: Vec<PickerItem>) -> Self {
                Self::new(title, items, PickerAction::$action)
            }
        )+
    };
}

macro_rules! picker_is {
    ($($name:ident => $action:ident),+ $(,)?) => {
        $(
            pub(in crate::tui) fn $name(&self) -> bool {
                self.action == PickerAction::$action
            }
        )+
    };
}

impl UiPicker {
    pub(in crate::tui::picker) fn new(
        title: impl Into<String>,
        items: Vec<PickerItem>,
        action: PickerAction,
    ) -> Self {
        Self {
            title: title.into(),
            key_hints: PickerKeyHints::default(),
            items,
            selected: 0,
            filter: String::new(),
            action,
            layout: PickerLayout::List,
            badge_placement: PickerBadgePlacement::Navigation,
            detail_scroll: 0,
            overlay_focus: OverlayFocus::default(),
            nav_scroll: 0,
            nav_follows_selection: true,
            overlay_scrollbar_drag: None,
            hovered_nav_row: None,
            confirm_verb: None,
            empty_message: None,
            restore_status: "ready",
            force_fuzzy_filter: false,
            space_confirms: false,
            overlay_chrome: None,
            parent: None,
            matches: RefCell::default(),
            detail_wrap_cache: RefCell::default(),
        }
    }

    picker_ctors! {
        models => SelectModel,
        internal_agent_models => SelectInternalAgentModel,
        login_group => LoginGroup,
        login_provider => LoginProvider,
        logout_provider => LogoutProvider,
        switch_auth_mode => SwitchAuthMode,
        refresh_model_list => RefreshModelList,
        insert_skill => InsertSkillCommand,
        view_agent => ViewAgent,
        view_mcp => ViewMcpServers,
        resume_session => ResumeSession,
        manage_sessions => ManageSessions,
        tree => SelectTreeNode,
        rewind_checkpoint => SelectRewindCheckpoint,
        confirm_rewind => ConfirmRewindCheckpoint,
        config => Config,
        theme => SelectTheme,
        edit_agent => EditAgent,
        workflow => Workflow,
        attach_subagent => AttachSubagent,
        dismiss => Dismiss,
    }

    picker_is! {
        is_config => Config,
        is_theme => SelectTheme,
        is_mcp_inventory => ViewMcpServers,
        is_edit_agent => EditAgent,
        is_attach_subagent => AttachSubagent,
        is_conversation_model => SelectModel,
        is_internal_agent_model => SelectInternalAgentModel,
        is_manage_sessions => ManageSessions,
        is_resume_session => ResumeSession,
        is_workflow => Workflow,
    }

    pub(in crate::tui) fn is_model_list(&self) -> bool {
        self.action.is_model_list()
    }

    pub(in crate::tui) fn space_confirms_selection(&self) -> bool {
        self.space_confirms || self.action.space_confirms_selection()
    }

    pub(in crate::tui) fn restore_status(&self) -> &'static str {
        self.restore_status
    }

    pub(super) fn with_layout(mut self, layout: PickerLayout) -> Self {
        self.layout = layout;
        self
    }

    pub(super) fn with_key_hints(mut self, key_hints: PickerKeyHints) -> Self {
        self.key_hints = key_hints;
        self
    }

    pub(super) fn with_fuzzy_filter(mut self) -> Self {
        self.force_fuzzy_filter = true;
        self
    }

    pub(super) fn with_space_confirm(mut self) -> Self {
        self.space_confirms = true;
        self
    }

    pub(super) fn uses_regex_filter(&self) -> bool {
        !self.force_fuzzy_filter && self.action.uses_regex_filter()
    }

    pub(super) fn with_badge_placement(mut self, placement: PickerBadgePlacement) -> Self {
        self.badge_placement = placement;
        self
    }

    pub(super) fn with_overlay_chrome(mut self, chrome: OverlayChrome) -> Self {
        self.overlay_chrome = Some(chrome);
        self
    }

    pub(super) fn is_overlay(&self) -> bool {
        self.layout.is_overlay()
    }

    /// Whether any item carries detail text. Shared by inline and overlay pickers.
    pub(super) fn has_item_details(&self) -> bool {
        self.items.iter().any(|item| item.detail.is_some())
    }

    /// Selection changed: show it, and return the nav viewport to it.
    fn on_selection_changed(&mut self) {
        self.reset_detail_scroll();
        self.nav_follows_selection = true;
    }

    /// Filter + match index for reopening this picker near the same place.
    pub(super) fn cursor(&self) -> super::PickerCursor {
        let match_index = self
            .matching_indices()
            .iter()
            .position(|index| *index == self.selected)
            .unwrap_or(0);
        super::PickerCursor {
            filter: self.filter.clone(),
            match_index,
        }
    }

    /// Restore filter and select the match nearest to `cursor.match_index`.
    pub(super) fn restore_cursor(&mut self, cursor: &super::PickerCursor) {
        self.filter = cursor.filter.clone();
        let selected = {
            let matches = self.matching_indices();
            matches
                .get(cursor.match_index.min(matches.len().saturating_sub(1)))
                .copied()
                .unwrap_or(0)
        };
        self.selected = selected;
        self.on_selection_changed();
    }

    pub(super) fn select_by_offset(&mut self, delta: isize) {
        let next = {
            let matches = self.matching_indices();
            if matches.is_empty() || delta == 0 {
                return;
            }
            let position = matches
                .iter()
                .position(|index| *index == self.selected)
                .unwrap_or(0);
            let next_position = if delta < 0 {
                position.saturating_sub(delta.unsigned_abs())
            } else {
                position
                    .saturating_add(delta as usize)
                    .min(matches.len().saturating_sub(1))
            };
            matches[next_position]
        };
        if next != self.selected {
            self.selected = next;
            self.on_selection_changed();
        }
    }

    pub(super) fn reset_detail_scroll(&mut self) {
        self.detail_scroll = 0;
    }

    pub(super) fn scroll_detail_by(&mut self, delta: isize, viewport: DetailViewport) {
        if !self.has_scrollable_detail() {
            return;
        }
        self.detail_scroll = if delta < 0 {
            self.detail_scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.detail_scroll.saturating_add(delta as usize)
        };
        self.clamp_detail_scroll(viewport);
    }

    pub(super) fn scroll_detail_home(&mut self) {
        if !self.has_scrollable_detail() {
            return;
        }
        self.reset_detail_scroll();
    }

    pub(super) fn scroll_detail_end(&mut self, viewport: DetailViewport) {
        if !self.has_scrollable_detail() {
            return;
        }
        let line_count = self.detail_line_count(viewport.width);
        self.detail_scroll = line_count.saturating_sub(viewport.rows.max(1));
    }

    pub(super) fn scroll_detail_page(&mut self, delta_pages: isize, viewport: DetailViewport) {
        if !self.has_scrollable_detail() {
            return;
        }
        let rows = viewport.rows.max(1) as isize;
        self.scroll_detail_by(delta_pages.saturating_mul(rows), viewport);
    }

    pub(super) fn clamp_detail_scroll(&mut self, viewport: DetailViewport) {
        if !self.has_scrollable_detail() {
            return;
        }
        let line_count = self.detail_line_count(viewport.width);
        self.detail_scroll =
            overlay::clamp_detail_scroll(self.detail_scroll, line_count, viewport.rows);
    }

    pub(super) fn detail_line_count(&self, detail_width: usize) -> usize {
        detail_content_line_count(
            self.wrapped_detail_lines(detail_width).len(),
            self.selected_detail_badge().is_some(),
        )
    }

    pub(super) fn wrapped_detail_lines(&self, detail_width: usize) -> Ref<'_, Vec<String>> {
        let detail = self.selected_detail();
        let detail_len = detail.len();
        let detail_ptr = detail.as_ptr() as usize;
        let width = detail_width.max(1);
        let stale = {
            let cache = self.detail_wrap_cache.borrow();
            cache.selected != self.selected
                || cache.width != width
                || cache.detail_len != detail_len
                || cache.detail_ptr != detail_ptr
                || cache.lines.is_empty() && !detail.is_empty()
        };
        if stale {
            let lines = overlay_detail_lines(detail, width);
            *self.detail_wrap_cache.borrow_mut() = DetailWrapCache {
                selected: self.selected,
                width,
                detail_len,
                detail_ptr,
                lines,
            };
        }
        Ref::map(self.detail_wrap_cache.borrow(), |cache| &cache.lines)
    }

    pub(super) fn selected_detail_badge(&self) -> Option<&PickerBadge> {
        if self.badge_placement != PickerBadgePlacement::Detail {
            return None;
        }
        self.selected_item()?.badge.as_ref()
    }

    pub(super) fn selected_detail(&self) -> &str {
        self.selected_item()
            .and_then(|item| item.detail.as_deref())
            .unwrap_or_default()
    }

    pub(super) fn confirm_action_label(&self) -> &str {
        if let Some(verb) = self.selected_item().and_then(|item| item.selection_verb) {
            return verb;
        }
        if let Some(verb) = self.confirm_verb.as_deref() {
            return verb;
        }
        self.action.default_confirm_verb()
    }

    pub(super) fn action_footer(&self) -> String {
        super::composer_chrome::join_footer_parts(
            self.action_footer_parts()
                .iter()
                .map(std::string::String::as_str),
        )
    }

    /// Key-hint segments for overlay and inline footers (no title prefix).
    pub(super) fn action_footer_parts(&self) -> Vec<String> {
        let confirm = self.confirm_action_label();
        let escape = self.escape_verb();
        let mut parts = Vec::new();
        // Dismiss overlays use Enter and Esc for the same exit.
        if confirm == escape {
            parts.push(format!("Enter/Esc {confirm}"));
        } else {
            parts.push(format!("Enter {confirm}"));
        }
        if self.space_confirms {
            parts.push(format!("Space {confirm}"));
        } else if self.action.space_confirms_selection() {
            parts.push("Space confirm".into());
        }
        if let Some(key) = &self.key_hints.pin_toggle {
            parts.push(format!("{key} pin/unpin"));
        }
        if let Some(key) = &self.key_hints.scope_toggle {
            parts.push(format!("{key} all/pinned"));
        }
        if self.key_hints.tab_complete {
            parts.push("Tab complete".into());
        }
        if self.key_hints.row_delete {
            parts.push("d delete".into());
        }
        if confirm != escape {
            parts.push(format!("Esc {escape}"));
        }
        parts
    }

    /// Inline list footer: title plus shared action hints and search cue.
    pub(super) fn list_footer_parts(&self) -> Vec<String> {
        let mut parts = vec![self.title.clone(), "Type to search".into()];
        parts.extend(self.action_footer_parts());
        parts
    }

    fn escape_verb(&self) -> &'static str {
        if self.has_parent() {
            return "back";
        }
        match self.confirm_action_label() {
            "close" => "close",
            _ => "cancel",
        }
    }

    /// Empty-match message when the filter yields no rows.
    pub(super) fn empty_match_message(&self) -> &str {
        if self.filter_is_invalid_regex() {
            "invalid regex"
        } else if self.filter.is_empty() {
            self.empty_message.as_deref().unwrap_or("no matches")
        } else {
            "no matches"
        }
    }

    pub(super) fn filter_is_invalid_regex(&self) -> bool {
        // Ensure the match cache is current before reading the flag.
        let _ = self.matching_indices();
        self.matches.borrow().invalid_regex
    }

    pub(super) fn with_confirm_verb(mut self, verb: impl Into<String>) -> Self {
        self.confirm_verb = Some(verb.into());
        self
    }

    pub(super) fn with_empty_message(mut self, message: impl Into<String>) -> Self {
        self.empty_message = Some(message.into());
        self
    }

    pub(super) fn with_restore_status(mut self, status: &'static str) -> Self {
        self.restore_status = status;
        self
    }

    pub(super) fn with_parent(mut self, parent: UiPicker) -> Self {
        self.parent = Some(Box::new(parent));
        self
    }

    pub(super) fn has_parent(&self) -> bool {
        self.parent.is_some()
    }

    pub(super) fn take_parent(&mut self) -> Option<UiPicker> {
        self.parent.take().map(|parent| *parent)
    }

    pub(super) fn select_previous(&mut self) {
        let next = {
            let matches = self.matching_indices();
            if matches.is_empty() {
                return;
            }
            let position = matches
                .iter()
                .position(|index| *index == self.selected)
                .unwrap_or(0);
            if position == 0 {
                *matches.last().unwrap()
            } else {
                matches[position - 1]
            }
        };
        self.selected = next;
        self.on_selection_changed();
    }

    pub(super) fn select_next(&mut self) {
        let next = {
            let matches = self.matching_indices();
            if matches.is_empty() {
                return;
            }
            let position = matches
                .iter()
                .position(|index| *index == self.selected)
                .unwrap_or(0);
            matches[(position + 1) % matches.len()]
        };
        self.selected = next;
        self.on_selection_changed();
    }

    pub(super) fn push_filter_char(&mut self, ch: char) {
        self.filter.push(ch);
        self.select_first_match();
    }

    pub(super) fn pop_filter_char(&mut self) {
        self.filter.pop();
        self.select_first_match();
    }

    pub(super) fn complete_filter(&mut self) {
        let Some(item) = self.selected_item() else {
            return;
        };
        if !Self::row_allows_filter_completion(item) {
            return;
        }
        self.filter = if self.uses_regex_filter() {
            regex::escape(&item.value)
        } else {
            item.value.clone()
        };
    }

    fn row_allows_filter_completion(item: &PickerItem) -> bool {
        item.allow_filter_completion
    }

    pub(super) fn select_first_match(&mut self) {
        let first = self.matching_indices().first().copied();
        if let Some(index) = first {
            self.selected = index;
            self.on_selection_changed();
        }
    }

    pub(super) fn select_last_match(&mut self) {
        let last = self.matching_indices().last().copied();
        if let Some(index) = last {
            self.selected = index;
            self.on_selection_changed();
        }
    }

    pub(super) fn matching_indices(&self) -> PickerMatches<'_> {
        let stale = {
            let cache = self.matches.borrow();
            !cache.initialized || cache.filter != self.filter
        };
        if stale {
            let filter = self.filter.trim();
            let (regex, invalid_regex) = if self.uses_regex_filter() && !filter.is_empty() {
                match RegexBuilder::new(filter).case_insensitive(true).build() {
                    Ok(regex) => (Some(regex), false),
                    Err(_) => (None, true),
                }
            } else {
                (None, false)
            };
            let indices = if self.uses_regex_filter() {
                picker_matching_indices_with_regex(&self.items, filter, regex.as_ref())
            } else {
                fuzzy_picker_matching_indices(&self.items, filter)
            };
            *self.matches.borrow_mut() = PickerMatchCache {
                initialized: true,
                filter: self.filter.clone(),
                _regex: regex,
                invalid_regex,
                indices,
            };
        }
        PickerMatches(Ref::map(self.matches.borrow(), |cache| &cache.indices))
    }

    pub(super) fn selected_item(&self) -> Option<&PickerItem> {
        self.matching_indices()
            .contains(&self.selected)
            .then(|| self.items.get(self.selected))
            .flatten()
    }
}

fn picker_matching_indices_with_regex(
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

pub(super) fn fuzzy_picker_matching_indices(items: &[PickerItem], filter: &str) -> Vec<usize> {
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

/// Best fuzzy score across the fields a user can see and reasonably type.
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
    .filter_map(|field| fuzzy_match_score(field, filter))
    .max()
}

fn picker_haystack(item: &PickerItem) -> String {
    let section = item.section.as_deref().unwrap_or_default();
    let detail = item.detail.as_deref().unwrap_or_default();
    let preview = item.preview.as_deref().unwrap_or_default();
    let badge = item
        .badge
        .as_ref()
        .map(|badge| badge.text.as_str())
        .unwrap_or_default();
    format!(
        "{} {} {} {} {} {}",
        item.label, item.value, section, detail, preview, badge
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
pub(super) fn fuzzy_match_score(haystack: &str, needle: &str) -> Option<i64> {
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

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
