use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{
    ContentBlock as AcpContentBlock, EmbeddedResourceResource,
};
use rho_protocol::ContentBlock;
use rho_sdk::UserInput;

use crate::{
    added_dirs::AddedDirs,
    app::prompt_content::{self, fence_context, InvalidPrompt},
};

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

/// An ACP `additionalDirectories` entry the session cannot grant. ACP forbids
/// starting a session with fewer roots than the host asked for, so one bad
/// entry fails the whole `session/new` or `session/load` as invalid params.
#[derive(Debug)]
pub(in crate::app::acp) struct RejectedDirectory {
    entry: PathBuf,
    reason: String,
}

impl std::fmt::Display for RejectedDirectory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "additionalDirectories entry '{}': {}",
            self.entry.display(),
            self.reason
        )
    }
}

impl std::error::Error for RejectedDirectory {}

/// The added directories of a session opened by `session/new` or
/// `session/load`: the launch `--add-dir` set plus the host's
/// `additionalDirectories`. The request alone names the host's roots, so a
/// loaded session does not get back roots it saved earlier. Entries must be
/// absolute, then resolve like `--add-dir`: an existing UTF-8 directory,
/// canonicalized. `root` is the canonical session cwd; entries it or another
/// entry already covers are folded in.
pub(super) fn session_added_dirs(
    launch: &AddedDirs,
    requested: &[PathBuf],
    root: &Path,
) -> Result<AddedDirs, RejectedDirectory> {
    let mut host = AddedDirs::default();
    for entry in requested {
        let reject = |reason: String| RejectedDirectory {
            entry: entry.clone(),
            reason,
        };
        if entry.as_os_str().is_empty() {
            return Err(reject("path is empty".into()));
        }
        if !entry.is_absolute() {
            return Err(reject("path must be absolute".into()));
        }
        let dir = crate::added_dirs::resolve(entry, root, /*home*/ None)
            .map_err(|error| reject(error.to_string()))?;
        host.insert(root, dir);
    }
    Ok(launch.union(root, &host))
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
