use pretty_assertions::assert_eq;

use super::*;
use crate::tui::picker::PickerBadgeTone;

fn texts(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

// Covers: the collapsed prompt must never spill past its row budget, and a
// cut must be visible so users know Enter shows more.
// Owner: picker detail layout
#[test]
fn excerpt_clips_to_row_budget_with_visible_cut() {
    let cases = [
        // (text, rows, width, expected)
        ("one two three", 3, 20, vec!["one two three"]),
        (
            "alpha beta\ngamma delta epsilon zeta",
            2,
            12,
            vec!["alpha beta", "gamma delta…"],
        ),
        ("aaaa bbbb cccc dddd", 1, 9, vec!["aaaa bb…"]),
    ];
    for (text, rows, width, expected) in cases {
        let lines = excerpt_lines(text, rows, width);
        assert_eq!(
            texts(&lines),
            expected,
            "{text:?} rows={rows} width={width}"
        );
        for line in texts(&lines) {
            assert!(display_width(&line) <= width, "{line:?} exceeds {width}");
        }
    }
}

// Covers: field values share one column so facts scan vertically, and a
// note either trails the value or wraps under it instead of overflowing.
// Owner: picker detail layout
#[test]
fn fields_align_values_and_place_notes() {
    let fields = [
        DetailField::new("Model", "gpt", DetailTone::Normal).with_note("override"),
        DetailField::new("Reasoning", "high", DetailTone::Normal),
        DetailField::new("Source", "/very/long/path/agent.md", DetailTone::Normal).keep_end(),
    ];
    assert_eq!(
        texts(&field_lines(&fields, 24)),
        vec![
            "Model      gpt  override",
            "Reasoning  high",
            "Source     …ath/agent.md",
        ]
    );
    // Narrower: the note no longer fits after the value and wraps below it.
    assert_eq!(
        texts(&field_lines(&fields[..1], 16)),
        vec!["Model  gpt", "       override"]
    );
}

// Covers: no sheet row may exceed the pane width, or the overlay border and
// scrollbar gutter shift.
// Owner: picker detail layout
#[test]
fn sheet_rows_fit_every_width() {
    let sheet = PickerDetail::Sheet(DetailSheet {
        blocks: vec![
            DetailBlock::Title {
                text: "permission-classifier".into(),
                tag: Some(PickerBadge {
                    text: "● read-only".into(),
                    tone: PickerBadgeTone::Muted,
                }),
            },
            DetailBlock::Paragraph("Classifies pending permission requests quickly.".into()),
            DetailBlock::Rule,
            DetailBlock::Fields(vec![
                DetailField::new("Model", "openai/gpt-5.5", DetailTone::Muted)
                    .with_note("conversation model"),
                DetailField::new("Source", "~/.rho/agents/x.md", DetailTone::Normal).keep_end(),
            ]),
            DetailBlock::Heading {
                label: "PROMPT".into(),
                status: "replaces system prompt · 12 lines".into(),
            },
            DetailBlock::Excerpt {
                text: "You classify requests. ".repeat(10),
                rows: 3,
            },
        ],
    });
    for width in [1, 4, 8, 14, 22, 40, 80] {
        for line in texts(&detail_lines(&sheet, width)) {
            assert!(
                display_width(&line) <= width,
                "width {width}: {line:?} is {} wide",
                display_width(&line)
            );
        }
    }
}
