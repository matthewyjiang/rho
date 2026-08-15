use pretty_assertions::assert_eq;
use ratatui::layout::Rect;

use super::{
    bottom_chrome_heights, split_interactive_budget, terminal_meets_minimum,
    visible_composer_start, BottomChrome, InteractiveBudget, MIN_TERMINAL_HEIGHT,
    MIN_TERMINAL_WIDTH,
};

// Covers: moving the caret within the visible composer must not move the text
// under an active pointer gesture.
// Owner: pure layout
#[test]
fn composer_view_stays_put_until_cursor_leaves_it() {
    assert_eq!(visible_composer_start(9, 10, 3, 0), 7);
    assert_eq!(visible_composer_start(7, 10, 3, 7), 7);
    assert_eq!(visible_composer_start(8, 10, 3, 7), 7);
    assert_eq!(visible_composer_start(6, 10, 3, 7), 6);
    assert_eq!(visible_composer_start(9, 10, 3, 6), 7);
    assert_eq!(visible_composer_start(0, 2, 3, 1), 0);
}

// Covers: tiny terminals must not enter the normal chrome layout.
// Owner: pure layout
#[test]
fn terminal_minimum_rejects_short_or_narrow_areas() {
    let cases = [
        (MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT, true),
        (MIN_TERMINAL_WIDTH - 1, MIN_TERMINAL_HEIGHT, false),
        (MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT - 1, false),
        (0, 0, false),
        (80, 24, true),
    ];
    for (width, height, expected) in cases {
        assert_eq!(
            terminal_meets_minimum(Rect::new(0, 0, width, height)),
            expected,
            "area {width}x{height}"
        );
    }
}

// Covers: statusline cannot consume the last composer row on short terminals.
// Owner: pure layout
#[test]
fn bottom_chrome_keeps_composer_row_before_statusline() {
    let cases = [
        (
            4usize,
            2usize,
            1usize,
            0usize,
            BottomChrome {
                statusline_height: 2,
                bottom_divider_height: 1,
                command_height: 0,
            },
        ),
        (
            3,
            2,
            1,
            0,
            BottomChrome {
                statusline_height: 2,
                bottom_divider_height: 0,
                command_height: 0,
            },
        ),
        (
            2,
            2,
            1,
            0,
            BottomChrome {
                statusline_height: 1,
                bottom_divider_height: 0,
                command_height: 0,
            },
        ),
        (
            1,
            2,
            1,
            0,
            BottomChrome {
                statusline_height: 0,
                bottom_divider_height: 0,
                command_height: 0,
            },
        ),
        (
            6,
            2,
            1,
            3,
            BottomChrome {
                statusline_height: 2,
                bottom_divider_height: 1,
                command_height: 2,
            },
        ),
        (
            4,
            2,
            0,
            0,
            BottomChrome {
                statusline_height: 2,
                bottom_divider_height: 1,
                command_height: 0,
            },
        ),
    ];

    for (height, desired_statusline, composer_lines, command_lines, expected) in cases {
        assert_eq!(
            bottom_chrome_heights(height, desired_statusline, composer_lines, command_lines),
            expected,
            "height={height} statusline={desired_statusline} composer={composer_lines} commands={command_lines}"
        );
        let reserved =
            expected.statusline_height + expected.bottom_divider_height + expected.command_height;
        let available_above = height.saturating_sub(reserved);
        if composer_lines > 0 {
            assert!(
                available_above >= 1,
                "composer lost its row at height={height}"
            );
        }
    }
}

// Covers: interactive claims follow a single priority order and keep the
// activity-history floor.
// Owner: pure layout
#[test]
fn interactive_split_follows_claim_priority() {
    let split = split_interactive_budget(InteractiveBudget {
        budget: 8,
        composer_lines: 3,
        desired_pending: 5,
        desired_subagents: 2,
        desired_processes: 2,
        activity_floor: 1,
    });
    assert_eq!(split.composer, 1);
    assert_eq!(split.pending_input, 2);
    assert_eq!(split.subagents, 2);
    assert_eq!(split.processes, 2);
    assert_eq!(split.history, 1);

    let grown = split_interactive_budget(InteractiveBudget {
        budget: 10,
        composer_lines: 1,
        desired_pending: 5,
        desired_subagents: 2,
        desired_processes: 2,
        activity_floor: 1,
    });
    assert_eq!(grown.composer, 1);
    assert_eq!(grown.pending_input, 4);
    assert_eq!(grown.subagents, 2);
    assert_eq!(grown.processes, 2);
    assert_eq!(grown.history, 1);

    let starved = split_interactive_budget(InteractiveBudget {
        budget: 4,
        composer_lines: 2,
        desired_pending: 3,
        desired_subagents: 2,
        desired_processes: 2,
        activity_floor: 1,
    });
    assert_eq!(starved.composer, 1);
    assert_eq!(starved.pending_input, 2);
    assert_eq!(starved.subagents, 0);
    assert_eq!(starved.processes, 0);
    assert_eq!(starved.history, 1);
}
