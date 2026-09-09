use super::*;

#[test]
fn preserves_a_draft_in_a_durable_recovery_file() {
    let path = preserve_draft_for_recovery("recovery contents").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "recovery contents");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn removes_only_the_editors_final_line_ending() {
    assert_eq!(remove_editor_final_line_ending("draft\n".into()), "draft");
    assert_eq!(remove_editor_final_line_ending("draft\r\n".into()), "draft");
    assert_eq!(
        remove_editor_final_line_ending("draft\n\n".into()),
        "draft\n"
    );
    assert_eq!(remove_editor_final_line_ending("draft".into()), "draft");
}
