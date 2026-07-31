//! Bounded text extraction for documents supported by Rho hosts and tools.
//!
//! [`extract_document_from_path`] and [`extract_document_from_bytes`] are the
//! stable entry points. Both enforce the same input and output limits. Office
//! and PDF support can be removed from minimal builds with this crate's
//! `document-docx`, `document-spreadsheets`, and `document-pdf` features.

use std::{
    fs::File,
    io::{Read, Take},
    path::{Path, PathBuf},
};

#[cfg(any(feature = "document-pdf", feature = "document-spreadsheets"))]
use std::fmt;

use thiserror::Error;

#[cfg(feature = "document-docx")]
mod docx;
#[cfg(feature = "document-pdf")]
mod pdf;
#[cfg(feature = "document-spreadsheets")]
mod spreadsheet;

/// Largest source document accepted by the extraction facade (25 MiB).
pub const MAX_DOCUMENT_INPUT_BYTES: usize = 25 * 1024 * 1024;
/// Largest extracted document body returned by the facade, measured in Unicode characters.
pub const MAX_EXTRACTED_CHARACTERS: usize = 200_000;
/// Largest number of extraction warnings returned with one document.
pub const MAX_DOCUMENT_WARNINGS: usize = 20;
/// Largest number of Unicode characters retained in one extraction warning.
pub const MAX_WARNING_CHARACTERS: usize = 500;
/// Largest number of Unicode characters retained in a document name.
pub const MAX_DOCUMENT_NAME_CHARACTERS: usize = 1_024;
/// Largest number of rows rendered from each spreadsheet worksheet.
pub const MAX_SPREADSHEET_ROWS: usize = 200;
/// Largest number of columns rendered from each spreadsheet worksheet.
pub const MAX_SPREADSHEET_COLUMNS: usize = 40;

/// Text and metadata extracted from one source document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractedDocument {
    pub name: String,
    pub mime: String,
    pub text: String,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

/// Failure to identify, read, or extract a document.
#[derive(Debug, Error)]
pub enum DocumentExtractionError {
    #[error("could not read document: {0}")]
    Io(#[from] std::io::Error),
    #[error("document '{name}' is {size} bytes; the input limit is {max} bytes")]
    InputTooLarge { name: String, size: u64, max: usize },
    #[error("unsupported document format for '{name}'")]
    UnsupportedFormat { name: String },
    #[error("{format} extraction support is disabled; enable the '{feature}' feature")]
    FeatureDisabled {
        format: &'static str,
        feature: &'static str,
    },
    #[error("could not extract {format} document '{name}': {message}")]
    Extraction {
        name: String,
        format: &'static str,
        message: String,
    },
    #[error("document extraction task failed: {message}")]
    Task { message: String },
}

#[derive(Debug)]
pub(super) struct ExtractedText {
    text: String,
    truncated: bool,
}

#[derive(Debug)]
pub(super) struct BoundedText {
    text: String,
    characters: usize,
    max_characters: usize,
    truncated: bool,
}

impl BoundedText {
    pub(super) fn new(max_characters: usize) -> Self {
        Self {
            text: String::new(),
            characters: 0,
            max_characters,
            truncated: false,
        }
    }

    pub(super) fn push_str(&mut self, value: &str) -> bool {
        if value.is_empty() {
            return true;
        }
        let remaining = self.max_characters.saturating_sub(self.characters);
        let Some((boundary, _)) = value.char_indices().nth(remaining) else {
            self.text.push_str(value);
            self.characters += value.chars().count();
            return true;
        };
        self.text.push_str(&value[..boundary]);
        self.characters = self.max_characters;
        self.truncated = true;
        false
    }

    #[cfg(any(feature = "document-docx", feature = "document-spreadsheets"))]
    pub(super) fn push(&mut self, character: char) -> bool {
        let mut encoded = [0_u8; 4];
        self.push_str(character.encode_utf8(&mut encoded))
    }

    #[cfg(feature = "document-docx")]
    pub(super) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    #[cfg(feature = "document-docx")]
    pub(super) fn ends_with_char(&self, suffix: char) -> bool {
        self.text.ends_with(suffix)
    }

