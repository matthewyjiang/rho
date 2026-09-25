use crossterm::event::{MouseButton, MouseEventKind};
use pretty_assertions::assert_eq;
use ratatui::{backend::TestBackend, layout::Rect, Terminal};
use rho_sdk::{DefaultSelection, HostChoice, HostInputRequest, HostQuestion, SelectionMode};

use super::{composer_target_at, lift_hovered_hit, ComposerChoice, ComposerHit};
use crate::tui::{
    approval::ApprovalChoice,
    questionnaire::{questionnaire_frame, QuestionnaireComposer, QuestionnaireTarget},
    tests::test_app,
    ComposerMode, QuestionnaireResponseChannel,
};

// Covers: a press lands on the choice painted under it even when the composer
// is scrolled (visible start > 0) or offset on screen; PTY cannot cheaply
// produce a composer taller than its rect.
// Owner: composer pointer hit mapping
#[test]
fn pointer_maps_through_origin_and_visible_start() {
    let hits = [
        ComposerHit::rows(2..4, 'a'),
        ComposerHit {
            lines: 4..5,
            columns: 3..6,
            target: 'b',
            active: false,
        },
    ];
    let origin = Rect::new(2, 10, 20, 3);
    let cases = [
        // (visible start, column, row, expected)
        (0, 2, 12, Some('a')),
        (0, 2, 13, None),
        (1, 2, 11, Some('a')),
        (1, 2, 12, Some('a')),
        (2, 5, 12, Some('b')),
        (2, 8, 12, None),
        (0, 1, 12, None),
        (0, 2, 9, None),
    ];
    for (start, column, row, expected) in cases {
        assert_eq!(
            composer_target_at(&hits, origin, start, column, row),
            expected,
            "start={start} column={column} row={row}"
        );
    }
}

fn composer(questions: Vec<HostQuestion>) -> QuestionnaireComposer {
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
    QuestionnaireComposer::new(
        HostInputRequest::questionnaire("", questions).unwrap(),
        QuestionnaireResponseChannel::new(reply_tx),
    )
}

// Covers: a choice's hit span drifts from its painted rows when choices take
// varying heights (wrapped labels, descriptions, the "(recommended)" overflow
// row, the expanded free-text row), so clicks land on a neighbor.
// Owner: questionnaire render
#[test]
fn choice_hits_tile_each_choice_block_in_order() {
    let long = "a label long enough to wrap onto a second row at this width";
    let question = |selection| {
        HostQuestion::new(
            "q",
            "Pick one?",
            vec![
                HostChoice::new("short", "short"),
                HostChoice::new("desc", "described").description("with a description row"),
                HostChoice::new("long", long),
            ],
            selection,
        )
        .unwrap()
        .default_value(serde_json::json!("long"))
        .default_selection(DefaultSelection::Focused)
        .allow_other()
    };
    let width = 30;
    // (name, mode, type into "other", which choices confirm on double click)
    for (name, selection, type_other, expected_confirms) in [
        (
            "single",
            SelectionMode::One,
            false,
            [true, true, true, false],
        ),
        (
            "multi with other text",
            SelectionMode::Many,
            true,
            [false; 4],
        ),
    ] {
        let mut composer = composer(vec![question(selection)]);
        if type_other {
            assert!(composer.insert_text("typed other text that also wraps around"));
        }
        let frame = questionnaire_frame(&composer, width);
        let choices: Vec<_> = frame
            .hits
            .iter()
            .filter_map(|hit| match hit.target {
                QuestionnaireTarget::Choice { index, confirms } => {
                    Some((index, confirms, hit.lines.clone()))
                }
                QuestionnaireTarget::Question(_) => None,
            })
            .collect();
        let row_text = |row: usize| -> String {
            frame.lines[row]
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        };

        assert_eq!(
            choices.iter().map(|(index, ..)| *index).collect::<Vec<_>>(),
            vec![0, 1, 2, 3],
            "{name}"
        );
        // Blocks are contiguous: each choice ends where the next begins, so
        // no painted row belongs to two choices or to none.
        for pair in choices.windows(2) {
            assert_eq!(pair[0].2.end, pair[1].2.start, "{name}");
        }
        // Each block starts on the row that paints its selection marker.
        for (index, _, lines) in &choices {
            let first = row_text(lines.start);
            assert!(
                first.contains('○')
                    || first.contains('●')
                    || first.contains('□')
                    || first.contains('■'),
                "{name}: choice {index} starts on a non-marker row: {first:?}"
            );
            for row in lines.clone().skip(1) {
                let text = row_text(row);
                assert!(
                    !(text.contains('○')
                        || text.contains('●')
                        || text.contains('□')
                        || text.contains('■')),
                    "{name}: choice {index} swallowed another marker row: {text:?}"
                );
            }
        }
        assert!(
            choices.iter().any(|(_, _, lines)| lines.len() > 1),
            "{name}: fixture should produce multi-row choices"
        );
        let confirms: Vec<_> = choices.iter().map(|(_, confirms, _)| *confirms).collect();
        assert_eq!(confirms, expected_confirms, "{name}");
    }
}

