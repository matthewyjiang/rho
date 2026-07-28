use std::path::Path;

use pretty_assertions::assert_eq;

use super::*;
use crate::herdr::HerdrReporter;

#[test]
fn pane_command_quotes_every_shell_argument() {
    assert_eq!(
        pane_attach_command_for_executable(Some(Path::new("/tmp/rho`*?[x]")), "a1b2c3"),
        "'/tmp/rho`*?[x]' 'attach' 'a1b2c3'"
    );
    assert_eq!(
        pane_attach_command_for_executable(None, "a1b2c3"),
        "'rho' 'attach' 'a1b2c3'"
    );
    assert_eq!(shell_single_quote("a'b"), r"'a'\''b'");
}

#[test]
fn destination_is_clipboard_without_herdr() {
    assert_eq!(
        destination(&HerdrReporter::default()),
        AttachDestination::Clipboard
    );
}

#[test]
fn destination_uses_herdr_when_configured() {
    let herdr = HerdrReporter::from_env_vars(|key| match key {
        "HERDR_ENV" => Some("1".into()),
        "HERDR_SOCKET_PATH" => Some("/tmp/herdr.sock".into()),
        "HERDR_PANE_ID" => Some("w1:p1".into()),
        _ => None,
    });
    #[cfg(unix)]
    assert_eq!(destination(&herdr), AttachDestination::HerdrPane);
    #[cfg(not(unix))]
    assert_eq!(destination(&herdr), AttachDestination::Clipboard);
}
