use pretty_assertions::assert_eq;

use super::*;

#[test]
fn bat_line_rejects_bad_script_path() {
    assert_eq!(
        bat_raw_arg_tail(Path::new(r#"C:\bad"name.cmd"#), &["x"]).unwrap_err(),
        WindowsShimArgError::InvalidScriptPath
    );
    assert_eq!(
        bat_raw_arg_tail(Path::new(r"C:\dir\"), &["x"]).unwrap_err(),
        WindowsShimArgError::InvalidScriptPath
    );
}
