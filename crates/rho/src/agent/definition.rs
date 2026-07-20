use std::{collections::BTreeSet, fmt, str::FromStr};

use sha2::{Digest, Sha256};
use thiserror::Error;

use rho_providers::reasoning::ReasoningLevel;

macro_rules! define_tool_capabilities {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        /// A parsed tool capability in an agent definition.
        ///
        /// Built-ins have stable variants so policy code does not parse names again.
        /// Extension names are reserved for capabilities supplied by the host.
        #[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
        pub enum ToolCapability {
            $($variant,)+
            Extension(String),
        }

        /// Every built-in tool capability understood by the Rho host.
        pub const BUILTIN_TOOL_CAPABILITIES: &[ToolCapability] = &[
            $(ToolCapability::$variant,)+
        ];

        impl ToolCapability {
            pub fn parse(name: String) -> Self {
                match name.as_str() {
                    $($name => Self::$variant,)+
                    _ => Self::Extension(name),
                }
            }

            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $name,)+
                    Self::Extension(name) => name,
                }
            }
        }
    };
}

define_tool_capabilities! {
    Agent => "agent",
    Agents => "agents",
    Bash => "bash",
    EditFile => "edit_file",
    FetchContent => "fetch_content",
    GetSearchContent => "get_search_content",
    ListDir => "list_dir",
    Powershell => "powershell",
    Process => "process",
    Questionnaire => "questionnaire",
    ReadFile => "read_file",
    Rho => "rho",
    Shell => "shell",
    Skill => "skill",
    WebSearch => "web_search",
    WriteFile => "write_file",
}

impl fmt::Display for ToolCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub type ToolCapabilitySet = BTreeSet<ToolCapability>;

/// Tool capabilities resolved against the current host and invocation role.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentCapabilities {
    tools: ToolCapabilitySet,
}

impl AgentCapabilities {
    pub fn new(tools: ToolCapabilitySet) -> Self {
        Self { tools }
    }

    pub fn all_host_tools() -> Self {
        Self::new(BUILTIN_TOOL_CAPABILITIES.iter().cloned().collect())
    }

    pub fn contains(&self, capability: &ToolCapability) -> bool {
        self.tools.contains(capability)
    }

    pub fn insert(&mut self, capability: ToolCapability) {
        self.tools.insert(capability);
    }

    pub fn remove(&mut self, capability: &ToolCapability) {
        self.tools.remove(capability);
    }
}

impl From<ToolCapabilitySet> for AgentCapabilities {
    fn from(tools: ToolCapabilitySet) -> Self {
        Self::new(tools)
    }
}

/// Stable identifier used to select an agent across invocations.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct AgentId(String);

impl AgentId {
    pub fn new(value: impl Into<String>) -> Result<Self, AgentIdError> {
        let value = value.into();
        validate_agent_id(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for AgentId {
    type Err = AgentIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("invalid agent ID '{value}': {reason}")]
pub struct AgentIdError {
    value: String,
    reason: &'static str,
}

fn validate_agent_id(value: &str) -> Result<(), AgentIdError> {
    let invalid = |reason| AgentIdError {
        value: value.to_string(),
        reason,
    };
    if value.is_empty() || value.len() > 64 {
        return Err(invalid("must contain 1-64 characters"));
    }
    if value.starts_with('-') || value.ends_with('-') || value.contains("--") {
        return Err(invalid("must use single hyphens only between segments"));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(invalid(
            "must contain only lowercase ASCII letters, digits, and hyphens",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptPolicy {
    Extend(String),
    Replace(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSelection {
    pub provider: Option<String>,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelPolicy {
    Inherit,
    Prefer(ModelSelection),
    Require(ModelSelection),
    Select(ModelSelection),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolPolicy {
    All,
    Allow(ToolCapabilitySet),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentDefinition {
    pub id: AgentId,
    pub description: String,
    pub prompt: PromptPolicy,
    pub model: ModelPolicy,
    pub tools: ToolPolicy,
    pub reasoning: Option<ReasoningLevel>,
}

impl AgentDefinition {
    /// Hashes semantic fields only, using an unambiguous length-prefixed form.
    pub fn fingerprint(&self) -> AgentFingerprint {
        let mut hash = Sha256::new();
        hash_field(&mut hash, b"rho-agent-definition-v1");
        hash_field(&mut hash, self.id.as_str().as_bytes());
        hash_field(&mut hash, self.description.as_bytes());
        match &self.prompt {
            PromptPolicy::Extend(text) => {
                hash_field(&mut hash, b"prompt:extend");
                hash_field(&mut hash, text.as_bytes());
            }
            PromptPolicy::Replace(text) => {
                hash_field(&mut hash, b"prompt:replace");
                hash_field(&mut hash, text.as_bytes());
            }
        }
        match &self.model {
            ModelPolicy::Inherit => hash_field(&mut hash, b"model:inherit"),
            ModelPolicy::Prefer(selection) => hash_selection(&mut hash, b"model:prefer", selection),
            ModelPolicy::Require(selection) => {
                hash_selection(&mut hash, b"model:require", selection)
            }
            ModelPolicy::Select(selection) => hash_selection(&mut hash, b"model:select", selection),
        }
        match &self.tools {
            ToolPolicy::All => hash_field(&mut hash, b"tools:all"),
            ToolPolicy::Allow(tools) => {
                hash_field(&mut hash, b"tools:allow");
                for tool in tools {
                    hash_field(&mut hash, tool.as_str().as_bytes());
                }
            }
        }
        if let Some(reasoning) = self.reasoning {
            hash_field(&mut hash, b"reasoning:some");
            hash_field(&mut hash, reasoning.to_string().as_bytes());
        } else {
            hash_field(&mut hash, b"reasoning:none");
        }
        AgentFingerprint(hash.finalize().into())
    }
}

fn hash_selection(hash: &mut Sha256, policy: &[u8], selection: &ModelSelection) {
    hash_field(hash, policy);
    hash_field(hash, selection.provider.as_deref().unwrap_or("").as_bytes());
    hash_field(hash, selection.model.as_bytes());
}

fn hash_field(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value);
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct AgentFingerprint([u8; 32]);

impl fmt::Display for AgentFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}
