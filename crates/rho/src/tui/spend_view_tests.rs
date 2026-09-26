use pretty_assertions::assert_eq;

use super::*;

// Covers: dollar amounts round to the cent, keep thousands separators, and
// never show sub-cent spend as $0.00.
// Owner: spend view
#[test]
fn money_rounds_to_cents_and_keeps_sub_cent_spend_visible() {
    let cases = [
        (0, "$0.00"),
        (1, "<$0.01"),
        (9_999, "<$0.01"),
        (10_000, "$0.01"),
        (14_999, "$0.01"),
        (15_000, "$0.02"),
        (1_288_587_460, "$1,288.59"),
        (4_352_000_000_000, "$4,352,000.00"),
    ];
    for (micros, expected) in cases {
        assert_eq!(money(micros), expected, "{micros}");
    }
}

// Covers: every chart column maps to a bucket so the newest bucket is always
// drawn, merged columns take the max (not a taller sum), and short ranges
// stretch each bucket across at most MAX_BUCKET_COLUMNS columns.
// Owner: spend view
#[test]
fn chart_columns_cover_every_bucket_through_the_newest() {
    let ramp = |len: u64| (1..=len).collect::<Vec<_>>();
    let cases: [(Vec<u64>, usize, Vec<u64>); 5] = [
        (vec![], 10, vec![]),
        (vec![1, 2, 3], 6, vec![1, 1, 2, 2, 3, 3]),
        (vec![1, 2, 3], 60, [[1; 6], [2; 6], [3; 6]].concat()),
        (vec![1, 5, 2, 9, 4], 2, vec![5, 9]),
        // Merged days take the busier one, never 1 + 5.
        (vec![1, 5, 2], 2, vec![1, 5]),
    ];
    for (values, width, expected) in cases {
        assert_eq!(
            chart_columns(&values, width),
            expected,
            "{values:?} @ {width}"
        );
    }

    // Uneven fits at real widths: 24 hours and 30 days across 58 and 70
    // columns fill the width and end on the newest bucket.
    for (len, width) in [(24, 58), (24, 70), (30, 58), (30, 70), (62, 58)] {
        let columns = chart_columns(&ramp(len), width);
        assert_eq!(
            (columns.len(), columns.first(), columns.last()),
            (width, Some(&1), Some(&len)),
            "{len} buckets @ {width}"
        );
    }
}

// Covers: folding a long table into `+ N more` must keep every request and
// dollar, and never fold a lone row.
// Owner: spend view
#[test]
fn fold_tail_keeps_totals_and_skips_single_row_folds() {
    let group = |requests: u64| SpendGroup {
        name: format!("m{requests}"),
        totals: SpendTotals {
            requests,
            tokens: requests * 10,
            actual_usd_micros: requests,
            computed_usd_micros: requests * 2,
            local_requests: 0,
            unpriced_requests: 1,
        },
    };
    let groups: Vec<_> = (1..=10).map(group).collect();
    let cases = [
        (3, 8, 3, None),
        // One extra row stays visible instead of becoming `1 more`.
        (9, 8, 9, None),
        (
            10,
            8,
            8,
            Some((
                2,
                SpendTotals {
                    requests: 19,
                    tokens: 190,
                    actual_usd_micros: 19,
                    computed_usd_micros: 38,
                    local_requests: 0,
                    unpriced_requests: 2,
                },
            )),
        ),
    ];
    for (len, max, shown, rest) in cases {
        let (visible, folded) = fold_tail(&groups[..len], max);
        assert_eq!(
            (visible.len(), folded),
            (shown, rest),
            "{len} rows, max {max}"
        );
    }
}
