#[cfg(feature = "document-pdf")]
use flate2::{write::ZlibEncoder, Compression};
#[cfg(feature = "document-pdf")]
use std::fmt::Write as _;
#[cfg(any(feature = "document-docx", feature = "document-spreadsheets"))]
use std::io::Cursor;
#[cfg(any(
    feature = "document-docx",
    feature = "document-pdf",
    feature = "document-spreadsheets"
))]
use std::io::Write as _;

use pretty_assertions::assert_eq;

use super::*;

// Covers: extension and signature detection must select the correct extractor and MIME.
// Owner: document detector
#[test]
fn detects_text_extensions_and_pdf_magic() {
    let cases = [
        (
            "notes.md",
            b"hello".as_slice(),
            DocumentFormat::Text("text/markdown"),
        ),
        (
            "data.json",
            b"{}".as_slice(),
            DocumentFormat::Text("application/json"),
        ),
        ("report.bin", b"%PDF-1.7\n".as_slice(), DocumentFormat::Pdf),
        ("book.xlsx", b"PK\x03\x04".as_slice(), DocumentFormat::Xlsx),
        (
            "legacy.xls",
            b"\xd0\xcf\x11\xe0".as_slice(),
            DocumentFormat::Xls,
        ),
        (
            "document.docx",
            b"PK\x03\x04".as_slice(),
            DocumentFormat::Docx,
        ),
    ];

    for (name, bytes, expected) in cases {
        assert_eq!(detect_format(name, bytes).unwrap(), expected, "{name}");
    }
}

// Covers: extraction must never return more than the public Unicode character cap.
// Owner: document extraction facade
#[test]
fn truncates_extracted_text_on_a_character_boundary() {
    let source = "é".repeat(MAX_EXTRACTED_CHARACTERS + 1);

    let document = extract_document_from_bytes("large.txt", source.as_bytes()).unwrap();

    assert_eq!(document.text.chars().count(), MAX_EXTRACTED_CHARACTERS);
    assert_eq!(document.text.len(), MAX_EXTRACTED_CHARACTERS * 2);
    assert!(document.truncated);
    assert_eq!(
        document.warnings,
        vec![format!(
            "extracted text was truncated at {MAX_EXTRACTED_CHARACTERS} characters"
        )]
    );
}

// Covers: warning producers must not build an unbounded intermediate vector before finalization.
// Owner: document extraction facade
#[test]
fn bounds_warnings_while_collecting_them() {
    let mut warnings = BoundedWarnings::default();
    for index in 0..MAX_DOCUMENT_WARNINGS + 5 {
        warnings.push(format!("warning {index}"));
    }

    let warnings = warnings.finish(/*reserve*/ 1);

    assert_eq!(warnings.len(), MAX_DOCUMENT_WARNINGS - 1);
    assert_eq!(
        warnings.last().map(String::as_str),
        Some("additional extraction warnings were omitted")
    );
}

// Covers: path extraction must reject oversized input before parsing or allocating its body.
// Owner: document extraction facade
#[test]
fn rejects_paths_over_the_input_byte_cap() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("oversized.txt");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_DOCUMENT_INPUT_BYTES as u64 + 1).unwrap();

    let error = extract_document_from_path(path).unwrap_err();

    assert!(matches!(
        error,
        DocumentExtractionError::InputTooLarge {
            name,
            size,
            max: MAX_DOCUMENT_INPUT_BYTES,
        } if name == "oversized.txt" && size == MAX_DOCUMENT_INPUT_BYTES as u64 + 1
    ));
}

// Covers: binary data must fail clearly rather than leak a UTF-8 decoder error.
// Owner: document detector
#[test]
fn rejects_unsupported_binary_data() {
    let error = extract_document_from_bytes("archive.zip", b"PK\x03\x04\0binary").unwrap_err();

    assert_eq!(
        error.to_string(),
        "unsupported document format for 'archive.zip'"
    );
}

