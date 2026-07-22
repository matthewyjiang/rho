use crate::session::tree::{NodeId, SessionTreeItem, SessionTreeItemKind, StoredCompactionFacts};

use super::{tree_picker, PickerBadgeTone, PickerLayout};

fn item(
    id: &str,
    depth: usize,
    kind: SessionTreeItemKind,
    preview: Option<&str>,
    active: bool,
    on_active_path: bool,
    ancestor_has_next_sibling: Vec<bool>,
    is_last_sibling: bool,
    compaction_facts: Option<StoredCompactionFacts>,
) -> SessionTreeItem {
    SessionTreeItem {
        id: NodeId::from_string(id).unwrap(),
        depth,
        kind,
        first_user_text: preview.map(str::to_string),
        compaction_facts,
        active,
        on_active_path,
        ancestor_has_next_sibling,
        is_last_sibling,
    }
}

#[test]
fn tree_picker_uses_list_only_overlay() {
    let root_id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    let branch_id = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    let picker = tree_picker(vec![
        item(
            root_id,
            0,
            SessionTreeItemKind::Turn,
            Some("root turn"),
            false,
            true,
            Vec::new(),
            false,
            None,
        ),
        item(
            branch_id,
            1,
            SessionTreeItemKind::Turn,
            Some("branch turn with a longer prompt"),
            true,
            true,
            // Depth-1 nodes keep an empty ancestor vector; the sibling glyph is
            // chosen only from is_last_sibling.
            Vec::new(),
            true,
            None,
        ),
    ]);

    assert_eq!(picker.layout, PickerLayout::OverlayList);
    assert!(picker.is_overlay());
    assert!(!picker.shows_detail());
    assert!(!picker.has_scrollable_detail());
    assert_eq!(picker.confirm_action_label(), "restore");
    let chrome = picker.overlay_chrome.as_ref().unwrap();
    assert_eq!(chrome.nav_label, " TREE");
    assert_eq!(chrome.nav_keys_hint, "↑↓ turns");
    assert_eq!(picker.selected, 1);
    assert_eq!(picker.items[0].label, "◆ root turn");
    assert_eq!(
        picker.items[1].label,
        "└─ ◆ branch turn with a longer prompt"
    );
    assert!(picker.items[0].detail.is_none());
    assert!(picker.items[1].detail.is_none());
    assert_eq!(picker.items[1].badge.as_ref().unwrap().text, "active");
    assert_eq!(
        picker.items[1].badge.as_ref().unwrap().tone,
        PickerBadgeTone::Selected
    );
}

#[test]
fn compaction_nodes_keep_facts_in_the_label() {
    let picker = tree_picker(vec![item(
        "cccccccc-cccc-cccc-cccc-cccccccccccc",
        2,
        SessionTreeItemKind::Compaction,
        None,
        false,
        false,
        // Parent at depth 1 still has a later sibling, so draw the vertical guide.
        vec![true],
        false,
        Some(StoredCompactionFacts {
            previous_messages: 12,
            current_messages: 4,
            previous_tokens: 1000,
            current_tokens: 250,
            cost_usd_micros: None,
        }),
    )]);

    assert_eq!(
        picker.items[0].label,
        "│  ├─ ◇ Compacted context (12 → 4 messages)"
    );
    assert!(picker.items[0].detail.is_none());
}
