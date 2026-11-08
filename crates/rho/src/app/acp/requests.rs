//! ACP request fields as the daemon protocol carries them: prompt content,
//! host MCP servers, a session's setup, and the session list.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use agent_client_protocol::{
    schema::v1::{
        ContentBlock as AcpContentBlock, EmbeddedResourceResource, ListSessionsResponse,
        McpServer as AcpMcpServer, SessionId, SessionInfo,
    },
    Error as AcpError,
};
use rho_protocol::{
    ContentBlock, McpServer, SecretMap, SessionListResponse, SessionPart, SessionSetup,
    SessionSummary,
};

use crate::app::prompt_content::fence_context;

/// What `rho acp`'s own flags put in every session it opens or reopens:
/// `--agent`, `--add-dir`, and the `--no-*` flags.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Launch {
    pub(super) agent: Option<String>,
    /// `--add-dir` directories, resolved against the process cwd.
    pub(super) added_dirs: Vec<PathBuf>,
    /// What the `--no-*` flags leave out.
    pub(super) without: BTreeSet<SessionPart>,
}

impl Launch {
    /// The setup of a session the host opens with `directories` and
    /// `servers`. A host that draws no forms gets no questionnaire, since
    /// it could not answer one.
    pub(super) fn setup(
        &self,
        directories: &[PathBuf],
        servers: &[AcpMcpServer],
        forms: bool,
    ) -> SessionSetup {
        let mut additional_directories = self.added_dirs.clone();
        for directory in directories {
            if !additional_directories.contains(directory) {
                additional_directories.push(directory.clone());
            }
        }
        let mut without = self.without.clone();
        if !forms {
            without.insert(SessionPart::Questions);
        }
        SessionSetup {
            additional_directories,
            mcp_servers: protocol_servers(servers),
            without,
        }
    }
}

/// Accepts only an absolute path that already exists as a directory, as a
/// new session's workspace.
pub(super) fn session_cwd(cwd: &Path) -> Result<PathBuf, AcpError> {
    if !cwd.is_absolute() {
        return Err(AcpError::invalid_params().data("ACP session cwd must be an absolute path"));
    }
    if !cwd.is_dir() {
        return Err(
            AcpError::invalid_params().data("ACP session cwd must be an existing directory")
        );
    }
    Ok(cwd.to_path_buf())
}

/// ACP prompt blocks as the daemon's prompt content. Blocks the wire has no
/// kind for (audio, a binary resource, an ACP block this build does not
/// know) become fenced text, so the model still sees them.
pub(super) fn prompt_content(prompt: &[AcpContentBlock]) -> Vec<ContentBlock> {
    prompt.iter().map(wire_block).collect()
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

/// The servers ACP `servers` name, in order, but those whose transport Rho
/// cannot speak, which are logged and skipped.
fn protocol_servers(servers: &[AcpMcpServer]) -> Vec<McpServer> {
    servers.iter().filter_map(protocol_server).collect()
}

fn protocol_server(server: &AcpMcpServer) -> Option<McpServer> {
    match server {
        AcpMcpServer::Stdio(stdio) => Some(McpServer::Stdio {
            name: stdio.name.clone(),
            command: stdio.command.clone(),
            args: stdio.args.clone(),
            env: by_name(
                &stdio.name,
                stdio
                    .env
                    .iter()
                    .map(|variable| (variable.name.as_str(), variable.value.as_str())),
            ),
        }),
        AcpMcpServer::Http(http) => Some(McpServer::Http {
            name: http.name.clone(),
            url: http.url.clone(),
            headers: by_name(
                &http.name,
                http.headers
                    .iter()
                    .map(|header| (header.name.as_str(), header.value.as_str())),
            ),
        }),
        AcpMcpServer::Sse(sse) => {
            tracing::warn!(
                server = %sse.name,
                "skipping host MCP server: SSE transport is not supported"
            );
            None
        }
        // McpServer is non_exhaustive; a transport Rho does not know cannot connect.
        _ => {
            tracing::warn!("skipping host MCP server with an unsupported transport");
            None
        }
    }
}

/// Server `server`'s environment variables or headers, an ACP list, by
/// name. When a name repeats, the last value wins.
fn by_name<'a>(server: &str, listed: impl Iterator<Item = (&'a str, &'a str)>) -> SecretMap {
    let mut values = BTreeMap::new();
    for (name, value) in listed {
        if values.insert(name.to_owned(), value.to_owned()).is_some() {
            tracing::warn!(
                server,
                entry = name,
                "host MCP server lists a name twice; keeping the last value"
            );
        }
    }
    values.into_iter().collect()
}

/// The daemon's session list page as ACP lists it.
pub(super) fn session_list(page: SessionListResponse) -> ListSessionsResponse {
    ListSessionsResponse::new(page.sessions.into_iter().map(session_info).collect())
        .next_cursor(page.next_cursor)
}

/// `summary` labelled with its title, else its first user message, with its
/// last save as an RFC 3339 time. Its directories are those still there,
/// which `session/load` takes only from its own request.
fn session_info(summary: SessionSummary) -> SessionInfo {
    let title = summary.title.or(summary.first_user_message);
    let updated_at = i64::try_from(summary.updated_at)
        .ok()
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, /*use_z*/ true));
    SessionInfo::new(SessionId::new(summary.session.as_str()), summary.cwd)
        .additional_directories(summary.added_dirs)
        .title(title)
        .updated_at(updated_at)
}

#[cfg(test)]
#[path = "requests_tests.rs"]
mod tests;
