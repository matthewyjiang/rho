use std::io::{Cursor, Read};

use quick_xml::{escape::unescape, events::Event, Reader};
use zip::ZipArchive;

use super::{BoundedText, BoundedWarnings, ExtractedText};

const MAX_DOCUMENT_XML_BYTES: u64 = 20 * 1024 * 1024;

pub(super) fn extract(
    bytes: &[u8],
    warnings: &mut BoundedWarnings,
    max_characters: usize,
) -> Result<ExtractedText, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let mut document = archive
        .by_name("word/document.xml")
        .map_err(|error| format!("missing word/document.xml: {error}"))?;
    if document.size() > MAX_DOCUMENT_XML_BYTES {
        return Err(format!(
            "word/document.xml exceeds the {MAX_DOCUMENT_XML_BYTES} byte expanded-data limit"
        ));
    }
    let mut xml = Vec::with_capacity(document.size() as usize);
    (&mut document)
        .take(MAX_DOCUMENT_XML_BYTES + 1)
        .read_to_end(&mut xml)
        .map_err(|error| error.to_string())?;
    if xml.len() as u64 > MAX_DOCUMENT_XML_BYTES {
        return Err(format!(
            "word/document.xml exceeds the {MAX_DOCUMENT_XML_BYTES} byte expanded-data limit"
        ));
    }
    drop(document);

    if archive.len() > 1_000 {
        warnings.push(
            "DOCX contains more than 1000 archive entries; only the main document was read".into(),
        );
    }
    extract_document_xml(&xml, max_characters)
}

fn extract_document_xml(xml: &[u8], max_characters: usize) -> Result<ExtractedText, String> {
    let mut reader = Reader::from_reader(xml);
    let mut output = BoundedText::new(max_characters);
    let mut in_text = false;
    let mut in_table = false;
    let mut in_table_cell = false;

    loop {
        match reader.read_event().map_err(|error| error.to_string())? {
            Event::Start(event) => match event.local_name().as_ref() {
                b"t" => in_text = true,
                b"tbl" => in_table = true,
                b"tr" => {
                    output.push('|');
                }
                b"tc" => in_table_cell = true,
                b"tab" => {
                    output.push('\t');
                }
                b"br" | b"cr" => push_separator(&mut output, '\n'),
                _ => {}
            },
            Event::Empty(event) => match event.local_name().as_ref() {
                b"tab" => {
                    output.push('\t');
                }
                b"br" | b"cr" => push_separator(&mut output, '\n'),
                _ => {}
            },
            Event::Text(text) if in_text => {
                output.push_str(&text.decode().map_err(|error| error.to_string())?);
            }
            Event::CData(text) if in_text => {
                output.push_str(&text.decode().map_err(|error| error.to_string())?);
            }
            Event::GeneralRef(reference) if in_text => {
                let reference = reference.decode().map_err(|error| error.to_string())?;
                let encoded = format!("&{reference};");
                output.push_str(&unescape(&encoded).map_err(|error| error.to_string())?);
            }
            Event::End(event) => match event.local_name().as_ref() {
                b"t" => in_text = false,
                b"tc" => {
                    in_table_cell = false;
                    trim_suffix(&mut output, "<br>");
                    output.push_str(" |");
                }
                b"tr" => {
                    push_separator(&mut output, '\n');
                }
                b"tbl" => {
                    in_table = false;
                    push_separator(&mut output, '\n');
                }
                b"p" if in_table_cell => {
                    output.push_str("<br>");
                }
                b"p" if !in_table => push_separator(&mut output, '\n'),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        if output.truncated() {
            break;
        }
    }
    output.remove_suffix("\n");
    Ok(output.into_extracted())
}

fn push_separator(output: &mut BoundedText, separator: char) {
    if !output.is_empty() && !output.ends_with_char(separator) {
        output.push(separator);
    }
}

fn trim_suffix(output: &mut BoundedText, suffix: &str) {
    output.remove_suffix(suffix);
}