// Covers: tab chip hit columns drift from the painted chips (separators,
// overflow ellipses, check marks), sending clicks to the wrong question.
// Owner: questionnaire render
#[test]
fn tab_chip_hits_cover_their_painted_labels() {
    let questions = (1..=4)
        .map(|index| {
            HostQuestion::new(
                format!("q{index}"),
                format!("question number {index}?"),
                vec![HostChoice::new("a", "a"), HostChoice::new("b", "b")],
                SelectionMode::One,
            )
            .unwrap()
        })
        .collect();
    let mut composer = composer(questions);
    composer.focus_question(3);

    // Narrow enough that the tab bar scrolls and paints a left overflow mark.
    let frame = questionnaire_frame(&composer, 40);
    let mut tabs = Vec::new();
    for hit in &frame.hits {
        let QuestionnaireTarget::Question(index) = hit.target else {
            continue;
        };
        let line: String = frame.lines[hit.lines.start]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let painted: String = line
            .chars()
            .skip(hit.columns.start)
            .take(hit.columns.len())
            .collect();
        tabs.push((index, painted.split_whitespace().next().map(str::to_owned)));
    }
    assert!(
        tabs.len() < 4,
        "tab bar should scroll at this width: {tabs:?}"
    );
    assert!(tabs.iter().any(|(index, _)| *index == 3));
    for (index, label) in tabs {
        assert_eq!(label, Some((index + 1).to_string()));
    }
}

// Covers: a click that races a newly opened approval (before its first paint)
// must not move focus off Deny; the same click after the paint does.
// Owner: composer pointer gating (a PTY cannot order a click before a repaint)
#[tokio::test]
async fn approval_clicks_wait_for_the_prompt_to_be_painted() {
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let request = rho_sdk::ApprovalRequest::new(
        rho_sdk::CapabilityRequest::read_path(
            "/workspace/file",
            rho_sdk::PathScope::PrimaryWorkspace,
            rho_sdk::CapabilitySource::built_in_tool("read_file"),
        ),
        "approval required",
    );
    app.open_approval(rho_sdk::PendingApproval::new(request).0)
        .await;
    let active = |app: &crate::tui::App| match app.input_ui.composer() {
        ComposerMode::Approval(approval) => approval.active(),
        other => panic!("approval closed: {other:?}"),
    };
    let allow_once = |app: &mut crate::tui::App| {
        let ctx = app.frame_context(Rect::new(0, 0, 80, 24));
        let hit = ctx
            .composer
            .choice_hits
            .iter()
            .find(|hit| hit.target == ComposerChoice::Approval(ApprovalChoice::AllowOnce))
            .expect("allow once is painted");
        let row = ctx.layout.composer.y + (hit.lines.start - ctx.layout.composer_start) as u16;
        (ctx.layout.composer.x, row)
    };

    let (column, row) = allow_once(&mut app);
    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        &mut terminal,
    )
    .unwrap();
    assert_eq!(active(&app), ApprovalChoice::Deny);

    terminal.draw(|frame| app.draw(frame)).unwrap();
    let (column, row) = allow_once(&mut app);
    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        &mut terminal,
    )
    .unwrap();
    assert_eq!(active(&app), ApprovalChoice::AllowOnce);
}

fn sectioned_item(section: &str, label: String) -> crate::tui::PickerItem {
    crate::tui::PickerItem {
        section: Some(section.into()),
        value: label.clone(),
        label,
        detail: None,
        preview: None,
        badge: None,
        selection_verb: None,
        allow_filter_completion: true,
    }
}

