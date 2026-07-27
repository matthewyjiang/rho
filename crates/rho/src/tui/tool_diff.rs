use rho_tools::tool_card::DiffRow;

/// Width of the line-number gutter for a diff body.
///
/// Zero when no row carries a number, so patch text without numbering (the
/// `/diff` command) renders without an empty column.
pub(super) fn gutter_width(rows: &[DiffRow]) -> usize {
    rows.iter()
        .filter_map(|row| row.line)
        .max()
        .map_or(0, |line| line.to_string().len())
}

pub(super) fn logical_lines(display_lines: &[String]) -> Vec<String> {
    display_lines
        .iter()
        .flat_map(|line| {
            let lines = line.lines().map(str::to_string).collect::<Vec<_>>();
            if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use rho_tools::tool_card::DiffRowKind;

    use super::*;

    #[test]
    fn sizes_gutter_to_the_widest_line_number() {
        let rows = vec![
            DiffRow::new(DiffRowKind::Context, Some(9), "keep"),
            DiffRow::new(DiffRowKind::Added, Some(1204), "new"),
            DiffRow::new(DiffRowKind::File, None, "src/lib.rs"),
        ];

        assert_eq!(gutter_width(&rows), 4);
    }

    #[test]
    fn drops_the_gutter_when_no_row_is_numbered() {
        let rows = vec![DiffRow::new(DiffRowKind::Added, None, "new")];

        assert_eq!(gutter_width(&rows), 0);
    }
}
