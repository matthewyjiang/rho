use std::io::Read as _;

use flate2::read::ZlibDecoder;
use lopdf::xref::XrefEntry;

use super::{BoundedText, ExtractedText};

const MAX_PDF_EXPANDED_STREAM_BYTES: usize = 64 * 1024 * 1024;
// Match lopdf 0.42's parser limit so older transitive parsers never receive deeper objects.
pub(super) const MAX_PDF_OBJECT_NESTING_DEPTH: usize = 100;

pub(super) fn extract(bytes: &[u8], max_characters: usize) -> Result<ExtractedText, String> {
    // Keep this preflight on a lopdf release that bounds nested objects. pdf-inspector does not
    // accept a preloaded document, so only pass it the same bytes after all Rho limits succeed.
    validate_object_nesting(bytes)?;
    validate_classic_cross_reference(bytes)?;
    let mut document = lopdf::Document::load_mem_with_options(
        bytes,
        lopdf::LoadOptions {
            filter: Some(drop_object_streams),
            strict: true,
            ..Default::default()
        },
    )
    .map_err(|error| error.to_string())?;
    if document
        .reference_table
        .entries
        .values()
        .any(|entry| matches!(entry, XrefEntry::Compressed { .. }))
    {
        return Err(
            "PDF object streams are unsupported because their expansion cannot be bounded".into(),
        );
    }
    if document.is_encrypted() {
        document.decrypt("").map_err(|error| error.to_string())?;
    }
    validate_stream_expansion(&document)?;
    drop(document);

    // pdf-inspector builds Markdown in memory. The preflight above caps all expanded source streams
    // at 64 MiB, and the facade bounds the returned text by Unicode character count here.
    let result = pdf_inspector::process_pdf_mem(bytes).map_err(|error| error.to_string())?;
    let markdown = result.markdown.unwrap_or_default();
    let mut text = BoundedText::new(max_characters);
    text.push_str(&markdown);
    Ok(text.into_extracted())
}

pub(super) fn validate_object_nesting(bytes: &[u8]) -> Result<(), String> {
    let mut index = 0;
    let mut depth = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                index += 1;
                while index < bytes.len() && !matches!(bytes[index], b'\r' | b'\n') {
                    index += 1;
                }
            }
            b'(' => {
                let mut string_depth = 1;
                index += 1;
                while index < bytes.len() && string_depth > 0 {
                    match bytes[index] {
                        b'\\' => index = (index + 2).min(bytes.len()),
                        b'(' => {
                            string_depth += 1;
                            index += 1;
                        }
                        b')' => {
                            string_depth -= 1;
                            index += 1;
                        }
                        _ => index += 1,
                    }
                }
            }
            b'<' if bytes.get(index + 1) == Some(&b'<') => {
                depth += 1;
                check_object_nesting_depth(depth)?;
                index += 2;
            }
            b'>' if bytes.get(index + 1) == Some(&b'>') => {
                depth = depth.saturating_sub(1);
                index += 2;
            }
            b'<' => {
                index += 1;
                while index < bytes.len() && bytes[index] != b'>' {
                    index += 1;
                }
                index += usize::from(index < bytes.len());
            }
            b'[' => {
                depth += 1;
                check_object_nesting_depth(depth)?;
                index += 1;
            }
            b']' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b's' if stream_keyword_at(bytes, index) => {
                let search_start = index + b"stream".len();
                let Some(end_offset) = bytes[search_start..]
                    .windows(b"endstream".len())
                    .position(|window| window == b"endstream")
                else {
                    return Err("PDF stream is missing an endstream marker".to_owned());
                };
                index = search_start + end_offset + b"endstream".len();
            }
            _ => index += 1,
        }
    }
    Ok(())
}

fn check_object_nesting_depth(depth: usize) -> Result<(), String> {
    if depth > MAX_PDF_OBJECT_NESTING_DEPTH {
        return Err(format!(
            "PDF object nesting depth {depth} exceeds the {MAX_PDF_OBJECT_NESTING_DEPTH} level limit"
        ));
    }
    Ok(())
}

