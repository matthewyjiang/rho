use std::{
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};

/// Independently grantable classes of security-sensitive work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum CapabilityKind {
    Read,
    Write,
    Process,
    Network,
    Skill,
    InstructionDiscovery,
}

impl CapabilityKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Process => "process",
            Self::Network => "network",
            Self::Skill => "skill",
            Self::InstructionDiscovery => "instruction discovery",
        }
    }
}

/// Identifies whether an operation comes from trusted host code or an SDK/application built-in.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CapabilitySource {
    HostProvidedTool { name: String },
    BuiltInTool { name: String },
    PromptConstruction,
}

impl CapabilitySource {
    pub fn host_tool(name: impl Into<String>) -> Self {
        Self::HostProvidedTool { name: name.into() }
    }

    pub fn built_in_tool(name: impl Into<String>) -> Self {
        Self::BuiltInTool { name: name.into() }
    }
}

/// Filesystem scope containing a resolved path.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathScope {
    PrimaryWorkspace,
    GrantedRoot { root: PathBuf },
}

/// How an executable name is resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExecutableSelection {
    ExactPath,
    SearchPath,
}

/// How a process executable is selected.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProcessInvocation {
    Executable {
        executable: PathBuf,
        selection: ExecutableSelection,
        arguments: Vec<String>,
    },
    Shell {
        executable: PathBuf,
        selection: ExecutableSelection,
        arguments: Vec<String>,
        command: String,
    },
}

impl ProcessInvocation {
    pub fn executable(executable: impl Into<PathBuf>, arguments: Vec<String>) -> Self {
        Self::Executable {
            executable: executable.into(),
            selection: ExecutableSelection::ExactPath,
            arguments,
        }
    }

    /// Creates a direct executable invocation resolved through the inherited `PATH`.
    pub fn executable_from_path(executable: impl Into<PathBuf>, arguments: Vec<String>) -> Self {
        Self::Executable {
            executable: executable.into(),
            selection: ExecutableSelection::SearchPath,
            arguments,
        }
    }

    pub fn shell(
        executable: impl Into<PathBuf>,
        arguments: Vec<String>,
        command: impl Into<String>,
    ) -> Self {
        Self::Shell {
            executable: executable.into(),
            selection: ExecutableSelection::ExactPath,
            arguments,
            command: command.into(),
        }
    }

    pub fn shell_from_path(
        executable: impl Into<PathBuf>,
        arguments: Vec<String>,
        command: impl Into<String>,
    ) -> Self {
        Self::Shell {
            executable: executable.into(),
            selection: ExecutableSelection::SearchPath,
            arguments,
            command: command.into(),
        }
    }

    pub fn executable_path(&self) -> &Path {
        match self {
            Self::Executable { executable, .. } | Self::Shell { executable, .. } => executable,
        }
    }

    pub fn executable_selection(&self) -> ExecutableSelection {
        match self {
            Self::Executable { selection, .. } | Self::Shell { selection, .. } => *selection,
        }
    }

    pub fn arguments(&self) -> &[String] {
        match self {
            Self::Executable { arguments, .. } | Self::Shell { arguments, .. } => arguments,
        }
    }

    /// Returns shell text only when execution explicitly uses a shell.
    pub fn shell_command(&self) -> Option<&str> {
        match self {
            Self::Shell { command, .. } => Some(command),
            Self::Executable { .. } => None,
        }
    }

    pub fn uses_shell(&self) -> bool {
        matches!(self, Self::Shell { .. })
    }
}

impl fmt::Debug for ProcessInvocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct(match self {
            Self::Executable { .. } => "Executable",
            Self::Shell { .. } => "Shell",
        });
        debug.field("executable", &self.executable_path());
        debug.field("selection", &self.executable_selection());
        debug.field("argument_count", &self.arguments().len());
        if self.uses_shell() {
            debug.field("command", &"[redacted]");
        }
        debug.finish()
    }
}

/// Ambient environment made available to a child process.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProcessEnvironment {
    Empty,
    InheritAll,
    /// Inherit the ambient environment after removing these variable names.
    InheritExcept {
        variable_names: Vec<String>,
    },
    InheritListed {
        variable_names: Vec<String>,
    },
}

impl ProcessEnvironment {
    /// Inherit ambient variables except the provided names.
    pub fn inherit_except(variable_names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut variable_names: Vec<String> = variable_names.into_iter().map(Into::into).collect();
        variable_names.sort_unstable();
        variable_names.dedup();
        if variable_names.is_empty() {
            return Self::InheritAll;
        }
        Self::InheritExcept { variable_names }
    }
}

/// Explicit bounds on process runtime and captured output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessOutputLimits {
    max_output_bytes: usize,
    timeout: Option<Duration>,
}

impl ProcessOutputLimits {
    pub fn new(max_output_bytes: usize, timeout: Option<Duration>) -> Self {
        Self {
            max_output_bytes: max_output_bytes.max(1),
            timeout,
        }
    }

    pub fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }

    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }
}

