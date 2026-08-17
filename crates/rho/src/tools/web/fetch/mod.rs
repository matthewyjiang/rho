pub(super) mod github;

use std::path::Path;

use futures_util::StreamExt;
use serde_json::{json, Value};
use url::Url;

use rho_tools::{
    document::{
        extract_document_from_bytes_async, extract_document_from_path_async,
        render_extracted_document, ExtractedDocument,
    },
    tool::ToolError,
};

use super::util::{extract_title, html_to_text, is_video_extension};

const MAX_FETCH_BYTES: usize = 2 * 1024 * 1024;

pub(super) struct DownloadedResponse {
    pub(super) bytes: Vec<u8>,
    pub(super) truncated: bool,
    pub(super) pdf_detection: PdfDetection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PdfDetection {
    NotDetected,
    Declared,
    MagicBytes,
}

enum DocumentSource {
    Http { prompt: Option<String> },
    Local { bytes: u64 },
}

pub(super) struct FetchedTarget {
    pub(super) title: Option<String>,
    pub(super) content: String,
    pub(super) metadata: Value,
}

pub(super) async fn fetch_http_url(
    url: &Url,
    prompt: Option<&str>,
) -> Result<FetchedTarget, ToolError> {
    let downloaded =
        fetch_url_bytes(url.as_str(), rho_tools::document::MAX_DOCUMENT_INPUT_BYTES).await?;
    if downloaded.pdf_detection != PdfDetection::NotDetected {
        if downloaded.truncated {
            return Err(ToolError::Message(format!(
                "remote PDF exceeds the {} byte extraction limit",
                rho_tools::document::MAX_DOCUMENT_INPUT_BYTES
            )));
        }
        let name = url
            .path_segments()
            .and_then(Iterator::last)
            .filter(|name| !name.is_empty())
            .unwrap_or("remote.pdf");
        let document = extract_document_from_bytes_async(name.to_owned(), downloaded.bytes)
            .await
            .map_err(|error| ToolError::Message(error.to_string()))?;
        return Ok(fetched_document(
            document,
            DocumentSource::Http {
                prompt: prompt.map(str::to_owned),
            },
        ));
    }
    let content = decode_downloaded_text(downloaded)?;
    let title = extract_title(&content);
    let markdown = html_to_text(&content);
    Ok(FetchedTarget {
        title,
        content: markdown,
        metadata: json!({"mode": "http_fetch", "prompt": prompt}),
    })
}

/// Content-fetch choke point: SSRF allow-ranges are resolved here, not in tool
/// plan types. The client is built here too, so no caller can reach an
/// arbitrary URL through a client that resolves the hostname itself.
pub(super) async fn fetch_url_text(url: &str) -> Result<String, ToolError> {
    let downloaded = fetch_url_bytes(url, MAX_FETCH_BYTES).await?;
    decode_downloaded_text(downloaded)
}

pub(super) async fn fetch_url_bytes(
    url: &str,
    max_bytes: usize,
) -> Result<DownloadedResponse, ToolError> {
    // Resolve and reject private/loopback targets, then connect only to the
    // vetted addresses so a changed DNS answer cannot move the request after
    // the check. Redirects are disabled on the client, so this is the full
    // guard; 3xx responses are refused below as defense in depth.
    let allow_ranges = super::ssrf::configured_allow_ranges()?;
    let target = super::ssrf::resolve_public_target(url, &allow_ranges).await?;
    let client = super::util::pinned_http_client(&target)?;
    let response = client
        .get(url)
        .header("User-Agent", "rho-coding-agent")
        .send()
        .await
        .map_err(|err| ToolError::Message(format!("request failed: {err}")))?;
    if response.status().is_redirection() {
        return Err(ToolError::Message(format!(
            "refusing to follow redirect from {url}"
        )));
    }
    let response = response
        .error_for_status()
        .map_err(|err| ToolError::Message(format!("request failed: {err}")))?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut pdf_detection = if Url::parse(url)
        .ok()
        .as_ref()
        .is_some_and(|url| is_pdf_response(url, content_type.as_deref()))
    {
        PdfDetection::Declared
    } else {
        PdfDetection::NotDetected
    };
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| ToolError::Message(format!("request failed: {err}")))?;
        if pdf_detection == PdfDetection::NotDetected
            && bytes.len() < b"%PDF-".len()
            && bytes
                .iter()
                .chain(chunk.iter())
                .take(b"%PDF-".len())
                .copied()
                .eq(b"%PDF-".iter().copied())
        {
            pdf_detection = PdfDetection::MagicBytes;
        }
        let remaining = (max_bytes + 1).saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if bytes.len() > max_bytes {
            break;
        }
    }
    let truncated = bytes.len() > max_bytes;
    bytes.truncate(max_bytes);
    Ok(DownloadedResponse {
        bytes,
        truncated,
        pdf_detection,
    })
}

