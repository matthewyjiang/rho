use super::{PickerAction, PickerItem, UiPicker};
use crate::skills::Skill;

pub(super) fn skill_picker(skills: Vec<Skill>) -> UiPicker {
    let items = skills
        .into_iter()
        .map(|skill| PickerItem {
            label: skill.name.clone(),
            detail: Some(skill.description),
            preview: None,
            badge: None,
            value: skill.name,
        })
        .collect::<Vec<_>>();

    UiPicker::new(
        "loaded skills",
        "enter inserts command, type regex filter, esc cancel",
        items,
        PickerAction::InsertSkillCommand,
    )
}
