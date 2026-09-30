// Covers: narrow composer wrapping must keep emoji sequences and trailing
// text visible without unnecessary rows. Owner: interactive UX through PTY.
#[test]
fn composer_unicode_wraps_without_clipping() {
    super::assert_pass("composer_unicode");
}
