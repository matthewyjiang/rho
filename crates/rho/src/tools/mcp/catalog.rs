//! Session-scoped access to the prompts and resources of connected servers.
//!
//! Tools reach the model through the registry, but prompts and resources are
//! things a *person* picks: a prompt fills the composer, a resource is pulled
//! into a message. Both therefore need a handle the interactive host can hold
//! and query, separate from the tool registry.
//!
//! Listings are cached at connect and refreshed when a server announces a
//! change, so palette matching stays a local lookup. Fetching one prompt or one
//! resource is a round-trip, because that is what the protocol requires.

use std::sync::{Arc, RwLock};

use rmcp::{
    model::{GetPromptRequestParams, ReadResourceRequestParams},
    Peer, RoleClient,
};

use super::result::{self, RenderedResult};

/// One prompt a server offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpPrompt {
    pub(crate) server: String,
    pub(crate) name: String,
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) arguments: Vec<McpPromptArgument>,
}

impl McpPrompt {
    /// The slash-command name this prompt is offered under.
    pub(crate) fn command_name(&self) -> String {
        format!("mcp:{}:{}", self.server, self.name)
    }

    pub(crate) fn label(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }

    /// The usage line shown next to the command in the palette.
    pub(crate) fn usage(&self) -> String {
        let mut usage = format!("/{}", self.command_name());
        for argument in &self.arguments {
            if argument.required {
                usage.push_str(&format!(" <{}>", argument.name));
            } else {
                usage.push_str(&format!(" [{}=…]", argument.name));
            }
        }
        usage
    }

    /// Turn the text typed after the command into named prompt arguments.
    ///
    /// MCP prompt arguments are named, but typing `key=value` for a prompt that
    /// takes one argument is friction with no purpose. So a prompt with exactly
    /// one argument takes the whole trailing text as that argument's value, and
    /// anything else is read as whitespace-separated `key=value` pairs.
    pub(crate) fn parse_arguments(
        &self,
        trailing: &str,
    ) -> serde_json::Map<String, serde_json::Value> {
        let trailing = trailing.trim();
        let mut arguments = serde_json::Map::new();
        if trailing.is_empty() {
            return arguments;
        }
        if let [only] = self.arguments.as_slice() {
            arguments.insert(only.name.clone(), trailing.into());
            return arguments;
        }
        for pair in trailing.split_whitespace() {
            if let Some((name, value)) = pair.split_once('=') {
                arguments.insert(name.to_string(), value.into());
            }
        }
        arguments
    }

    /// Required arguments the caller did not supply, for a clear error.
    pub(crate) fn missing_arguments(
        &self,
        supplied: &serde_json::Map<String, serde_json::Value>,
    ) -> Vec<&str> {
        self.arguments
            .iter()
            .filter(|argument| argument.required && !supplied.contains_key(&argument.name))
            .map(|argument| argument.name.as_str())
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpPromptArgument {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) required: bool,
}

/// One resource a server offers, either a concrete URI or a URI template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpResource {
    pub(crate) server: String,
    /// A concrete `uri`, or a `uriTemplate` when `templated` is set.
    pub(crate) uri: String,
    pub(crate) name: String,
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) mime_type: Option<String>,
    /// The URI carries RFC 6570 placeholders the user must fill before a read.
    pub(crate) templated: bool,
}

impl McpResource {
    pub(crate) fn label(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }
}

/// What one server contributes to the catalog.
#[derive(Debug, Default)]
struct McpCatalogEntry {
    prompts: Vec<McpPrompt>,
    resources: Vec<McpResource>,
}

#[derive(Debug)]
struct McpCatalogServer {
    identity: String,
    peer: Peer<RoleClient>,
    entry: RwLock<McpCatalogEntry>,
}

/// Every connected server's prompts and resources, shared by the whole session.
#[derive(Clone, Debug, Default)]
pub(crate) struct McpCatalog {
    servers: Arc<RwLock<Vec<Arc<McpCatalogServer>>>>,
}

/// Why a catalog lookup or fetch could not be served.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum McpCatalogError {
    /// No connected server has that identity, so nothing can be fetched.
    UnknownServer(String),
    /// The server answered, but not in a way Rho can use.
    Server(String),
}

