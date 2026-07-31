use std::io::Read as _;

use flate2::read::ZlibDecoder;
use pdf_extract::xref::XrefEntry;

use super::{BoundedText, ExtractedText};

const MAX_PDF_EXPANDED_STREAM_BYTES: usize = 64 * 1024 * 1024;

impl<'a> pdf_extract::ConvertToFmt for &'a mut BoundedText {
    type Writer = &'a mut BoundedText;

    fn convert(self) -> Self::Writer {
        self
    }
}

pub(super) fn extract(bytes: &[u8], max_characters: usize) -> Result<ExtractedText, String> {
    validate_classic_cross_reference(bytes)?;
    let mut document = pdf_extract::Document::load_mem_with_options(
        bytes,
        pdf_extract::LoadOptions {
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
    let mut text = BoundedText::new(max_characters);
    let result = {
        let mut output = pdf_extract::PlainTextOutput::new(&mut text);
        pdf_extract::output_doc(&document, &mut output)
    };
    if let Err(error) = result {
        if !text.truncated() {
            return Err(error.to_string());
        }
    }
    Ok(text.into_extracted())
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
    object: &mut pdf_extract::Object,
) -> Option<((u32, u16), pdf_extract::Object)> {
    if object
        .as_stream()
        .is_ok_and(|stream| stream.dict.has_type(b"ObjStm"))
    {
        None
    } else {
        Some((id, object.clone()))
    }
}

fn validate_stream_expansion(document: &pdf_extract::Document) -> Result<(), String> {
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
                    matches!(*filter, b"LZWDecode" | b"LZW" | b"ASCII85Decode" | b"A85")
                }) =>
            {
                return Err(format!(
                    "PDF stream filter '{}' is unsupported by bounded extraction",
                    String::from_utf8_lossy(
                        filters
                            .iter()
                            .find(|filter| {
                                matches!(
                                    **filter,
                                    b"LZWDecode" | b"LZW" | b"ASCII85Decode" | b"A85"
                                )
                            })
                            .expect("matching filter checked above")
                    )
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
        format!(
            "PDF expanded stream data exceeds the {} byte limit",
            MAX_PDF_EXPANDED_STREAM_BYTES
        )
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