// Covers: once an inline list picker scrolls (or filters), a click must pick
// the item painted under it by its index in the full item list, and section
// headers must not be clickable; a hit keyed by window offset or match
// position would select a neighbor.
// Owner: inline picker hit mapping (pure frame output; a PTY scenario would
// need a long sectioned picker and cell arithmetic to reach the same state).
#[test]
fn inline_picker_hits_name_absolute_items_and_skip_headers() {
    let items: Vec<_> = (0..12)
        .map(|index| {
            let section = if index < 6 { "ALPHA" } else { "BETA" };
            sectioned_item(section, format!("{}-{index:02}", section.to_lowercase()))
        })
        .collect();
    // (name, filter, selected item, item that must be scrolled out of view)
    let cases = [
        ("scrolled past the first section", "", 10, Some(0)),
        ("filtered to the second section", "beta", 11, None),
    ];
    for (name, filter, selected, hidden) in cases {
        let mut picker = crate::tui::UiPicker::config("sections", items.clone());
        picker.filter = filter.into();
        picker.selected = selected;
        let mut app = test_app();
        app.input_ui.set_composer(ComposerMode::Picker(picker));

        // About ten rows fit: selecting the second-to-last item scrolls the
        // first ones out while the BETA header stays painted.
        let frame = app.composer_frame(80, /*viewport_height*/ 20);
        let text = |line: usize| -> String {
            frame.lines[line]
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        };
        let hits: Vec<_> = frame
            .choice_hits
            .iter()
            .map(|hit| match hit.target {
                ComposerChoice::PickerRow(target) => (target.item, hit.lines.clone()),
                ComposerChoice::Questionnaire(_)
                | ComposerChoice::Approval(_)
                | ComposerChoice::InlineChoice(_) => {
                    panic!("{name}: foreign target {:?}", hit.target)
                }
            })
            .collect();

        assert!(
            hits.iter().any(|(item, _)| *item == selected),
            "{name}: selection painted"
        );
        if let Some(hidden) = hidden {
            assert!(
                hits.iter().all(|(item, _)| *item != hidden),
                "{name}: window scrolled"
            );
        }
        // Each hit covers exactly the row painting its item's label.
        for (item, lines) in &hits {
            assert_eq!(lines.len(), 1, "{name}");
            let row = text(lines.start);
            assert!(
                row.contains(&items[*item].label),
                "{name}: item {item} hit lands on {row:?}"
            );
        }
        // Painted section headers take no hit.
        let header_rows: Vec<usize> = (0..frame.lines.len())
            .filter(|line| matches!(text(*line).trim(), "ALPHA" | "BETA"))
            .collect();
        assert!(!header_rows.is_empty(), "{name}: fixture paints a header");
        for line in header_rows {
            assert!(
                hits.iter().all(|(_, lines)| !lines.contains(&line)),
                "{name}: header row {line} is clickable"
            );
        }
    }
}

// Covers: the hover pass lifts only the hovered target's painted rows, never
// the active (selected) one, clips to the rows the composer window shows, and
// leaves every cell alone when the pointer is off all targets.
// Owner: composer hover paint (pure buffer pass)
#[test]
fn hover_lifts_only_the_hovered_inactive_rows() {
    use ratatui::{
        buffer::Buffer,
        style::{Color, Style},
    };

    let origin = Rect::new(0, 0, 6, 3);
    let hits = [
        ComposerHit::rows(0..1, 'a').with_active(true),
        ComposerHit::rows(1..3, 'b'),
        ComposerHit::rows(3..4, 'c'),
    ];
    // Paint dim RGB ink everywhere, then report the screen rows the lift
    // changed. The lift blends RGB ink (or bolds named ink), so any change
    // to a cell means it was lifted.
    let painted = {
        let mut buffer = Buffer::empty(origin);
        buffer.set_style(origin, Style::default().fg(Color::Rgb(90, 90, 90)));
        buffer
    };
    let lifted_rows = |start: usize, pointer: Option<(u16, u16)>| {
        let mut buffer = painted.clone();
        lift_hovered_hit(&mut buffer, &hits, origin, start, pointer);
        (0..origin.height)
            .filter(|&y| buffer[(0, y)] != painted[(0, y)])
            .collect::<Vec<_>>()
    };
    // (name, visible start, pointer cell, screen rows expected lifted)
    let cases = [
        ("inactive two-row target", 0, Some((2, 2)), vec![1, 2]),
        ("active target stays as painted", 0, Some((2, 0)), vec![]),
        (
            "scrolled: target clipped to the window",
            1,
            Some((2, 0)),
            vec![0, 1],
        ),
        ("pointer off every target", 0, None, vec![]),
    ];
    for (name, start, pointer, expected) in cases {
        assert_eq!(lifted_rows(start, pointer), expected, "{name}");
    }
}
