use std::{fmt::Write as _, io::Cursor};

use calamine::{open_workbook_auto_from_rs, Reader};
use zip::ZipArchive;

use super::{
    BoundedText, BoundedWarnings, ExtractedText, MAX_SPREADSHEET_COLUMNS, MAX_SPREADSHEET_ROWS,
};

const MAX_ARCHIVE_ENTRIES: usize = 10_000;
const MAX_EXPANDED_BYTES: u64 = 100 * 1024 * 1024;

pub(super) fn extract(
    bytes: &[u8],
    warnings: &mut BoundedWarnings,
    max_characters: usize,
) -> Result<ExtractedText, String> {
    if bytes.starts_with(b"PK\x03\x04") {
        validate_archive_limits(bytes)?;
    }
    let mut workbook =
        open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let sheet_names = workbook.sheet_names().to_vec();
    let mut output = BoundedText::new(max_characters);

    'sheets: for sheet_name in sheet_names {
        let range = workbook
            .worksheet_range(&sheet_name)
            .map_err(|error| error.to_string())?;
        if !output.is_empty() {
            output.push('\n');
        }
        if writeln!(output, "## {}", escape_cell(&sheet_name)).is_err() {
            break;
        }

        let width = range.width().min(MAX_SPREADSHEET_COLUMNS);
        if range.width() > MAX_SPREADSHEET_COLUMNS {
            warnings.push(format!(
                "worksheet '{sheet_name}' was limited to {MAX_SPREADSHEET_COLUMNS} columns"
            ));
        }
        if range.height() > MAX_SPREADSHEET_ROWS {
            warnings.push(format!(
                "worksheet '{sheet_name}' was limited to {MAX_SPREADSHEET_ROWS} rows"
            ));
        }
        if width == 0 || range.height() == 0 {
            continue;
        }

        for (row_index, row) in range.rows().take(MAX_SPREADSHEET_ROWS).enumerate() {
            output.push('|');
            for cell in row.iter().take(width) {
                if write!(output, " {} |", escape_cell(&cell.to_string())).is_err() {
                    break 'sheets;
                }
            }
            for _ in row.len().min(width)..width {
                if !output.push_str("  |") {
                    break 'sheets;
                }
            }
            if !output.push('\n') {
                break 'sheets;
            }
            if row_index == 0 {
                if !output.push('|') {
                    break 'sheets;
                }
                for _ in 0..width {
                    if !output.push_str(" --- |") {
                        break 'sheets;
                    }
                }
                if !output.push('\n') {
                    break 'sheets;
                }
            }
        }
    }

    output.trim_end();
    Ok(output.into_extracted())
}

fn validate_archive_limits(bytes: &[u8]) -> Result<(), String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(format!(
            "spreadsheet archive has {} entries; the limit is {MAX_ARCHIVE_ENTRIES}",
            archive.len()
        ));
    }
    let mut expanded_bytes = 0_u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|error| error.to_string())?;
        expanded_bytes = expanded_bytes
            .checked_add(file.size())
            .ok_or_else(|| "spreadsheet expanded-data size overflowed".to_owned())?;
        if expanded_bytes > MAX_EXPANDED_BYTES {
            return Err(format!(
                "spreadsheet expanded data exceeds the {MAX_EXPANDED_BYTES} byte limit"
            ));
        }
    }
    Ok(())
}

fn escape_cell(value: &str) -> String {
    value
        .trim()
        .replace('|', "\\|")
        .replace(['\r', '\n'], "<br>")
}
