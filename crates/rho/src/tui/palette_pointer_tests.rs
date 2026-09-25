use pretty_assertions::assert_eq;
use tempfile::tempdir;

use crate::tui::{
    composer_pointer::ComposerHit, palette::PaletteRow, tests::test_app, MAX_COMMAND_SUGGESTIONS,
};

/// One full-width hit per painted row, in paint order, with `selected` marked
/// active.
fn row_hits(
    rows: impl IntoIterator<Item = PaletteRow>,
    selected: PaletteRow,
) -> Vec<ComposerHit<PaletteRow>> {
    rows.into_iter()
        .enumerate()
        .map(|(line, row)| ComposerHit::rows(line..line + 1, row).with_active(row == selected))
        .collect()
}

// Covers: once the highlight scrolls the palette window, a click must pick the
// match painted under it (absolute index), not the match at that window
// offset; the file palette's scroll footer is not a pickable row.
// Owner: palette pointer hit mapping (pure frame output; a PTY scenario would
// need eight arrow presses and cell arithmetic to reach the same state).
#[test]
fn scrolled_palette_hits_name_absolute_matches() {
    let workspace = tempdir().unwrap();
    for index in 0..7 {
        std::fs::write(workspace.path().join(format!("file-{index}.txt")), "").unwrap();
    }
    let cases = [
        (
            "command window scrolled past the first rows",
            "/",
            /*selection*/ 7,
            row_hits((3..=7).map(PaletteRow::Command), PaletteRow::Command(7)),
            MAX_COMMAND_SUGGESTIONS,
        ),
        (
            "file window scrolled, footer painted without a hit",
            "@",
            /*selection*/ 6,
            row_hits((2..=6).map(PaletteRow::File), PaletteRow::File(6)),
            MAX_COMMAND_SUGGESTIONS + 1,
        ),
    ];
    for (name, text, selection, expected_hits, expected_lines) in cases {
        let mut app = test_app();
        app.info.runtime.cwd = workspace.path().to_path_buf();
        app.input_ui.set_text(text.to_string());
        app.input_ui.set_cursor(1);
        app.input_ui.move_command_selection(selection);
        app.input_ui.set_file_selection(selection);

        let frame = app.command_suggestion_lines(80);

        assert_eq!(frame.hits, expected_hits, "{name}");
        assert_eq!(frame.lines.len(), expected_lines, "{name}");
    }
}
