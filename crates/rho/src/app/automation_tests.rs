use std::io;

use super::prompt_from_reader;

#[test]
fn prompt_requires_input() {
    let mut stdin = io::empty();
    let error = prompt_from_reader(Vec::new(), /*read_stdin*/ false, &mut stdin).unwrap_err();

    assert!(error.to_string().contains("requires a prompt"));
}
