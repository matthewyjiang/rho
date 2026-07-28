use super::PathGlob;
use crate::tool::ToolError;

#[test]
fn invalid_pattern_names_the_glob() {
    let Err(error) = PathGlob::compile("a[") else {
        panic!("expected invalid glob");
    };
    match error {
        ToolError::Message(message) => {
            assert!(message.contains("invalid glob 'a['"), "{message}");
        }
        other => panic!("expected Message, got {other:?}"),
    }
}
