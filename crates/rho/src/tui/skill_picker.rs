use super::{PickerItem, UiPicker};
use crate::skills::Skill;

pub(super) fn skill_picker(skills: Vec<Skill>) -> UiPicker {
    let items = skills
        .into_iter()
        .map(|skill| PickerItem {
            section: None,
            label: skill.name.clone(),
            detail: Some(skill.description),
            preview: None,
            badge: None,
            value: skill.name,
            selection_verb: None,
            allow_filter_completion: true,
        })
        .collect::<Vec<_>>();

    UiPicker::insert_skill("Loaded skills", items)
}
