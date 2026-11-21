use pretty_assertions::assert_eq;
use ratatui::{layout::Size, text::Line};

use super::{FeedMark, ResizeRewind, ScrollbackFeed};

// Covers: a resize never moves the mark past rows the terminal may still
// show, so a reflowing terminal's pulled-back rows are repainted rather than
// lost, and rows it pulls back from before the document return to
// scrollback instead of being painted over.
#[test]
fn resize_rewinds_only_for_a_taller_screen() {
    let size = |width, height| Size { width, height };
    let rewind = |mark_row, foreign_rows| ResizeRewind {
        mark_row,
        foreign_rows,
    };
    let cases = [
        ("taller", 100, size(80, 24), size(80, 30), rewind(94, 0)),
        (
            "taller than the mark",
            4,
            size(80, 24),
            size(80, 30),
            rewind(0, 2),
        ),
        (
            "taller before any write",
            0,
            size(80, 24),
            size(80, 30),
            rewind(0, 6),
        ),
        ("shorter", 100, size(80, 30), size(80, 24), rewind(100, 0)),
        ("narrower", 100, size(80, 24), size(60, 24), rewind(100, 0)),
        // Rows pulled back at a new width no longer line up with the
        // document's, so the mark steps back twice as far.
        (
            "wider and taller",
            100,
            size(80, 24),
            size(100, 30),
            rewind(88, 0),
        ),
        (
            "narrower and taller than twice the mark",
            8,
            size(80, 24),
            size(60, 30),
            rewind(0, 0),
        ),
    ];
    for (name, mark_row, previous, resized, expected) in cases {
        assert_eq!(
            ScrollbackFeed::rewind_after_resize(mark_row, previous, resized),
            expected,
            "{name}"
        );
    }
}

// Covers: rows already written can change under the mark. A surface the
// keyboard left is written again from its start, and removed entries step
// the mark back before them, so no changed row is skipped.
#[test]
fn transcript_changes_rewind_the_mark_before_them() {
    let entry = |index, line| FeedMark::Entry { index, line };
    let cases = [
        ("nothing changed", entry(5, 3), None, None, entry(5, 3)),
        (
            "surface at the mark",
            entry(5, 3),
            Some(5),
            None,
            entry(5, 0),
        ),
        (
            "surface before the mark",
            entry(5, 3),
            Some(2),
            None,
            entry(2, 0),
        ),
        (
            "surface not written yet",
            entry(5, 3),
            Some(7),
            None,
            entry(5, 3),
        ),
        (
            "removed at the mark",
            entry(5, 3),
            None,
            Some(5),
            entry(4, usize::MAX),
        ),
        (
            "removed after the mark",
            entry(5, 3),
            None,
            Some(6),
            entry(5, 3),
        ),
        (
            "every entry removed",
            entry(5, 3),
            None,
            Some(0),
            FeedMark::Header(usize::MAX),
        ),
        (
            "removed before a left surface",
            entry(5, 3),
            Some(4),
            Some(2),
            entry(1, usize::MAX),
        ),
        (
            "header only",
            FeedMark::Header(2),
            Some(0),
            Some(0),
            FeedMark::Header(2),
        ),
    ];
    for (name, mark, left_surface, removed_from, expected) in cases {
        assert_eq!(mark.rewound(left_surface, removed_from), expected, "{name}");
    }
}