fn stream_keyword_at(bytes: &[u8], index: usize) -> bool {
    let keyword = b"stream";
    let Some(end) = index.checked_add(keyword.len()) else {
        return false;
    };
    bytes.get(index..end) == Some(keyword)
        && index > 0
        && bytes[index - 1].is_ascii_whitespace()
        && bytes.get(end).is_some_and(u8::is_ascii_whitespace)
}

fn validate_classic_cross_reference(bytes: &[u8]) -> Result<(), String> {
    let marker = b"startxref";
    let marker_start = bytes
        .windows(marker.len())
        .rposition(|window| window == marker)
        .ok_or_else(|| "PDF has no startxref marker".to_owned())?;
    let offset_start = marker_start + marker.len();
    let offset_text = bytes[offset_start..]
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .take_while(u8::is_ascii_digit)
        .collect::<Vec<_>>();
    let offset = std::str::from_utf8(&offset_text)
        .ok()
        .and_then(|text| text.parse::<usize>().ok())
        .filter(|offset| *offset < bytes.len())
        .ok_or_else(|| "PDF has an invalid startxref offset".to_owned())?;
    let cross_reference = bytes[offset..]
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .take(4)
        .collect::<Vec<_>>();
    if cross_reference != b"xref" {
        return Err(
            "PDF cross-reference streams are unsupported because their expansion cannot be bounded"
                .into(),
        );
    }
    Ok(())
}

fn drop_object_streams(
    id: (u32, u16),
    object: &mut lopdf::Object,
) -> Option<((u32, u16), lopdf::Object)> {
    if object
        .as_stream()
        .is_ok_and(|stream| stream.dict.has_type(b"ObjStm"))
    {
        None
    } else {
        Some((id, object.clone()))
    }
}

fn validate_stream_expansion(document: &lopdf::Document) -> Result<(), String> {
    let mut remaining = MAX_PDF_EXPANDED_STREAM_BYTES;
    for object in document.objects.values() {
        let Ok(stream) = object.as_stream() else {
            continue;
        };
        let filters = stream.filters().unwrap_or_default();
        match filters.as_slice() {
            [] => consume_budget(&mut remaining, stream.content.len())?,
            [b"FlateDecode" | b"Fl"] => {
                let expanded = bounded_flate_size(&stream.content, remaining)?;
                consume_budget(&mut remaining, expanded)?;
            }
            filters
                if filters.iter().any(|filter| {
                    matches!(
                        *filter,
                        b"LZWDecode" | b"LZW" | b"ASCII85Decode" | b"A85" | b"FlateDecode" | b"Fl"
                    )
                }) =>
            {
                let chain = filters
                    .iter()
                    .map(|filter| String::from_utf8_lossy(filter))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(format!(
                    "PDF stream filter chain '[{chain}]' is unsupported by bounded extraction"
                ));
            }
            // Image codecs and other filters are not expanded by lopdf's text-content path.
            _ => {}
        }
    }
    Ok(())
}

fn consume_budget(remaining: &mut usize, size: usize) -> Result<(), String> {
    *remaining = remaining.checked_sub(size).ok_or_else(|| {
        format!("PDF expanded stream data exceeds the {MAX_PDF_EXPANDED_STREAM_BYTES} byte limit")
    })?;
    Ok(())
}

pub(super) fn bounded_flate_size(compressed: &[u8], max: usize) -> Result<usize, String> {
    let mut decoder = ZlibDecoder::new(compressed).take(max.saturating_add(1) as u64);
    let mut expanded = Vec::with_capacity(max.min(64 * 1024));
    decoder
        .read_to_end(&mut expanded)
        .map_err(|error| format!("could not validate PDF stream expansion: {error}"))?;
    if expanded.len() > max {
        return Err(format!("PDF stream expands beyond its {max} byte budget"));
    }
    Ok(expanded.len())
}