    #[cfg(feature = "document-docx")]
    pub(super) fn remove_suffix(&mut self, suffix: &str) {
        while self.text.ends_with(suffix) {
            self.text.truncate(self.text.len() - suffix.len());
            self.characters -= suffix.chars().count();
        }
    }

    #[cfg(feature = "document-spreadsheets")]
    pub(super) fn trim_end(&mut self) {
        let trimmed_len = self.text.trim_end().len();
        if trimmed_len < self.text.len() {
            self.characters -= self.text[trimmed_len..].chars().count();
            self.text.truncate(trimmed_len);
        }
    }

    #[cfg(any(feature = "document-docx", feature = "document-pdf"))]
    pub(super) fn truncated(&self) -> bool {
        self.truncated
    }

    pub(super) fn into_extracted(self) -> ExtractedText {
        ExtractedText {
            text: self.text,
            truncated: self.truncated,
        }
    }
}

#[cfg(any(feature = "document-pdf", feature = "document-spreadsheets"))]
impl fmt::Write for BoundedText {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.push_str(value) {
            Ok(())
        } else {
            Err(fmt::Error)
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct BoundedWarnings {
    values: Vec<String>,
    omitted: bool,
}

impl BoundedWarnings {
    pub(super) fn push(&mut self, warning: String) {
        if self.values.len() == MAX_DOCUMENT_WARNINGS {
            self.omitted = true;
            return;
        }
        let (mut warning, truncated) = truncate_characters(warning, MAX_WARNING_CHARACTERS);
        if truncated {
            warning.push_str("...");
        }
        self.values.push(warning);
    }

    fn finish(mut self, reserve: usize) -> Vec<String> {
        let limit = MAX_DOCUMENT_WARNINGS.saturating_sub(reserve);
        if self.values.len() > limit {
            self.values.truncate(limit);
            self.omitted = true;
        }
        if self.omitted && limit > 0 {
            let omitted = "additional extraction warnings were omitted".to_owned();
            if self.values.len() == limit {
                self.values[limit - 1] = omitted;
            } else {
                self.values.push(omitted);
            }
        }
        self.values
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DocumentFormat {
    Text(&'static str),
    Pdf,
    Xlsx,
    Xls,
    Ods,
    Docx,
}

impl DocumentFormat {
    fn mime(self) -> &'static str {
        match self {
            Self::Text(mime) => mime,
            Self::Pdf => "application/pdf",
            Self::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Self::Xls => "application/vnd.ms-excel",
            Self::Ods => "application/vnd.oasis.opendocument.spreadsheet",
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        }
    }
}

/// Extracts a document from a filesystem path with hard byte and character caps.
pub fn extract_document_from_path(
    path: impl AsRef<Path>,
) -> Result<ExtractedDocument, DocumentExtractionError> {
    let path = path.as_ref();
    let name = sanitize_document_name(
        &path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned()),
    );
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    if size > MAX_DOCUMENT_INPUT_BYTES as u64 {
        return Err(DocumentExtractionError::InputTooLarge {
            name,
            size,
            max: MAX_DOCUMENT_INPUT_BYTES,
        });
    }
    let bytes = read_bounded(file, &name)?;
    extract_document_from_bytes(&name, &bytes)
}

/// Extracts a document path on Tokio's blocking pool.
pub async fn extract_document_from_path_async(
    path: impl Into<PathBuf>,
) -> Result<ExtractedDocument, DocumentExtractionError> {
    let path = path.into();
    tokio::task::spawn_blocking(move || extract_document_from_path(path))
        .await
        .map_err(|error| DocumentExtractionError::Task {
            message: error.to_string(),
        })?
}

/// Extracts owned document bytes on Tokio's blocking pool.
pub async fn extract_document_from_bytes_async(
    name: String,
    bytes: Vec<u8>,
) -> Result<ExtractedDocument, DocumentExtractionError> {
    tokio::task::spawn_blocking(move || extract_document_from_bytes(&name, &bytes))
        .await
        .map_err(|error| DocumentExtractionError::Task {
            message: error.to_string(),
        })?
}

/// Extracts a named in-memory document with hard byte and character caps.
///
/// `name` should include an extension when one is known. PDF signatures are
/// recognized independently of the extension, and valid UTF-8 with no known
/// extension is treated as plain text.
pub fn extract_document_from_bytes(
    name: &str,
    bytes: &[u8],
) -> Result<ExtractedDocument, DocumentExtractionError> {
    let safe_name = sanitize_document_name(name);
    if bytes.len() > MAX_DOCUMENT_INPUT_BYTES {
        return Err(DocumentExtractionError::InputTooLarge {
            name: safe_name,
            size: bytes.len() as u64,
            max: MAX_DOCUMENT_INPUT_BYTES,
        });
    }

    let format =
        detect_format(name, bytes).map_err(|_| DocumentExtractionError::UnsupportedFormat {
            name: safe_name.clone(),
        })?;
    let mut warnings = BoundedWarnings::default();
    let extracted = match format {
        DocumentFormat::Text(_) => extract_text(&safe_name, bytes)?,
        DocumentFormat::Pdf => extract_pdf(&safe_name, bytes)?,
        DocumentFormat::Xlsx | DocumentFormat::Xls | DocumentFormat::Ods => {
            extract_spreadsheet(&safe_name, bytes, &mut warnings)?
        }
        DocumentFormat::Docx => extract_docx(&safe_name, bytes, &mut warnings)?,
    };

    if extracted.text.trim().is_empty() {
        warnings.push(match format {
            DocumentFormat::Pdf => {
                "PDF contains no extractable text; it may be empty or contain only scanned images"
                    .to_owned()
            }
            _ => "document contains no extractable text".to_owned(),
        });
    }

    let mut warnings = warnings.finish(usize::from(extracted.truncated));
    if extracted.truncated {
        warnings.push(format!(
            "extracted text was truncated at {MAX_EXTRACTED_CHARACTERS} characters"
        ));
    }
    Ok(ExtractedDocument {
        name: safe_name,
        mime: format.mime().to_owned(),
        text: extracted.text,
        truncated: extracted.truncated,
        warnings,
    })
}

fn sanitize_document_name(name: &str) -> String {
    let name = name
        .chars()
        .map(|character| {
            if character.is_control() {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    let (mut name, truncated) = truncate_characters(name, MAX_DOCUMENT_NAME_CHARACTERS);
    if truncated {
        name.push_str("...");
    }
    if name.is_empty() {
        "document".into()
    } else {
        name
    }
}

fn read_bounded(file: File, name: &str) -> Result<Vec<u8>, DocumentExtractionError> {
    let mut bytes = Vec::new();
    let mut reader: Take<File> = file.take(MAX_DOCUMENT_INPUT_BYTES as u64 + 1);
    reader.read_to_end(&mut bytes)?;
    if bytes.len() > MAX_DOCUMENT_INPUT_BYTES {
        return Err(DocumentExtractionError::InputTooLarge {
            name: name.to_owned(),
            size: bytes.len() as u64,
            max: MAX_DOCUMENT_INPUT_BYTES,
        });
    }
    Ok(bytes)
}

fn detect_format(name: &str, bytes: &[u8]) -> Result<DocumentFormat, DocumentExtractionError> {
    if bytes.starts_with(b"%PDF-") {
        return Ok(DocumentFormat::Pdf);
    }

    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let format = match extension.as_deref() {
        Some("pdf") => Some(DocumentFormat::Pdf),
        Some("xlsx") => Some(DocumentFormat::Xlsx),
        Some("xls") => Some(DocumentFormat::Xls),
        Some("ods") => Some(DocumentFormat::Ods),
        Some("docx") => Some(DocumentFormat::Docx),
        Some(extension) => text_mime(extension).map(DocumentFormat::Text),
        None => None,
    };
    if let Some(format) = format {
        return Ok(format);
    }
    if std::str::from_utf8(bytes).is_ok() && !looks_binary(bytes) {
        return Ok(DocumentFormat::Text("text/plain"));
    }
    Err(DocumentExtractionError::UnsupportedFormat {
        name: name.to_owned(),
    })
}

fn text_mime(extension: &str) -> Option<&'static str> {
    match extension {
        "txt" | "text" | "log" => Some("text/plain"),
        "md" | "markdown" | "mdx" => Some("text/markdown"),
        "csv" | "tsv" => Some("text/csv"),
        "json" | "jsonl" => Some("application/json"),
        "xml" => Some("application/xml"),
        "html" | "htm" => Some("text/html"),
        "yaml" | "yml" => Some("application/yaml"),
        "toml" => Some("application/toml"),
        "rs" | "c" | "cc" | "cpp" | "h" | "hpp" | "go" | "java" | "js" | "jsx" | "ts" | "tsx"
        | "py" | "rb" | "php" | "swift" | "kt" | "kts" | "scala" | "sh" | "bash" | "zsh"
        | "fish" | "ps1" | "sql" | "css" | "scss" | "less" | "vue" | "svelte" | "graphql"
        | "gql" => Some("text/plain"),
        _ => None,
    }
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

fn extract_text(name: &str, bytes: &[u8]) -> Result<ExtractedText, DocumentExtractionError> {
    let text = std::str::from_utf8(bytes).map_err(|error| DocumentExtractionError::Extraction {
        name: name.to_owned(),
        format: "UTF-8 text",
        message: error.to_string(),
    })?;
    let mut output = BoundedText::new(MAX_EXTRACTED_CHARACTERS);
    output.push_str(text.strip_prefix('\u{feff}').unwrap_or(text));
    Ok(output.into_extracted())
}

#[cfg(feature = "document-pdf")]
fn extract_pdf(name: &str, bytes: &[u8]) -> Result<ExtractedText, DocumentExtractionError> {
    pdf::extract(bytes, MAX_EXTRACTED_CHARACTERS).map_err(|message| {
        DocumentExtractionError::Extraction {
            name: name.to_owned(),
            format: "PDF",
            message,
        }
    })
}

#[cfg(not(feature = "document-pdf"))]
fn extract_pdf(_name: &str, _bytes: &[u8]) -> Result<ExtractedText, DocumentExtractionError> {
    Err(DocumentExtractionError::FeatureDisabled {
        format: "PDF",
        feature: "document-pdf",
    })
}

#[cfg(feature = "document-spreadsheets")]
fn extract_spreadsheet(
    name: &str,
    bytes: &[u8],
    warnings: &mut BoundedWarnings,
) -> Result<ExtractedText, DocumentExtractionError> {
    spreadsheet::extract(bytes, warnings, MAX_EXTRACTED_CHARACTERS).map_err(|message| {
        DocumentExtractionError::Extraction {
            name: name.to_owned(),
            format: "spreadsheet",
            message,
        }
    })
}

#[cfg(not(feature = "document-spreadsheets"))]
fn extract_spreadsheet(
    _name: &str,
    _bytes: &[u8],
    _warnings: &mut BoundedWarnings,
) -> Result<ExtractedText, DocumentExtractionError> {
    Err(DocumentExtractionError::FeatureDisabled {
        format: "spreadsheet",
        feature: "document-spreadsheets",
    })
}

#[cfg(feature = "document-docx")]
fn extract_docx(
    name: &str,
    bytes: &[u8],
    warnings: &mut BoundedWarnings,
) -> Result<ExtractedText, DocumentExtractionError> {
    docx::extract(bytes, warnings, MAX_EXTRACTED_CHARACTERS).map_err(|message| {
        DocumentExtractionError::Extraction {
            name: name.to_owned(),
            format: "DOCX",
            message,
        }
    })
}

#[cfg(not(feature = "document-docx"))]
fn extract_docx(
    _name: &str,
    _bytes: &[u8],
    _warnings: &mut BoundedWarnings,
) -> Result<ExtractedText, DocumentExtractionError> {
    Err(DocumentExtractionError::FeatureDisabled {
        format: "DOCX",
        feature: "document-docx",
    })
}

fn truncate_characters(mut text: String, max_characters: usize) -> (String, bool) {
    let Some((boundary, _)) = text.char_indices().nth(max_characters) else {
        return (text, false);
    };
    text.truncate(boundary);
    (text, true)
}

#[cfg(test)]
#[path = "document_tests.rs"]
mod tests;
