use std::io;

use super::prompt_from_reader;

#[test]
fn prompt_joins_inline_parts() {
    let mut stdin = io::empty();
    let prompt = prompt_from_reader(
        vec!["review".into(), "this".into()],
        /*read_stdin*/ false,
        &mut stdin,
    )
    .unwrap();

    assert_eq!(prompt, "review this");
}

#[test]
fn prompt_combines_inline_and_stdin() {
    let mut stdin = "diff contents".as_bytes();
    let prompt =
        prompt_from_reader(vec!["review".into()], /*read_stdin*/ true, &mut stdin).unwrap();

    assert_eq!(prompt, "review\n\ndiff contents");
}

#[test]
fn prompt_requires_input() {
    let mut stdin = io::empty();
    let error = prompt_from_reader(Vec::new(), /*read_stdin*/ false, &mut stdin).unwrap_err();

    assert!(error.to_string().contains("requires a prompt"));
}