// Covers: untrusted filenames must not inject control characters into tool output or chat chips.
// Owner: document extraction facade
#[test]
fn sanitizes_document_names_and_bounds_their_length() {
    let name = format!(
        "unsafe\n{}md",
        "x".repeat(MAX_DOCUMENT_NAME_CHARACTERS + 20)
    );

    let document = extract_document_from_bytes(&name, b"body").unwrap();

    assert!(!document.name.chars().any(char::is_control));
    assert_eq!(
        document.name.chars().count(),
        MAX_DOCUMENT_NAME_CHARACTERS + 3
    );
    assert!(document.name.ends_with("..."));
}

#[cfg(feature = "document-pdf")]
// Covers: the shipped PDF backend must extract text-layer content as structured Markdown.
// Owner: PDF extractor
#[test]
fn extracts_pdf_markdown_and_warns_for_empty_pages() {
    let text_document =
        extract_document_from_bytes("renamed.bin", &pdf_fixture("(Hello PDF) Tj")).unwrap();
    assert_eq!(text_document.mime, "application/pdf");
    assert!(
        text_document.text.contains("Hello PDF"),
        "structured markdown should keep the text layer: {:?}",
        text_document.text
    );
    assert_eq!(text_document.warnings, Vec::<String>::new());

    let empty_document = extract_document_from_bytes("blank.pdf", &pdf_fixture("")).unwrap();
    assert!(empty_document.text.trim().is_empty());
    assert_eq!(
        empty_document.warnings,
        vec![
            "PDF contains no extractable text; it may be empty or contain only scanned images"
                .to_owned()
        ]
    );
}

#[cfg(feature = "document-pdf")]
// Covers: the facade must cap the PDF backend's Markdown at its output budget.
// Owner: document facade
#[test]
fn bounds_pdf_markdown_output() {
    let operation = format!("({}) Tj", "A".repeat(MAX_EXTRACTED_CHARACTERS + 1));

    let document = extract_document_from_bytes("large.pdf", &pdf_fixture(&operation)).unwrap();

    assert_eq!(document.text.chars().count(), MAX_EXTRACTED_CHARACTERS);
    assert!(document.truncated);
}

#[cfg(feature = "document-pdf")]
// Covers: compressed PDF streams are measured with a bounded decoder before extraction.
// Owner: PDF extractor
#[test]
fn rejects_flate_streams_that_exceed_the_expanded_budget() {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&vec![b'A'; 2_000]).unwrap();
    let compressed = encoder.finish().unwrap();

    let error = pdf::bounded_flate_size(&compressed, /*remaining*/ 1_000).unwrap_err();

    assert!(error.contains("remaining 1000 byte budget"));
}

#[cfg(feature = "document-pdf")]
// Covers: chained Flate filters must be rejected rather than bypass the expansion budget.
// Owner: PDF extractor
#[test]
fn rejects_pdf_streams_with_chained_flate_filters() {
    let bytes = pdf_fixture_with_filter("(content) Tj", "/FlateDecode /FlateDecode");

    let error = extract_document_from_bytes("filters.pdf", &bytes).unwrap_err();

    assert!(error
        .to_string()
        .contains("filter chain '[FlateDecode, FlateDecode]'"));
}

#[cfg(feature = "document-pdf")]
// Covers: modern PDF 1.5 object and cross-reference streams must extract when Flate stays in budget.
// Owner: PDF extractor
#[test]
fn extracts_pdf_with_object_and_cross_reference_streams() {
    let document =
        extract_document_from_bytes("modern.pdf", &pdf_fixture_with_object_streams()).unwrap();

    assert!(
        document.text.contains("Hello ObjStm"),
        "modern PDF text layer should extract: {:?}",
        document.text
    );
}

