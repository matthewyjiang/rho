use super::*;

#[test]
fn display_escapes_and_round_trips() {
    let pointer = Pointer::parse("/a~1b/c~0d/~01").expect("valid pointer");
    assert_eq!(pointer.segments(), ["a/b", "c~d", "~1"]);
    assert_eq!(pointer.to_string(), "/a~1b/c~0d/~01");
    assert_eq!(Pointer::parse(&pointer.to_string()), Ok(pointer));
    assert_eq!(Pointer::root().to_string(), "/");
}

#[test]
fn forbidden_segments_are_whole_segments() {
    assert!(Pointer::parse("/a/__proto__").is_err());
    assert!(Pointer::parse("/constructors").is_ok());
}
