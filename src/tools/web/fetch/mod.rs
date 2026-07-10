pub(super) mod github;

use std::{fs, path::Path};

use futures_util::StreamExt;
use serde_json::{json, Value};
use url::Url;

use crate::tool::{resolve_path, truncate, ToolContext, ToolError};

use super::util::{extract_title, html_to_text, http_client, is_video_extension, is_youtube_url};

pub(super) const PREVIEW_BYTES: usize = 8_000;
const MAX_FETCH_BYTES: usize = 2 * 1024 * 1024;

pub(super) struct FetchedTarget {
    pub(super) title: Option<String>,
    pub(super) content: String,
    pub(super) preview: Value,
    pub(super) metadata: Value,
}

pub(super) async fn fetch_target(
    target: &str,
    ctx: &ToolContext,
    prompt: Option<&str>,
    timestamp: Option<&str>,
    frames: usize,
    force_clone: bool,
) -> Result<FetchedTarget, ToolError> {
    if let Some(github) = github::parse_url(target) {
        return github::fetch(&github, force_clone).await;
    }

    if is_youtube_url(target) {
        let content = format!(
            "YouTube video analysis requires optional video extraction dependencies. prompt: {}; timestamp: {}; frames: {frames}",
            prompt.unwrap_or("none"),
            timestamp.unwrap_or("none")
        );
        return Ok(FetchedTarget {
            title: Some("youtube video".into()),
            content: content.clone(),
            preview: json!({"type": "youtube_video", "warning": content}),
            metadata: json!({"mode": "video_placeholder", "timestamp": timestamp, "frames": frames}),
        });
    }

    if let Ok(url) = Url::parse(target) {
        if content_type_from_path(url.path()) == "pdf" {
            return Ok(remote_pdf_fallback(target));
        }
        let content = fetch_url_text(url.as_str()).await?;
        let title = extract_title(&content);
        let markdown = html_to_text(&content);
        return Ok(FetchedTarget {
            title: title.clone(),
            content: markdown.clone(),
            preview: json!({
                "type": content_type_from_path(url.path()),
                "url": target,
                "title": title,
                "preview": truncate(markdown.clone(), PREVIEW_BYTES)
            }),
            metadata: json!({"mode": "http_fetch", "prompt": prompt}),
        });
    }

    fetch_local_path(target, &ctx.cwd, prompt, timestamp, frames)
}

pub(super) async fn fetch_url_text(url: &str) -> Result<String, ToolError> {
    fetch_url_text_with_auth(url, None).await
}

async fn fetch_url_text_with_auth(
    url: &str,
    bearer_token: Option<&str>,
) -> Result<String, ToolError> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ToolError::Message(
            "only http and https URLs are supported".into(),
        ));
    }
    let mut request = http_client()
        .get(url)
        .header("User-Agent", "rho-coding-agent");
    if let Some(token) = bearer_token {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .map_err(|err| ToolError::Message(format!("request failed: {err}")))?
        .error_for_status()
        .map_err(|err| ToolError::Message(format!("request failed: {err}")))?;
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| ToolError::Message(format!("request failed: {err}")))?;
        let remaining = MAX_FETCH_BYTES.saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if bytes.len() >= MAX_FETCH_BYTES {
            break;
        }
    }
    String::from_utf8(bytes).map_err(ToolError::Utf8)
}

fn fetch_local_path(
    target: &str,
    cwd: &Path,
    prompt: Option<&str>,
    timestamp: Option<&str>,
    frames: usize,
) -> Result<FetchedTarget, ToolError> {
    let path = resolve_path(cwd, target);
    let metadata = fs::metadata(&path)?;
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
            content: content.clone(),
            preview: json!({"type": "local_video", "path": path, "warning": content}),
            metadata: json!({"mode": "video_placeholder", "bytes": metadata.len()}),
        });
    }
    if extension.eq_ignore_ascii_case("pdf") {
        let content = format!(
            "PDF detected at {} ({} bytes). PDF text extraction is not available in this local MVP.",
            path.display(),
            metadata.len()
        );
        return Ok(FetchedTarget {
            title: path
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
            content: content.clone(),
            preview: json!({"type": "pdf", "path": path, "warning": content}),
            metadata: json!({"mode": "pdf_placeholder", "bytes": metadata.len()}),
        });
    }

    let content = fs::read_to_string(&path)?;
    Ok(FetchedTarget {
        title: path
            .file_name()
            .map(|name| name.to_string_lossy().to_string()),
        content: content.clone(),
        preview: json!({
            "type": "local_file",
            "path": path,
            "preview": truncate(content, PREVIEW_BYTES)
        }),
        metadata: json!({"mode": "local_file", "bytes": metadata.len()}),
    })
}

fn content_type_from_path(path: &str) -> &'static str {
    if path.ends_with(".pdf") {
        "pdf"
    } else {
        "webpage"
    }
}

fn remote_pdf_fallback(url: &str) -> FetchedTarget {
    let content = format!(
        "Remote PDF detected at {url}. PDF text extraction is not available in this local MVP."
    );
    FetchedTarget {
        title: Some("remote pdf".into()),
        content: content.clone(),
        preview: json!({"type": "pdf", "url": url, "warning": content}),
        metadata: json!({"mode": "pdf_placeholder"}),
    }
}