#[cfg(feature = "document-pdf")]
// Covers: /Length must skip stream payloads so an embedded endstream marker does not cut the scan.
// Owner: PDF preflight
#[test]
fn accepts_pdf_stream_payload_containing_endstream_marker() {
    let payload = b"xxxxxendstreamxx";
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let object_offset = bytes.len();
    write!(bytes, "1 0 obj\n<< /Length {} >>\nstream\n", payload.len()).unwrap();
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let xref_offset = bytes.len();
    write!(
        bytes,
        "xref\n0 2\n0000000000 65535 f \n{object_offset:010} 00000 n \ntrailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
    )
    .unwrap();

    assert_eq!(pdf::validate_object_nesting(&bytes), Ok(()));
}

#[cfg(feature = "document-pdf")]
// Covers: a present but invalid /Filter must fail closed instead of counting as unfiltered.
// Owner: PDF preflight
#[test]
fn rejects_pdf_streams_with_malformed_filter_entries() {
    let bytes = pdf_fixture_with_stream_dictionary("(content) Tj", "", "/Filter 123 ");

    let error = extract_document_from_bytes("bad-filter.pdf", &bytes).unwrap_err();

    assert!(error.to_string().contains("malformed /Filter"));
}

#[cfg(feature = "document-pdf")]
// Covers: untrusted PDFs with excessive object nesting must fail before the extraction parser.
// Owner: PDF preflight
#[test]
fn rejects_excessive_pdf_object_nesting_before_extraction() {
    let excessive_depth = pdf::MAX_PDF_OBJECT_NESTING_DEPTH + 1;
    let nested = format!(
        "{}0{}",
        "[".repeat(excessive_depth),
        "]".repeat(excessive_depth)
    );
    let bytes = pdf_fixture_with_catalog_entry("(content) Tj", &format!("/Nested {nested}"));

    let error = extract_document_from_bytes("nested.pdf", &bytes).unwrap_err();

    assert!(matches!(
        error,
        DocumentExtractionError::Extraction { format: "PDF", .. }
    ));
}

#[cfg(feature = "document-pdf")]
// Covers: object-like bytes in PDF comments, strings, and streams must not trigger the nesting cap.
// Owner: PDF preflight
#[test]
fn ignores_pdf_object_tokens_outside_object_syntax() {
    let nested = "[".repeat(pdf::MAX_PDF_OBJECT_NESTING_DEPTH + 1);
    let cases = [
        "/stream".to_owned(),
        format!("% {nested}\n"),
        format!("({nested})"),
        format!("<<>>\nstream\n{nested}\nendstream"),
    ];

    for bytes in cases {
        assert_eq!(pdf::validate_object_nesting(bytes.as_bytes()), Ok(()));
    }
}

#[cfg(feature = "document-docx")]
// Covers: OOXML entities, paragraphs, and table cells must survive the focused DOCX walk.
// Owner: DOCX extractor
#[test]
fn extracts_docx_paragraphs_and_tables() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:r><w:t>Hello &amp; world</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
</w:body></w:document>"#;
    let bytes = zip_fixture(&[("word/document.xml", xml)]);

    let document = extract_document_from_bytes("sample.docx", &bytes).unwrap();

    assert_eq!(document.text, "Hello & world\n|A |B |");
    assert_eq!(document.warnings, Vec::<String>::new());
}

#[cfg(feature = "document-docx")]
// Covers: nested tables must not clear the surrounding table-cell formatting state.
// Owner: DOCX extractor
#[test]
fn preserves_outer_cell_context_after_nested_tables() {
    let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Before</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>Nested</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:t>After</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
</w:body></w:document>"#;

    let document =
        extract_document_from_bytes("nested.docx", &zip_fixture(&[("word/document.xml", xml)]))
            .unwrap();

    assert_eq!(document.text, "|Before<br>|Nested |\nAfter |");
}

#[cfg(feature = "document-docx")]
// Covers: the DOCX XML walk must stop writing when the output budget is exhausted.
// Owner: DOCX extractor
#[test]
fn bounds_docx_output_during_extraction() {
    let body = "A".repeat(MAX_EXTRACTED_CHARACTERS + 1);
    let xml = format!(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{body}</w:t></w:r></w:p></w:body></w:document>"#
    );
    let bytes = zip_fixture(&[("word/document.xml", &xml)]);

    let document = extract_document_from_bytes("large.docx", &bytes).unwrap();

    assert_eq!(document.text.chars().count(), MAX_EXTRACTED_CHARACTERS);
    assert!(document.truncated);
}

