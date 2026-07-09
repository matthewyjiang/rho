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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickerAction {
    SelectModel,
    SelectTitleModel,
    LoginProvider,
    LogoutProvider,
    InsertSkillCommand,
    ResumeSession,
    Config,
}

impl PickerAction {
    pub(super) fn space_confirms_selection(self) -> bool {
        match self {
            PickerAction::Config => true,
            PickerAction::SelectModel
            | PickerAction::SelectTitleModel
            | PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::InsertSkillCommand
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
        picker_matching_indices(&self.items, &self.filter)
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
        .filter_map(|(index, item)| {
            let detail = item.detail.as_deref().unwrap_or_default();
            let preview = item.preview.as_deref().unwrap_or_default();
            let badge = item
                .badge
                .as_ref()
                .map(|badge| badge.text.as_str())
                .unwrap_or_default();
            let haystack = format!(
                "{} {} {} {} {}",
                item.label, item.value, detail, preview, badge
            );
            regex.is_match(&haystack).then_some(index)
        })
        .collect()
}
