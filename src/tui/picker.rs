use regex::RegexBuilder;

#[derive(Clone, Debug)]
pub(super) struct UiPicker {
    pub(super) title: String,
    pub(super) help: String,
    pub(super) items: Vec<PickerItem>,
    pub(super) selected: usize,
    pub(super) filter: String,
    pub(super) action: PickerAction,
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
    Selected,
    Favorite,
    Healthy,
    Warning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickerAction {
    SelectModel,
    SelectTitleModel,
    LoginProvider,
    LogoutProvider,
    InsertSkillCommand,
    InsertFilePath,
    ResumeSession,
    Config,
    Doctor,
}

impl PickerAction {
    pub(super) fn space_confirms_selection(self) -> bool {
        match self {
            PickerAction::Config | PickerAction::Doctor => true,
            PickerAction::SelectModel
            | PickerAction::SelectTitleModel
            | PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::InsertSkillCommand
            | PickerAction::InsertFilePath
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
        }
    }

    pub(super) fn select_previous(&mut self) {
        let matches = self.matching_indices();
        if matches.is_empty() {
            return;
        }
        let position = matches
            .iter()
            .position(|index| *index == self.selected)
            .unwrap_or(0);
        self.selected = if position == 0 {
            *matches.last().unwrap()
        } else {
            matches[position - 1]
        };
    }

    pub(super) fn select_next(&mut self) {
        let matches = self.matching_indices();
        if matches.is_empty() {
            return;
        }
        let position = matches
            .iter()
            .position(|index| *index == self.selected)
            .unwrap_or(0);
        self.selected = matches[(position + 1) % matches.len()];
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
        if let Some(item) = self.selected_item() {
            self.filter = regex::escape(&item.value);
        }
    }

    pub(super) fn select_first_match(&mut self) {
        if let Some(index) = self.matching_indices().first().copied() {
            self.selected = index;
        }
    }

    pub(super) fn matching_indices(&self) -> Vec<usize> {
        match self.action {
            PickerAction::InsertFilePath => {
                fuzzy_picker_matching_indices(&self.items, &self.filter)
            }
            PickerAction::SelectModel
            | PickerAction::SelectTitleModel
            | PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::InsertSkillCommand
            | PickerAction::ResumeSession
            | PickerAction::Config
            | PickerAction::Doctor => picker_matching_indices(&self.items, &self.filter),
        }
    }

    pub(super) fn selected_item(&self) -> Option<&PickerItem> {
        self.matching_indices()
            .contains(&self.selected)
            .then(|| self.items.get(self.selected))
            .flatten()
    }
}

pub(super) fn picker_matching_indices(items: &[PickerItem], filter: &str) -> Vec<usize> {
    let filter = filter.trim();
    if filter.is_empty() {
        return (0..items.len()).collect();
    }

    let Ok(regex) = RegexBuilder::new(filter).case_insensitive(true).build() else {
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

fn fuzzy_match_score(haystack: &str, needle: &str) -> Option<i64> {
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

fn is_word_boundary(ch: char) -> bool {
    matches!(ch, '/' | '\\' | '_' | '-' | '.' | ' ')
}