/// Complete, structured process facts presented to policy and approval handlers.
#[derive(Clone, PartialEq, Eq)]
pub struct ProcessExecution {
    working_directory: PathBuf,
    invocation: ProcessInvocation,
    environment: ProcessEnvironment,
    output_limits: ProcessOutputLimits,
}

impl ProcessExecution {
    pub fn new(
        working_directory: impl Into<PathBuf>,
        invocation: ProcessInvocation,
        environment: ProcessEnvironment,
        output_limits: ProcessOutputLimits,
    ) -> Self {
        Self {
            working_directory: working_directory.into(),
            invocation,
            environment,
            output_limits,
        }
    }

    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }

    pub fn invocation(&self) -> &ProcessInvocation {
        &self.invocation
    }

    pub fn environment(&self) -> &ProcessEnvironment {
        &self.environment
    }

    pub fn output_limits(&self) -> ProcessOutputLimits {
        self.output_limits
    }
}

impl fmt::Debug for ProcessExecution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessExecution")
            .field("working_directory", &self.working_directory)
            .field("invocation", &self.invocation)
            .field("environment", &self.environment)
            .field("output_limits", &self.output_limits)
            .finish()
    }
}

/// Destination of a network-capable operation.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum NetworkTarget {
    Url(String),
    ToolManaged,
}

impl NetworkTarget {
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Url(url) => Some(url),
            Self::ToolManaged => None,
        }
    }
}

impl fmt::Debug for NetworkTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url(url) => {
                let origin = url::Url::parse(url)
                    .ok()
                    .map(|parsed| parsed.origin().ascii_serialization())
                    .unwrap_or_else(|| "[invalid url]".into());
                formatter.debug_tuple("Url").field(&origin).finish()
            }
            Self::ToolManaged => formatter.write_str("ToolManaged"),
        }
    }
}

/// Structured operation evaluated by a workspace policy.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CapabilityOperation {
    ReadPath { path: PathBuf, scope: PathScope },
    WritePath { path: PathBuf, scope: PathScope },
    ExecuteProcess(ProcessExecution),
    NetworkAccess(NetworkTarget),
    LoadSkill { name: String, path: Option<PathBuf> },
    DiscoverInstructions { path: PathBuf, scope: PathScope },
}

/// Security-sensitive authority requested by a tool or prompt adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityRequest {
    operation: CapabilityOperation,
    source: CapabilitySource,
}

impl CapabilityRequest {
    pub fn new(operation: CapabilityOperation, source: CapabilitySource) -> Self {
        Self { operation, source }
    }

    pub fn read_path(path: impl Into<PathBuf>, scope: PathScope, source: CapabilitySource) -> Self {
        Self::new(
            CapabilityOperation::ReadPath {
                path: path.into(),
                scope,
            },
            source,
        )
    }

    pub fn write_path(
        path: impl Into<PathBuf>,
        scope: PathScope,
        source: CapabilitySource,
    ) -> Self {
        Self::new(
            CapabilityOperation::WritePath {
                path: path.into(),
                scope,
            },
            source,
        )
    }

    pub fn process(execution: ProcessExecution, source: CapabilitySource) -> Self {
        Self::new(CapabilityOperation::ExecuteProcess(execution), source)
    }

    pub fn network(target: NetworkTarget, source: CapabilitySource) -> Self {
        Self::new(CapabilityOperation::NetworkAccess(target), source)
    }

    pub fn skill(name: impl Into<String>, path: Option<PathBuf>, source: CapabilitySource) -> Self {
        Self::new(
            CapabilityOperation::LoadSkill {
                name: name.into(),
                path,
            },
            source,
        )
    }

    pub fn instruction_discovery(
        path: impl Into<PathBuf>,
        scope: PathScope,
        source: CapabilitySource,
    ) -> Self {
        Self::new(
            CapabilityOperation::DiscoverInstructions {
                path: path.into(),
                scope,
            },
            source,
        )
    }

    pub fn operation(&self) -> &CapabilityOperation {
        &self.operation
    }

    pub fn source(&self) -> &CapabilitySource {
        &self.source
    }

    pub fn kind(&self) -> CapabilityKind {
        match self.operation {
            CapabilityOperation::ReadPath { .. } => CapabilityKind::Read,
            CapabilityOperation::WritePath { .. } => CapabilityKind::Write,
            CapabilityOperation::ExecuteProcess(_) => CapabilityKind::Process,
            CapabilityOperation::NetworkAccess(_) => CapabilityKind::Network,
            CapabilityOperation::LoadSkill { .. } => CapabilityKind::Skill,
            CapabilityOperation::DiscoverInstructions { .. } => {
                CapabilityKind::InstructionDiscovery
            }
        }
    }

    pub(crate) fn is_outside_primary_root(&self) -> bool {
        matches!(
            &self.operation,
            CapabilityOperation::ReadPath {
                scope: PathScope::GrantedRoot { .. },
                ..
            } | CapabilityOperation::WritePath {
                scope: PathScope::GrantedRoot { .. },
                ..
            } | CapabilityOperation::DiscoverInstructions {
                scope: PathScope::GrantedRoot { .. },
                ..
            }
        )
    }
}