fn decode_downloaded_text(downloaded: DownloadedResponse) -> Result<String, ToolError> {
    String::from_utf8(downloaded.bytes).or_else(|error| {
        if downloaded.truncated && error.utf8_error().error_len().is_none() {
            let valid_len = error.utf8_error().valid_up_to();
            let mut bytes = error.into_bytes();
            bytes.truncate(valid_len);
            return Ok(String::from_utf8(bytes).expect("bytes are valid up to valid_len"));
        }
        Err(ToolError::Utf8(error))
    })
}

fn is_pdf_response(url: &Url, content_type: Option<&str>) -> bool {
    url.path().to_ascii_lowercase().ends_with(".pdf")
        || content_type.is_some_and(|content_type| {
            content_type
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/pdf"))
        })
}

pub(super) async fn fetch_local_path(
    path: &Path,
    prompt: Option<&str>,
    timestamp: Option<&str>,
    frames: usize,
) -> Result<FetchedTarget, ToolError> {
    let metadata = tokio::fs::metadata(path).await?;
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    if is_video_extension(extension) {
        let content = format!(
            "Local video detected at {}. Visual analysis requires optional video extraction dependencies. prompt: {}; timestamp: {}; frames: {frames}",
            path.display(),
            prompt.unwrap_or("none"),
            timestamp.unwrap_or("none")
        );
        return Ok(FetchedTarget {
            title: path
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
            content,
            metadata: json!({"mode": "video_placeholder", "bytes": metadata.len()}),
        });
    }
    match extract_document_from_path_async(path.to_path_buf()).await {
        Ok(document) => Ok(fetched_document(
            document,
            DocumentSource::Local {
                bytes: metadata.len(),
            },
        )),
        Err(error) => Err(ToolError::Message(error.to_string())),
    }
}

fn fetched_document(document: ExtractedDocument, source: DocumentSource) -> FetchedTarget {
    let content = render_extracted_document(&document);
    let metadata = match source {
        DocumentSource::Http { prompt } => json!({
            "mode": "document_extract",
            "source": "http",
            "prompt": prompt,
            "mime": document.mime,
            "truncated": document.truncated,
            "warnings": document.warnings,
        }),
        DocumentSource::Local { bytes } => json!({
            "mode": "document_extract",
            "source": "local",
            "bytes": bytes,
            "mime": document.mime,
            "truncated": document.truncated,
            "warnings": document.warnings,
        }),
    };
    FetchedTarget {
        title: Some(document.name),
        content,
        metadata,
    }
}

pub(super) fn youtube_placeholder(
    _target: &str,
    prompt: Option<&str>,
    timestamp: Option<&str>,
    frames: usize,
) -> FetchedTarget {
    let content = format!(
        "YouTube video analysis requires optional video extraction dependencies. prompt: {}; timestamp: {}; frames: {frames}",
        prompt.unwrap_or("none"),
        timestamp.unwrap_or("none")
    );
    FetchedTarget {
        title: Some("youtube video".into()),
        content,
        metadata: json!({"mode": "video_placeholder", "timestamp": timestamp, "frames": frames}),
    }
}
