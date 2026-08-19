use pretty_assertions::assert_eq;

#[test]
fn attach_command_stays_portable_for_external_terminals() {
    assert_eq!(super::attach_command("a1b2c3"), "rho attach a1b2c3");
}