impl std::fmt::Display for McpCatalogError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownServer(identity) => {
                write!(formatter, "no connected MCP server named `{identity}`")
            }
            Self::Server(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for McpCatalogError {}

/// One prompt expanded into the text it contributes to a message.
#[derive(Debug, PartialEq)]
pub(crate) struct McpPromptExpansion {
    pub(crate) description: Option<String>,
    pub(crate) text: String,
}

impl McpCatalog {
    /// Register a connected server. Called once per server during connect.
    pub(super) fn register(&self, identity: String, peer: Peer<RoleClient>) -> McpCatalogHandle {
        let server = Arc::new(McpCatalogServer {
            identity,
            peer,
            entry: RwLock::new(McpCatalogEntry::default()),
        });
        self.write().push(Arc::clone(&server));
        McpCatalogHandle { server }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.read()
            .iter()
            .all(|server| server.entry().prompts.is_empty() && server.entry().resources.is_empty())
    }

    /// Every prompt, ordered by server then prompt name so palette results are
    /// stable between keystrokes.
    pub(crate) fn prompts(&self) -> Vec<McpPrompt> {
        self.collect(|entry| entry.prompts.clone())
    }

    pub(crate) fn resources(&self) -> Vec<McpResource> {
        self.collect(|entry| entry.resources.clone())
    }

    fn collect<T>(&self, select: impl Fn(&McpCatalogEntry) -> Vec<T>) -> Vec<T> {
        let mut servers = self.read().clone();
        servers.sort_by(|left, right| left.identity.cmp(&right.identity));
        servers
            .iter()
            .flat_map(|server| select(&server.entry()))
            .collect()
    }

    /// Fetch one prompt and render its messages into composer text.
    pub(crate) async fn get_prompt(
        &self,
        server: &str,
        name: &str,
        arguments: serde_json::Map<String, serde_json::Value>,
        max_output_bytes: usize,
    ) -> Result<McpPromptExpansion, McpCatalogError> {
        let peer = self.peer(server)?;
        let mut params = GetPromptRequestParams::new(name);
        if !arguments.is_empty() {
            params.arguments = Some(arguments);
        }
        let result = peer
            .get_prompt(params)
            .await
            .map_err(|error| McpCatalogError::Server(error.to_string()))?;
        Ok(McpPromptExpansion {
            description: result.description.clone(),
            text: result::render_prompt_messages(&result.messages, max_output_bytes),
        })
    }

    /// Read one resource and render its contents for a message.
    pub(crate) async fn read_resource(
        &self,
        server: &str,
        uri: &str,
        max_output_bytes: usize,
    ) -> Result<RenderedResult, McpCatalogError> {
        let peer = self.peer(server)?;
        let result = peer
            .read_resource(ReadResourceRequestParams::new(uri))
            .await
            .map_err(|error| McpCatalogError::Server(error.to_string()))?;
        Ok(result::render_resource_contents(
            &result.contents,
            max_output_bytes,
        ))
    }

    /// Suggested values for one prompt argument, from `completion/complete`.
    ///
    /// A server that does not support completion simply has no suggestions, so
    /// an error here is an empty list rather than a failure the user must read.
    pub(crate) async fn complete_prompt_argument(
        &self,
        server: &str,
        prompt: &str,
        argument: &str,
        typed: &str,
    ) -> Vec<String> {
        let Ok(peer) = self.peer(server) else {
            return Vec::new();
        };
        peer.complete_prompt_simple(prompt, argument, typed)
            .await
            .unwrap_or_default()
    }

    /// Suggested values for one placeholder in a resource URI template.
    pub(crate) async fn complete_resource_argument(
        &self,
        server: &str,
        uri_template: &str,
        argument: &str,
        typed: &str,
    ) -> Vec<String> {
        let Ok(peer) = self.peer(server) else {
            return Vec::new();
        };
        peer.complete_resource_simple(uri_template, argument, typed)
            .await
            .unwrap_or_default()
    }

    fn peer(&self, identity: &str) -> Result<Peer<RoleClient>, McpCatalogError> {
        self.read()
            .iter()
            .find(|server| server.identity == identity)
            .map(|server| server.peer.clone())
            .ok_or_else(|| McpCatalogError::UnknownServer(identity.to_string()))
    }

    /// A poisoned catalog lock still holds a valid server list, so recover
    /// rather than fail a palette lookup on an unrelated panic.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Vec<Arc<McpCatalogServer>>> {
        self.servers
            .read()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Vec<Arc<McpCatalogServer>>> {
        self.servers
            .write()
            .unwrap_or_else(|error| error.into_inner())
    }
}

impl McpCatalogServer {
    fn entry(&self) -> std::sync::RwLockReadGuard<'_, McpCatalogEntry> {
        self.entry.read().unwrap_or_else(|error| error.into_inner())
    }
}

/// Write access to one server's catalog entry, held by that server's session.
#[derive(Debug)]
pub(super) struct McpCatalogHandle {
    server: Arc<McpCatalogServer>,
}

impl McpCatalogHandle {
    pub(super) fn identity(&self) -> &str {
        &self.server.identity
    }

    pub(super) fn peer(&self) -> &Peer<RoleClient> {
        &self.server.peer
    }

    pub(super) fn set_prompts(&self, prompts: Vec<McpPrompt>) {
        self.write().prompts = prompts;
    }

    pub(super) fn set_resources(&self, resources: Vec<McpResource>) {
        self.write().resources = resources;
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, McpCatalogEntry> {
        self.server
            .entry
            .write()
            .unwrap_or_else(|error| error.into_inner())
    }
}

/// Convert one server's `prompts/list` page into catalog entries.
pub(super) fn prompts_from_remote(identity: &str, remote: Vec<rmcp::model::Prompt>) -> Vec<McpPrompt> {
    let mut prompts = remote
        .into_iter()
        .map(|prompt| McpPrompt {
            server: identity.to_string(),
            name: prompt.name,
            title: prompt.title,
            description: prompt.description,
            arguments: prompt
                .arguments
                .unwrap_or_default()
                .into_iter()
                .map(|argument| McpPromptArgument {
                    name: argument.name,
                    description: argument.description,
                    required: argument.required.unwrap_or(false),
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    prompts.sort_by(|left, right| left.name.cmp(&right.name));
    prompts
}

/// Convert `resources/list` and `resources/templates/list` into one ordered set.
pub(super) fn resources_from_remote(
    identity: &str,
    concrete: Vec<rmcp::model::Resource>,
    templates: Vec<rmcp::model::ResourceTemplate>,
) -> Vec<McpResource> {
    let mut resources = concrete
        .into_iter()
        .map(|resource| McpResource {
            server: identity.to_string(),
            uri: resource.uri,
            name: resource.name,
            title: resource.title,
            description: resource.description,
            mime_type: resource.mime_type,
            templated: false,
        })
        .chain(templates.into_iter().map(|template| McpResource {
            server: identity.to_string(),
            uri: template.uri_template,
            name: template.name,
            title: template.title,
            description: template.description,
            mime_type: template.mime_type,
            templated: true,
        }))
        .collect::<Vec<_>>();
    resources.sort_by(|left, right| left.name.cmp(&right.name).then(left.uri.cmp(&right.uri)));
    resources
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