// Covers: a new width rewraps the entry the mark is in, and parts of it
// rewrap at different rates, so no row count from the old width says where
// its unwritten rows start; a mark past them would never write them.
#[test]
fn a_new_width_never_marks_unwritten_rows_of_the_marked_entry() {
    let entry = |line| FeedMark::Entry { index: 3, line };
    let lines = |rows: &[&str]| {
        rows.iter()
            .map(|&row| Line::from(row.to_owned()))
            .collect::<Vec<_>>()
    };
    let paragraph = lines(&["aaa bbb", "ccc ddd", "eee"]);
    let widened = lines(&["aaa bbb ccc", "ddd eee"]);
    let cases = [
        (
            "wider, boundary mid-row",
            entry(1),
            paragraph.clone(),
            widened.clone(),
            entry(0),
        ),
        (
            "wider, boundary on a row",
            entry(2),
            paragraph.clone(),
            lines(&["aaa bbb ccc ddd", "eee"]),
            entry(1),
        ),
        (
            "narrower",
            entry(1),
            widened.clone(),
            paragraph.clone(),
            entry(1),
        ),
        // Long written rows halve; short unwritten rows stay.
        (
            "short rows unwritten",
            entry(2),
            lines(&["aaa", "bbb", "c", "d"]),
            lines(&["aaa bbb", "c", "d"]),
            entry(1),
        ),
        (
            "blank rows unwritten",
            entry(1),
            lines(&["aaa", "", "", "bbb"]),
            lines(&["aaa", "", "", "bbb"]),
            entry(1),
        ),
        (
            "other text at the new width",
            entry(2),
            lines(&["| a | b |", "| c | d |", "| e | f |"]),
            lines(&["a  b", "c  d", "e  f"]),
            entry(0),
        ),
        (
            "whole entry",
            entry(3),
            paragraph.clone(),
            widened.clone(),
            entry(2),
        ),
        (
            "header",
            FeedMark::Header(4),
            paragraph.clone(),
            widened.clone(),
            FeedMark::Header(4),
        ),
    ];
    for (name, mark, old, new, expected) in cases {
        assert_eq!(mark.rewrapped(&old, &new), expected, "{name}");
    }
}

// Covers: real entries rewrap unevenly (a paragraph halves, a table switches
// between source text and a boxed layout), so a mark at any row of the old
// width keeps every word below it unwritten at the new width.
#[test]
fn a_new_width_keeps_every_unwritten_word_of_a_rendered_entry() {
    use super::super::{
        render::entry_lines,
        types::{AssistantEntry, Entry},
    };
    let header: Vec<String> = (1..=10).map(|column| format!("h{column}")).collect();
    let row = |row: usize| {
        (1..=10)
            .map(|column| format!("r{row}c{column}"))
            .collect::<Vec<_>>()
    };
    let table_row = |cells: &[String]| format!("| {} |", cells.join(" | "));
    let text = [
        "lorem ipsum dolor sit amet ".repeat(12),
        String::new(),
        table_row(&header),
        table_row(&vec!["---".to_owned(); 10]),
        table_row(&row(1)),
        table_row(&row(2)),
        String::new(),
        "short".to_owned(),
        "tail".to_owned(),
    ]
    .join("\n");
    let entry = Entry::Assistant(AssistantEntry::new(text));
    let words = |rows: &[Line<'_>]| {
        let mut words: Vec<String> = rows
            .iter()
            .flat_map(|row| row.spans.iter())
            .flat_map(|span| span.content.split(|ch: char| !ch.is_alphanumeric()))
            .filter(|word| !word.is_empty())
            .map(str::to_owned)
            .collect();
        words.sort();
        words
    };
    let intersection = |left: &[String], right: &[String]| {
        let mut right = right.to_vec();
        left.iter()
            .filter(|word| match right.binary_search(word) {
                Ok(found) => {
                    right.remove(found);
                    true
                }
                Err(_) => false,
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    let mut layouts = std::collections::BTreeSet::new();
    for width in 30..110 {
        for (old_width, new_width) in [(width, width + 1), (width + 1, width)] {
            let old = entry_lines(&entry, old_width, 20, 10);
            let new = entry_lines(&entry, new_width, 20, 10);
            layouts.insert(old.iter().any(|row| row.to_string().contains('│')));
            for line in 0..old.len() {
                let mark = FeedMark::Entry { index: 0, line }.rewrapped(&old, &new);
                let FeedMark::Entry { line: new_line, .. } = mark else {
                    unreachable!("an entry mark stays an entry mark");
                };
                // Words the new width renders at all, as a narrow boxed
                // table truncates its cells.
                let mut missing = intersection(&words(&old[line..]), &words(&new));
                for word in words(&new[new_line..]) {
                    if let Ok(found) = missing.binary_search(&word) {
                        missing.remove(found);
                    }
                }
                assert!(
                    missing.is_empty(),
                    "{old_width} -> {new_width}, line {line} -> {new_line}: lost {missing:?}"
                );
            }
        }
    }
    assert_eq!(
        layouts.len(),
        2,
        "the table renders both ways in this range"
    );
}
