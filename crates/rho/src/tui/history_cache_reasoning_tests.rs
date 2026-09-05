use pretty_assertions::assert_eq;

use super::*;
use crate::tui::ReasoningEntry;

// Covers: hidden reasoning leaks without a receipt, loses its receipt, or stays
// stale after a visibility toggle. Owner: history cache display/resplice policy.
// PTY fixtures close reasoning; this table also supplies receipt-less entries
// directly and checks that soft updates do not rebuild unrelated entries.
#[test]
fn reasoning_visibility_preserves_receipts_and_resplices_only_reasoning() {
    let _guard = crate::tui::theme::theme_test_lock();
    let width = 40;
    for zen_mode in [false, true] {
        for initially_visible in [false, true] {
            for thought_for in [None, Some(std::time::Duration::from_secs(3))] {
                let mut reasoning = ReasoningEntry::new("retained private reasoning");
                reasoning.thought_for = thought_for;
                let entries = vec![
                    Entry::User("question".into()),
                    Entry::Reasoning(reasoning),
                    Entry::Assistant("answer".into()),
                ];
                let prefix = expected_entry_lines(&entries[0], width);
                let suffix = expected_entry_lines(&entries[2], width);
                let visible = expected_entry_lines(&entries[1], width);
                let receipt = thought_for.map_or_else(Vec::new, |duration| {
                    expected_entry_lines(
                        &Entry::Reasoning(ReasoningEntry::summary_only(duration)),
                        width,
                    )
                });
                assert!(!visible.is_empty());
                if thought_for.is_some() {
                    assert!(!receipt.is_empty(), "receipt must contribute lines");
                    assert_ne!(visible, receipt, "receipt must omit the stored body");
                }

                let mut cache = HistoryLineCache::default();
                // Start cold at each matrix point, then change only reasoning
                // output in both directions without manually invalidating.
                for (step, show_reasoning_output) in
                    [initially_visible, !initially_visible, initially_visible]
                        .into_iter()
                        .enumerate()
                {
                    let settings = HistoryRenderSettings {
                        zen_mode,
                        show_reasoning_output,
                        ..settings(width)
                    };
                    let expected_reasoning = match (zen_mode, show_reasoning_output) {
                        (true, _) => &[][..],
                        (false, true) => visible.as_slice(),
                        (false, false) => receipt.as_slice(),
                    };
                    let expected =
                        [prefix.as_slice(), expected_reasoning, suffix.as_slice()].concat();
                    let renders_before = cache.entry_render_count();
                    let mut actual = Vec::new();
                    cache.extend_visible_lines(
                        &entries,
                        settings,
                        HistoryLineSlice {
                            start: 0,
                            count: usize::MAX,
                        },
                        &mut actual,
                        &no_images,
                    );
                    assert_eq!(
                        actual, expected,
                        "zen={zen_mode}, show={show_reasoning_output}, receipt={thought_for:?}, step={step}"
                    );
                    assert_eq!(
                        cache.entry_ranges,
                        vec![
                            0..prefix.len(),
                            prefix.len()..prefix.len() + expected_reasoning.len(),
                            prefix.len() + expected_reasoning.len()..expected.len(),
                        ],
                        "hidden entries must keep their indices without blank rows"
                    );
                    assert_eq!(
                        cache.line_count(&entries, settings, &no_images),
                        expected.len()
                    );
                    assert_eq!(
                        cache.entry_render_count() - renders_before,
                        if step == 0 { entries.len() as u64 } else { 1 },
                        "reasoning-output changes must resplice only reasoning"
                    );
                }
            }
        }
    }
}
