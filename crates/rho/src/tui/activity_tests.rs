use super::*;

#[test]
fn bottom_follow_activity_inset_only_when_activity_and_pinned() {
    assert_eq!(bottom_follow_activity_inset(false, true), 0);
    assert_eq!(bottom_follow_activity_inset(true, false), 0);
    assert_eq!(
        bottom_follow_activity_inset(true, true),
        ACTIVITY_RAIL_ROWS + ACTIVITY_CONTENT_GAP_ROWS
    );
}
