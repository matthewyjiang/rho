use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;

use super::{inline_choice_frame, InlineChoice, InlineChoiceKeyOutcome, InlineChoiceOption};
use crate::tui::composer_pointer::ComposerChoice;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn navigation_skips_unavailable_options() {
    let mut choice = InlineChoice::new(
        "choose",
        "details",
        vec![
            InlineChoiceOption::unavailable("first", '1', "first", "unavailable"),
            InlineChoiceOption::available("second", '2', "second", "available"),
            InlineChoiceOption::available("third", '3', "third", "available"),
        ],
    )
    .unwrap();

    assert_eq!(choice.selected_value(), "second");
    assert_eq!(
        choice.handle_key(key(KeyCode::Down)),
        InlineChoiceKeyOutcome::Handled
    );
    assert_eq!(choice.selected_value(), "third");
    assert_eq!(
        choice.handle_key(key(KeyCode::Up)),
        InlineChoiceKeyOutcome::Handled
    );
    assert_eq!(choice.selected_value(), "second");
}

#[test]
fn shortcut_selects_and_submits_option() {
    for (case, modifiers) in [
        ("plain", KeyModifiers::NONE),
        ("shift", KeyModifiers::SHIFT),
    ] {
        let mut choice = InlineChoice::new(
            "choose",
            "details",
            vec![
                InlineChoiceOption::available("compact", '1', "compact", "first"),
                InlineChoiceOption::available("direct", '2', "direct", "second"),
            ],
        )
        .unwrap();

        assert_eq!(
            choice.handle_key(KeyEvent::new(KeyCode::Char('2'), modifiers)),
            InlineChoiceKeyOutcome::Selected("direct".into()),
            "{case}"
        );
        assert_eq!(choice.selected_value(), "direct", "{case}");
        assert_eq!(
            choice.handle_key(key(KeyCode::Esc)),
            InlineChoiceKeyOutcome::Cancelled,
            "{case}"
        );
    }
}

// Covers: an option's click span drifts from its painted rows when labels or
// details wrap or blank separators appear between groups, so a click lands on
// (or confirms) a neighbor; unavailable options must take no clicks.
// Owner: inline choice render
#[test]
fn option_hits_tile_each_available_option_group() {
    let options = vec![
        InlineChoiceOption::available("keep", '1', "Keep", "Leave everything as it is"),
        InlineChoiceOption::unavailable("locked", '2', "Locked option", "Needs a login first"),
        InlineChoiceOption::available(
            "delete",
            '3',
            "Delete every saved session in this directory",
            "Permanently removes transcripts and cached web content",
        ),
        InlineChoiceOption::available("later", '4', "Decide later", ""),
    ];
    let choice = InlineChoice::new("Delete sessions?", "", options.clone()).unwrap();
    let markers = ['→', '·', ' '];
    // The option whose marker and shortcut prefix starts `row`, if any.
    let option_at = |row: &str| {
        let row = row.trim_start_matches(markers);
        options
            .iter()
            .position(|option| row.starts_with(&format!("{}  ", option.shortcut)))
    };
    // (width, whether some option group wraps past label + detail rows)
    for (width, wraps) in [(80, false), (24, true)] {
        let frame = inline_choice_frame(&choice, width, /*return_to_parent*/ false);
        let rows: Vec<String> = frame
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        let hits: Vec<_> = frame
            .choice_hits
            .iter()
            .map(|hit| match hit.target {
                ComposerChoice::InlineChoice(index) => (index, hit.lines.clone()),
                ComposerChoice::Questionnaire(_)
                | ComposerChoice::Approval(_)
                | ComposerChoice::PickerRow(_) => {
                    panic!("width {width}: foreign target {:?}", hit.target)
                }
            })
            .collect();

        assert_eq!(
            hits.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            vec![0, 2, 3],
            "width {width}"
        );
        assert_eq!(
            hits.iter().any(|(_, lines)| lines.len() > 2),
            wraps,
            "width {width}: fixture wrap expectation"
        );
        for (index, lines) in &hits {
            let option = &options[*index];
            assert_eq!(
                option_at(&rows[lines.start]),
                Some(*index),
                "width {width}: option {index} starts off its marker row"
            );
            assert!(
                rows[lines.clone()].iter().all(|row| !row.trim().is_empty()),
                "width {width}: option {index} span includes a blank separator"
            );
            // The span paints exactly the label then the detail: no row of
            // this group is left out and none of a neighbor's is taken.
            let painted: Vec<&str> = rows[lines.clone()]
                .iter()
                .enumerate()
                .flat_map(|(offset, row)| {
                    let text = if offset == 0 {
                        row.trim_start_matches(markers)
                            .trim_start_matches(option.shortcut)
                    } else {
                        row.as_str()
                    };
                    text.split_whitespace()
                })
                .collect();
            let expected: Vec<&str> = option
                .label
                .split_whitespace()
                .chain(option.detail.split_whitespace())
                .collect();
            assert_eq!(painted, expected, "width {width}: option {index}");
        }
    }
}
