use std::path::Path;

use agent_client_protocol::schema::v1::{
    ContentBlock as AcpContentBlock, EmbeddedResourceResource,
};
use rho_protocol::ContentBlock;
use rho_sdk::UserInput;

use crate::app::prompt_content::{self, fence_context, InvalidPrompt};

/// Why an ACP session cwd cannot be used as the workspace root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionCwdError {
    NotAbsolute,
    NotDirectory,
}

impl std::fmt::Display for SessionCwdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAbsolute => formatter.write_str("ACP session cwd must be an absolute path"),
            Self::NotDirectory => {
                formatter.write_str("ACP session cwd must be an existing directory")
            }
        }
    }
}

impl std::error::Error for SessionCwdError {}

/// Accepts only an absolute path that already exists as a directory.
pub(super) fn validate_session_cwd(cwd: &Path) -> Result<&Path, SessionCwdError> {
    if !cwd.is_absolute() {
        return Err(SessionCwdError::NotAbsolute);
    }
    if !cwd.is_dir() {
        return Err(SessionCwdError::NotDirectory);
    }
    Ok(cwd)
}

/// Prefers the request cwd when the client sent one; otherwise the process cwd.
pub(super) fn workspace_cwd<'a>(request_cwd: &'a Path, process_cwd: &'a Path) -> &'a Path {
    if request_cwd.as_os_str().is_empty() {
        process_cwd
    } else {
        request_cwd
    }
}

/// Maps ACP prompt blocks onto SDK user input, through the wire blocks the
/// daemon's prompts carry, so both convert alike (#1670).
///
/// Blocks the wire has no kind for (audio, a binary resource, an ACP block
/// this build does not know) become fenced text here, so the model still
/// sees them.
pub(super) fn user_input_from_prompt(
    prompt: &[AcpContentBlock],
) -> Result<UserInput, InvalidPrompt> {
    let blocks: Vec<_> = prompt.iter().map(wire_block).collect();
    prompt_content::user_input(&blocks)
}

fn wire_block(block: &AcpContentBlock) -> ContentBlock {
    let fenced = |kind: &str, header: &str, body: &str| ContentBlock::Text {
        text: fence_context(kind, header, body),
    };
    match block {
        AcpContentBlock::Text(text) => ContentBlock::Text {
            text: text.text.clone(),
        },
        AcpContentBlock::Image(image) => ContentBlock::Image {
            mime_type: image.mime_type.clone(),
            data: image.data.clone(),
        },
        AcpContentBlock::Audio(audio) => fenced("audio", &audio.mime_type, &audio.data),
        AcpContentBlock::ResourceLink(link) => ContentBlock::ResourceLink {
            uri: link.uri.clone(),
            name: link.name.clone(),
            description: link.description.clone(),
        },
        AcpContentBlock::Resource(embedded) => match &embedded.resource {
            EmbeddedResourceResource::TextResourceContents(resource) => ContentBlock::Resource {
                uri: resource.uri.clone(),
                text: resource.text.clone(),
            },
            EmbeddedResourceResource::BlobResourceContents(resource) => {
                let header = match &resource.mime_type {
                    Some(mime_type) => format!("{} {mime_type}", resource.uri),
                    None => resource.uri.clone(),
                };
                fenced("resource", &header, &resource.blob)
            }
            _ => fenced("resource", "embedded", ""),
        },
        _ => fenced("context", "unsupported", ""),
    }
}
