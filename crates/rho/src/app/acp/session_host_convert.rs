use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{
    ContentBlock as AcpContentBlock, EmbeddedResourceResource,
};
use rho_sdk::{
    model::{ContentBlock, ImageContent},
    UserInput,
};

use crate::added_dirs::AddedDirs;

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

/// Maps ACP prompt blocks onto SDK user input.
///
/// Text and images stay typed. Resource, audio, and other blocks become fenced
/// text so the model still sees them.
pub(super) fn user_input_from_prompt(prompt: &[AcpContentBlock]) -> anyhow::Result<UserInput> {
    let blocks = prompt.iter().map(content_block_from_acp).collect();
    Ok(UserInput::content(blocks)?)
}

fn content_block_from_acp(block: &AcpContentBlock) -> ContentBlock {
    match block {
        AcpContentBlock::Text(text) => ContentBlock::Text(text.text.clone()),
        AcpContentBlock::Image(image) => ContentBlock::Image(ImageContent {
            data: image.data.clone(),
            mime_type: image.mime_type.clone(),
        }),
        AcpContentBlock::Audio(audio) => {
            ContentBlock::Text(fence_context("audio", &audio.mime_type, &audio.data))
        }
        AcpContentBlock::ResourceLink(link) => {
            let body = link.description.as_deref().unwrap_or(link.name.as_str());
            ContentBlock::Text(fence_context("resource", &link.uri, body))
        }
        AcpContentBlock::Resource(embedded) => match &embedded.resource {
            EmbeddedResourceResource::TextResourceContents(resource) => {
                ContentBlock::Text(fence_context("resource", &resource.uri, &resource.text))
            }
            EmbeddedResourceResource::BlobResourceContents(resource) => {
                let header = match &resource.mime_type {
                    Some(mime_type) => format!("{} {mime_type}", resource.uri),
                    None => resource.uri.clone(),
                };
                ContentBlock::Text(fence_context("resource", &header, &resource.blob))
            }
            _ => ContentBlock::Text(fence_context("resource", "embedded", "")),
        },
        _ => ContentBlock::Text(fence_context("context", "unsupported", "")),
    }
}

/// Wraps a body in a code fence long enough to survive backticks inside it.
/// A body that already contains ``` would end the fence early, so the fence
/// grows past the body's longest backtick run.
fn fence_context(kind: &str, header: &str, body: &str) -> String {
    let fence = "`".repeat(
        longest_backtick_run(header)
            .max(longest_backtick_run(body))
            .max(2)
            + 1,
    );
    format!("{fence}{kind} {header}\n{body}\n{fence}")
}

fn longest_backtick_run(body: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for character in body.chars() {
        run = if character == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}
