//! PDF extraction with byte preflight and shared lopdf load checks.

mod byte_scan;

use super::{BoundedText, ExtractedText};

#[cfg(test)]
pub(super) use byte_scan::MAX_PDF_OBJECT_NESTING_DEPTH;
pub(super) use byte_scan::{bounded_flate_size, validate_object_nesting};

use byte_scan::{consume_budget, MAX_PDF_EXPANDED_STREAM_BYTES};

pub(super) fn extract(bytes: &[u8], max_characters: usize) -> Result<ExtractedText, String> {
    // pdf-inspector only accepts bytes and always loads with its own lopdf 0.41
    // path. Preflight uses that same lopdf version so reject decisions match the
    // extractor stack. A second parse is forced by the crate API; keep it short.
    validate_object_nesting(bytes)?;
    preflight_document(bytes)?;

    // pdf-inspector builds Markdown in memory; the facade then caps Unicode length.
    let result = pdf_inspector::process_pdf_mem(bytes).map_err(|error| error.to_string())?;
    let markdown = result.markdown.unwrap_or_default();
    let mut text = BoundedText::new(max_characters);
    text.push_str(&markdown);
    Ok(text.into_extracted())
}

fn preflight_document(bytes: &[u8]) -> Result<(), String> {
    let mut document = lopdf::Document::load_mem_with_options(
        bytes,
        lopdf::LoadOptions {
            strict: true,
            ..Default::default()
        },
    )
    .map_err(|error| error.to_string())?;
    if document.is_encrypted() {
        document.decrypt("").map_err(|error| error.to_string())?;
    }
    // Defense in depth after load. Byte preflight already budgets Flate streams,
    // including ObjStm and cross-reference streams, before either parser runs.
    validate_stream_expansion(&document)
}

fn validate_stream_expansion(document: &lopdf::Document) -> Result<(), String> {
    let mut remaining = MAX_PDF_EXPANDED_STREAM_BYTES;
    for object in document.objects.values() {
        let Ok(stream) = object.as_stream() else {
            continue;
        };
        // Image XObjects are not decompressed on the text/Markdown path.
        // Budget only streams the extractor may expand (content, fonts, etc.).
        if stream_is_image(stream) {
            continue;
        }
        let filters = if stream.dict.has(b"Filter") {
            stream
                .filters()
                .map_err(|error| format!("PDF stream has a malformed /Filter entry: {error}"))?
        } else {
            Vec::new()
        };
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
            // Image codecs and other filters are not expanded by the text path.
            _ => {}
        }
    }
    Ok(())
}

fn stream_is_image(stream: &lopdf::Stream) -> bool {
    stream
        .dict
        .get(b"Subtype")
        .and_then(lopdf::Object::as_name)
        .is_ok_and(|name| name == b"Image")
}