#[cfg(feature = "document-spreadsheets")]
// Covers: spreadsheet rendering must enforce per-sheet row and column limits.
// Owner: spreadsheet extractor
#[test]
fn renders_xlsx_as_bounded_markdown_tables() {
    let bytes = xlsx_fixture(MAX_SPREADSHEET_ROWS + 1, MAX_SPREADSHEET_COLUMNS + 1);

    let document = extract_document_from_bytes("sample.xlsx", &bytes).unwrap();

    assert!(document.text.starts_with("## Data\n| R1C1 | R1C2 |"));
    assert!(!document.text.contains("R1C41"));
    assert!(!document.text.contains("R201C1"));
    assert_eq!(
        document.warnings,
        vec![
            format!("worksheet 'Data' was limited to {MAX_SPREADSHEET_COLUMNS} columns"),
            format!("worksheet 'Data' was limited to {MAX_SPREADSHEET_ROWS} rows"),
        ]
    );
}

#[cfg(feature = "document-spreadsheets")]
// Covers: empty worksheets must not produce orphan Markdown headings.
// Owner: spreadsheet extractor
#[test]
fn skips_empty_spreadsheet_worksheets() {
    let document = extract_document_from_bytes("empty.xlsx", &xlsx_fixture(0, 0)).unwrap();

    assert_eq!(document.text, "");
}

#[cfg(feature = "document-spreadsheets")]
// Covers: archive preflight must count bytes actually decompressed, not ZIP metadata alone.
// Owner: spreadsheet extractor
#[test]
fn rejects_spreadsheet_entries_exceeding_expansion_budget() {
    let bytes = zip_fixture(&[("large.xml", &"A".repeat(1_024))]);

    let error = spreadsheet::validate_archive_limits_with_budget(&bytes, 512).unwrap_err();

    assert!(error.contains("exceeds the 512 byte limit"));
}

