use super::install;

#[test]
fn empty_or_whitespace_filter_is_a_no_op() {
    install(None);
    install(Some(""));
    install(Some("   "));
}
