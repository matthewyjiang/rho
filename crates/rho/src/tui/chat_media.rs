use rho_providers::model::{image_summary, ContentBlock, ImageContent};

/// Media queued with the next user turn.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ChatMedia {
    Image(ImageContent),
    TextDocument(ChatTextDocument),
}

/// Extracted document content kept as text rather than raw file bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ChatTextDocument {
    pub(super) name: String,
    pub(super) mime: String,
    pub(super) body: String,
    pub(super) truncated: bool,
    pub(super) warnings: Vec<String>,
}

impl ChatMedia {
    pub(super) fn composer_label(&self, index: usize) -> String {
        match self {
            Self::Image(image) => format!("[image {index}: {}]", image_summary(image)),
            Self::TextDocument(document) => document.label(),
        }
    }

    pub(super) fn display_block(&self) -> ContentBlock {
        match self {
            Self::Image(image) => ContentBlock::Image(image.clone()),
            Self::TextDocument(document) => ContentBlock::Text(document.label()),
        }
    }

    pub(super) fn model_block(self) -> ContentBlock {
        match self {
            Self::Image(image) => ContentBlock::Image(image),
            Self::TextDocument(document) => ContentBlock::Text(document.model_text()),
        }
    }
}

impl ChatTextDocument {
    fn label(&self) -> String {
        let kind = document_kind(&self.name, &self.mime);
        let suffix = if self.truncated { " · truncated" } else { "" };
        format!(
            "[{kind}: {} · {} chars{suffix}]",
            self.name,
            self.body.chars().count()
        )
    }

    fn model_text(self) -> String {
        let mut header = format!("Attached file: {} ({})", self.name, self.mime);
        if self.truncated {
            header.push_str("\nExtraction notice: content was truncated to the attachment limit.");
        }
        if !self.warnings.is_empty() {
            header.push_str("\nExtraction warnings: ");
            header.push_str(&self.warnings.join("; "));
        }
        format!("{header}\n<<<\n{}\n>>>", self.body)
    }
}

impl From<rho_tools::document::ExtractedDocument> for ChatTextDocument {
    fn from(document: rho_tools::document::ExtractedDocument) -> Self {
        Self {
            name: document.name,
            mime: document.mime,
            body: document.text,
            truncated: document.truncated,
            warnings: document.warnings,
        }
    }
}

fn document_kind(name: &str, mime: &str) -> String {
    std::path::Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| !extension.is_empty())
        .map(|extension| extension.to_ascii_lowercase())
        .or_else(|| mime.split('/').next_back().map(str::to_owned))
        .unwrap_or_else(|| "document".into())
}
