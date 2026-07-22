use crate::session::tree::{NodeId, SessionTreeItem, SessionTreeItemKind, StoredCompactionFacts};

use super::{tree_picker, PickerBadgeTone, PickerLayout};

fn turn(
    id: &str,
    depth: usize,
    preview: &str,
    active: bool,
    on_active_path: bool,
    ancestor_has_next_sibling: Vec<bool>,
    is_last_sibling: bool,
) -> SessionTreeItem {
    SessionTreeItem {
        id: NodeId::from_string(id).unwrap(),
        depth,
        kind: SessionTreeItemKind::Turn,
        first_user_text: Some(preview.to_string()),
        compaction_facts: None,
        active,
        on_active_path,
        ancestor_has_next_sibling,
        is_last_sibling,
    }
}

fn compaction(
    id: &str,
    depth: usize,
    ancestor_has_next_sibling: Vec<bool>,
    is_last_sibling: bool,
    facts: StoredCompactionFacts,
) -> SessionTreeItem {
    SessionTreeItem {
        id: NodeId::from_string(id).unwrap(),
        depth,
        kind: SessionTreeItemKind::Compaction,
        first_user_text: None,
        compaction_facts: Some(facts),
        active: false,
        on_active_path: false,
        ancestor_has_next_sibling,
        is_last_sibling,
    }
}

#[test]
fn tree_picker_uses_list_only_overlay() {
    let root_id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    let branch_id = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    let picker = tree_picker(vec![
        turn(
            root_id,
            0,
            "root turn",
            /*active*/ false,
            /*on_active_path*/ true,
            Vec::new(),
            /*is_last_sibling*/ false,
        ),
        turn(
            branch_id,
            1,
            "branch turn with a longer prompt",
            /*active*/ true,
            /*on_active_path*/ true,
            // Depth-1 nodes keep an empty ancestor vector; the sibling glyph is
            // chosen only from is_last_sibling.
            Vec::new(),
            /*is_last_sibling*/ true,
        ),
    ]);

    assert_eq!(picker.layout, PickerLayout::Overlay);
    assert!(picker.is_overlay());
    assert!(!picker.has_item_details());
    assert!(!picker.has_scrollable_detail());
    assert_eq!(picker.confirm_action_label(), "restore");
    let chrome = picker.overlay_chrome.as_ref().unwrap();
    assert_eq!(chrome.nav_label, " TREE");
    assert!(chrome.detail_label.is_none());
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
    let picker = tree_picker(vec![compaction(
        "cccccccc-cccc-cccc-cccc-cccccccccccc",
        2,
        // Parent at depth 1 still has a later sibling, so draw the vertical guide.
        vec![true],
        /*is_last_sibling*/ false,
        StoredCompactionFacts {
            previous_messages: 12,
            current_messages: 4,
            previous_tokens: 1000,
            current_tokens: 250,
            cost_usd_micros: None,
        },
    )]);

    assert_eq!(
        picker.items[0].label,
        "│  ├─ ◇ Compacted context (12 → 4 messages)"
    );
    assert!(picker.items[0].detail.is_none());
}
