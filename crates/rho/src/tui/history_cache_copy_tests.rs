use pretty_assertions::assert_eq;

use super::*;

// Covers: lazy COPY must follow partial lines, split closure, and a previously
// projected snapshot. Cache metadata is the owner; PTY cannot read the payload.
#[test]
fn streamed_fence_copy_tracks_partial_lines_and_split_closure() {
    for opening in ["```text\n", "~~~text\n"] {
        let mut cache = HistoryLineCache::default();
        let mut entries = vec![Entry::Assistant(opening.into())];
        let marker = &opening[..3];
        let mut snapshot = None;
        for (fragment, expected) in [
            ("λ🙂", "λ🙂"),
            ("\nsecond", "λ🙂\nsecond"),
            ("\n", "λ🙂\nsecond"),
            (
                &marker[..2],
                if marker == "```" {
                    "λ🙂\nsecond\n``"
                } else {
                    "λ🙂\nsecond\n~~"
                },
            ),
            (&marker[2..], "λ🙂\nsecond"),
            ("\nprose", "λ🙂\nsecond"),
        ] {
            let Entry::Assistant(text) = &mut entries[0] else {
                unreachable!();
            };
            text.push_str(fragment);
            cache.entry_appended(0);
            let blocks = cache.code_blocks(&entries, settings(80), &no_images);
            assert_eq!(blocks[0].text.as_ref(), expected);
            if snapshot.is_none() {
                snapshot = Some(Arc::clone(&blocks[0].text));
            }
        }
        assert_eq!(snapshot.as_deref(), Some("λ🙂"));
    }
}
