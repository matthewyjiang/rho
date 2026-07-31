use super::hub_picker;
use pretty_assertions::assert_eq;

#[test]
fn hub_picker_lists_three_sections() {
    let picker = hub_picker(2, 1, 0);
    assert_eq!(picker.title, "workflows");
    assert_eq!(picker.items.len(), 3);
    assert_eq!(picker.items[0].value, "hub:sources");
    assert_eq!(picker.items[1].value, "hub:plans");
    assert_eq!(picker.items[2].value, "hub:runs");
    assert_eq!(
        picker.items[0]
            .badge
            .as_ref()
            .map(|badge| badge.text.as_str()),
        Some("2")
    );
}