#[cfg(any(feature = "document-docx", feature = "document-spreadsheets"))]
fn zip_fixture(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(content.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[cfg(feature = "document-spreadsheets")]
fn xlsx_fixture(rows: usize, columns: usize) -> Vec<u8> {
    let mut sheet = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    for row in 1..=rows {
        write!(sheet, "<row r=\"{row}\">").unwrap();
        for column in 1..=columns {
            let reference = format!("{}{row}", spreadsheet_column_name(column));
            write!(
                sheet,
                "<c r=\"{reference}\" t=\"inlineStr\"><is><t>R{row}C{column}</t></is></c>"
            )
            .unwrap();
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");

    zip_fixture(&[
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#,
        ),
        (
            "xl/workbook.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
</Relationships>"#,
        ),
        ("xl/worksheets/sheet1.xml", &sheet),
    ])
}

#[cfg(feature = "document-spreadsheets")]
fn spreadsheet_column_name(mut column: usize) -> String {
    let mut name = String::new();
    while column > 0 {
        column -= 1;
        name.insert(0, (b'A' + (column % 26) as u8) as char);
        column /= 26;
    }
    name
}

#[cfg(feature = "document-pdf")]
fn pdf_fixture(text_operation: &str) -> Vec<u8> {
    pdf_fixture_with_catalog_entry(text_operation, "")
}

#[cfg(feature = "document-pdf")]
fn pdf_fixture_with_catalog_entry(text_operation: &str, catalog_entry: &str) -> Vec<u8> {
    pdf_fixture_with_stream_dictionary(text_operation, catalog_entry, "")
}

#[cfg(feature = "document-pdf")]
fn pdf_fixture_with_filter(text_operation: &str, filters: &str) -> Vec<u8> {
    pdf_fixture_with_stream_dictionary(text_operation, "", &format!("/Filter [{filters}] "))
}

#[cfg(feature = "document-pdf")]
fn pdf_fixture_with_stream_dictionary(
    text_operation: &str,
    catalog_entry: &str,
    extra_dictionary: &str,
) -> Vec<u8> {
    let stream = format!("BT /F1 12 Tf 72 720 Td {text_operation} ET");
    let objects = [
        format!("<< /Type /Catalog /Pages 2 0 R {catalog_entry} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        format!(
            "<< {extra_dictionary}/Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        ),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        write!(pdf, "{} 0 obj\n{}\nendobj\n", index + 1, object).unwrap();
    }
    let xref_offset = pdf.len();
    write!(pdf, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).unwrap();
    for offset in offsets {
        writeln!(pdf, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        pdf,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
        objects.len() + 1
    )
    .unwrap();
    pdf
}

#[cfg(feature = "document-pdf")]
fn pdf_fixture_with_object_streams() -> Vec<u8> {
    fn be(value: u32, width: usize) -> Vec<u8> {
        value.to_be_bytes()[4 - width..].to_vec()
    }

    let inner_objects = [
        (2u32, b"<< /Type /Catalog /Pages 3 0 R >>".as_slice()),
        (3, b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>"),
        (
            4,
            b"<< /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R >>",
        ),
        (5, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
    ];

    let mut header = String::new();
    let mut data = Vec::new();
    for (object_number, body) in inner_objects {
        header.push_str(&format!("{object_number} {} ", data.len()));
        data.extend_from_slice(body);
        data.push(b' ');
    }
    let first = header.len();
    let mut object_stream_raw = header.into_bytes();
    object_stream_raw.extend_from_slice(&data);
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&object_stream_raw).unwrap();
    let object_stream_compressed = encoder.finish().unwrap();

    let content = b"BT /F1 12 Tf 72 720 Td (Hello ObjStm) Tj ET";
    let mut pdf = b"%PDF-1.5\n".to_vec();
    let mut offsets = std::collections::BTreeMap::new();

    offsets.insert(1u32, pdf.len());
    write!(
        pdf,
        "1 0 obj\n<< /Type /ObjStm /N {} /First {first} /Filter /FlateDecode /Length {} >>\nstream\n",
        inner_objects.len(),
        object_stream_compressed.len()
    )
    .unwrap();
    pdf.extend_from_slice(&object_stream_compressed);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");

    offsets.insert(6, pdf.len());
    write!(pdf, "6 0 obj\n<< /Length {} >>\nstream\n", content.len()).unwrap();
    pdf.extend_from_slice(content);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");

    let xref_offset = pdf.len();
    let mut xref_raw = Vec::new();
    xref_raw.extend(be(0, 1));
    xref_raw.extend(be(0, 4));
    xref_raw.extend(be(65535, 2));
    xref_raw.extend(be(1, 1));
    xref_raw.extend(be(offsets[&1] as u32, 4));
    xref_raw.extend(be(0, 2));
    for index in 0..4u32 {
        xref_raw.extend(be(2, 1));
        xref_raw.extend(be(1, 4));
        xref_raw.extend(be(index, 2));
    }
    xref_raw.extend(be(1, 1));
    xref_raw.extend(be(offsets[&6] as u32, 4));
    xref_raw.extend(be(0, 2));
    xref_raw.extend(be(1, 1));
    xref_raw.extend(be(xref_offset as u32, 4));
    xref_raw.extend(be(0, 2));

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&xref_raw).unwrap();
    let xref_compressed = encoder.finish().unwrap();

    write!(
        pdf,
        "7 0 obj\n<< /Type /XRef /Size 8 /W [1 4 2] /Root 2 0 R /Filter /FlateDecode /Length {} >>\nstream\n",
        xref_compressed.len()
    )
    .unwrap();
    pdf.extend_from_slice(&xref_compressed);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    write!(pdf, "startxref\n{xref_offset}\n%%EOF\n").unwrap();
    pdf
}
