use regex::{Regex, RegexBuilder};
use std::{
    cell::{Ref, RefCell},
    ops::Deref,
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

#[derive(Clone, Debug, Default)]
struct PickerMatchCache {
    initialized: bool,
    filter: String,
    _regex: Option<Regex>,
    indices: Vec<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct UiPicker {
    pub(super) title: String,
    pub(super) help: String,
    pub(super) items: Vec<PickerItem>,
    pub(super) selected: usize,
    pub(super) filter: String,
    pub(super) action: PickerAction,
    pub(super) layout: PickerLayout,
    pub(super) detail_scroll: usize,
    pub(super) confirm_verb: Option<String>,
    parent: Option<Box<UiPicker>>,
    matches: RefCell<PickerMatchCache>,
}

#[derive(Clone, Debug)]
pub(super) struct PickerItem {
    pub(super) label: String,
    pub(super) detail: Option<String>,
    pub(super) preview: Option<String>,
    pub(super) badge: Option<PickerBadge>,
    pub(super) value: String,
}

#[derive(Clone, Debug)]
pub(super) struct PickerBadge {
    pub(super) text: String,
    pub(super) tone: PickerBadgeTone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickerBadgeTone {
    Internal,
    Selected,
    Favorite,
    Healthy,
    Warning,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PickerLayout {
    #[default]
    List,
    NavigablePopup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickerAction {
    SelectModel,
    SelectInternalAgentModel,
    LoginGroup,
    LoginProvider,
    LogoutProvider,
    RefreshModelList,
    InsertSkillCommand,
    ViewAgent,
    ResumeSession,
    Config,
    Doctor,
}

impl PickerAction {
    pub(super) fn space_confirms_selection(self) -> bool {
        match self {
            PickerAction::Config | PickerAction::Doctor => true,
            PickerAction::SelectModel
            | PickerAction::SelectInternalAgentModel
            | PickerAction::LoginGroup
            | PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::RefreshModelList
            | PickerAction::InsertSkillCommand
            | PickerAction::ViewAgent
            | PickerAction::ResumeSession => false,
        }
    }
}

impl UiPicker {
    pub(super) fn new(
        title: impl Into<String>,
        help: impl Into<String>,
        items: Vec<PickerItem>,
        action: PickerAction,
    ) -> Self {
        Self {
            title: title.into(),
            help: help.into(),
            items,
            selected: 0,
            filter: String::new(),
            action,
            layout: PickerLayout::List,
            detail_scroll: 0,
            confirm_verb: None,
            parent: None,
            matches: RefCell::default(),
        }
    }

    pub(super) fn with_layout(mut self, layout: PickerLayout) -> Self {
        self.layout = layout;
        self
    }

    pub(super) fn uses_navigable_popup(&self) -> bool {
        matches!(self.layout, PickerLayout::NavigablePopup)
    }

    pub(super) fn reset_detail_scroll(&mut self) {
        self.detail_scroll = 0;
    }

    pub(super) fn scroll_detail_by(&mut self, delta: isize) {
        if delta < 0 {
            self.detail_scroll = self.detail_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.detail_scroll = self.detail_scroll.saturating_add(delta as usize);
        }
    }

    pub(super) fn scroll_detail_home(&mut self) {
        self.detail_scroll = 0;
    }

    pub(super) fn scroll_detail_end(&mut self) {
        self.detail_scroll = usize::MAX;
    }

    pub(super) fn clamp_detail_scroll_for(&mut self, detail_width: usize, viewport_rows: usize) {
        let detail = self.selected_detail();
        let line_count =
            super::navigable_popup::navigable_popup_detail_lines(detail, detail_width).len();
        self.detail_scroll = super::navigable_popup::clamp_detail_scroll(
            self.detail_scroll,
            line_count,
            viewport_rows,
        );
    }

    pub(super) fn selected_detail(&self) -> &str {
        self.selected_item()
            .and_then(|item| item.detail.as_deref())
            .unwrap_or_default()
    }

    pub(super) fn navigable_popup_action_footer(&self) -> String {
        let action = match self.action {
            PickerAction::ViewAgent
                if self
                    .selected_item()
                    .and_then(|item| item.badge.as_ref())
                    .is_some_and(|badge| badge.tone == PickerBadgeTone::Internal) =>
            {
                "Enter configure"
            }
            PickerAction::ViewAgent => "Enter close",
            _ => "Enter select",
        };
        format!("{action} · Esc close")
    }

    pub(super) fn with_confirm_verb(mut self, verb: impl Into<String>) -> Self {
        self.confirm_verb = Some(verb.into());
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
        self.reset_detail_scroll();
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
        self.reset_detail_scroll();
    }

    pub(super) fn push_filter_char(&mut self, ch: char) {
        self.filter.push(ch);
        self.select_first_match();
        self.reset_detail_scroll();
    }

    pub(super) fn pop_filter_char(&mut self) {
        self.filter.pop();
        self.select_first_match();
        self.reset_detail_scroll();
    }

    pub(super) fn complete_filter(&mut self) {
        if let Some(item) = self.selected_item() {
            self.filter = match self.action {
                PickerAction::SelectModel | PickerAction::SelectInternalAgentModel => {
                    item.value.clone()
                }
                PickerAction::LoginGroup
                | PickerAction::LoginProvider
                | PickerAction::LogoutProvider
                | PickerAction::RefreshModelList
                | PickerAction::InsertSkillCommand
                | PickerAction::ViewAgent
                | PickerAction::ResumeSession
                | PickerAction::Config
                | PickerAction::Doctor => regex::escape(&item.value),
            };
        }
    }

    pub(super) fn select_first_match(&mut self) {
        let first = self.matching_indices().first().copied();
        if let Some(index) = first {
            self.selected = index;
        }
    }

    pub(super) fn matching_indices(&self) -> PickerMatches<'_> {
        let stale = {
            let cache = self.matches.borrow();
            !cache.initialized || cache.filter != self.filter
        };
        if stale {
            let filter = self.filter.trim();
            let regex = match self.action {
                PickerAction::SelectModel | PickerAction::SelectInternalAgentModel => None,
                PickerAction::LoginGroup
                | PickerAction::LoginProvider
                | PickerAction::LogoutProvider
                | PickerAction::RefreshModelList
                | PickerAction::InsertSkillCommand
                | PickerAction::ViewAgent
                | PickerAction::ResumeSession
                | PickerAction::Config
                | PickerAction::Doctor => (!filter.is_empty())
                    .then(|| {
                        RegexBuilder::new(filter)
                            .case_insensitive(true)
                            .build()
                            .ok()
                    })
                    .flatten(),
            };
            let indices = match self.action {
                PickerAction::SelectModel | PickerAction::SelectInternalAgentModel => {
                    fuzzy_picker_matching_indices(&self.items, filter)
                }
                PickerAction::LoginGroup
                | PickerAction::LoginProvider
                | PickerAction::LogoutProvider
                | PickerAction::RefreshModelList
                | PickerAction::InsertSkillCommand
                | PickerAction::ViewAgent
                | PickerAction::ResumeSession
                | PickerAction::Config
                | PickerAction::Doctor => {
                    picker_matching_indices_with_regex(&self.items, filter, regex.as_ref())
                }
            };
            *self.matches.borrow_mut() = PickerMatchCache {
                initialized: true,
                filter: self.filter.clone(),
                _regex: regex,
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
        .filter_map(|(index, item)| {
            fuzzy_match_score(&item.value, filter).map(|score| (index, score))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(left_index, left_score), (right_index, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_index.cmp(right_index))
    });
    matches.into_iter().map(|(index, _)| index).collect()
}

fn picker_haystack(item: &PickerItem) -> String {
    let detail = item.detail.as_deref().unwrap_or_default();
    let preview = item.preview.as_deref().unwrap_or_default();
    let badge = item
        .badge
        .as_ref()
        .map(|badge| badge.text.as_str())
        .unwrap_or_default();
    format!(
        "{} {} {} {} {}",
        item.label, item.value, detail, preview, badge
    )
}

pub(super) fn fuzzy_match_score(haystack: &str, needle: &str) -> Option<i64> {
    let haystack = haystack.to_lowercase();
    let needle = needle.to_lowercase();
    let haystack_chars = haystack.chars().collect::<Vec<_>>();
    let mut search_start = 0;
    let mut first_match = None;
    let mut previous_match = None;
    let mut score = 0;

    for needle_char in needle.chars() {
        let candidate = haystack_chars[search_start..]
            .iter()
            .enumerate()
            .filter(|(_, haystack_char)| **haystack_char == needle_char)
            .map(|(offset, _)| search_start + offset)
            .max_by_key(|index| fuzzy_character_bonus(&haystack_chars, *index, previous_match))?;
        let index = candidate;
        first_match.get_or_insert(index);
        score += 10;
        score += fuzzy_character_bonus(&haystack_chars, index, previous_match);
        previous_match = Some(index);
        search_start = index + 1;
    }

    let first_match = first_match.unwrap_or_default() as i64;
    let span = previous_match.unwrap_or_default() as i64 - first_match;
    Some(score - first_match - span)
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

impl super::App {
    pub(super) fn reset_navigable_popup_detail_scroll(&mut self) {
        if let super::ComposerMode::Picker(picker) = &mut self.composer {
            if picker.uses_navigable_popup() {
                picker.reset_detail_scroll();
            }
        }
    }

    pub(super) fn open_child_picker(&mut self, child: UiPicker) {
        let previous = std::mem::replace(&mut self.composer, super::ComposerMode::Input);
        let super::ComposerMode::Picker(parent) = previous else {
            unreachable!("child picker requires an active parent picker")
        };
        self.status = child.title.clone();
        self.composer = super::ComposerMode::Picker(child.with_parent(parent));
    }

    pub(super) fn pop_picker_level(&mut self) -> bool {
        let parent = match &mut self.composer {
            super::ComposerMode::Picker(picker) => picker.take_parent(),
            _ => None,
        };
        let Some(parent) = parent else {
            return false;
        };
        self.status = parent.title.clone();
        self.composer = super::ComposerMode::Picker(parent);
        true
    }
}

fn is_word_boundary(ch: char) -> bool {
    matches!(ch, '/' | '\\' | '_' | '-' | '.' | ' ')
}
