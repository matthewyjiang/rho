use super::{PickerItem, UiPicker};
use crate::skills::Skill;

pub(super) fn skill_picker(skills: Vec<Skill>) -> UiPicker {
    let items = skills
        .into_iter()
        .map(|skill| PickerItem {
            detail: Some(skill.description.into()),
            ..PickerItem::new(skill.name.clone(), skill.name)
        })
        .collect::<Vec<_>>();

    UiPicker::insert_skill("Loaded skills", items)
}
