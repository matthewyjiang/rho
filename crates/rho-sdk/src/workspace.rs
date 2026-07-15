use std::{
    collections::BTreeSet,
    fmt,
    future::Future,
    path::{Component, Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use crate::Error;

/// Explicit filesystem scope supplied to tools.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, Error> {
        let root = root.into();
        if !root.is_absolute() {
            return Err(Error::InvalidConfiguration {
                message: "workspace root must be an absolute path".into(),
            });
        }
        if root
            .components()
            .any(|component| component == Component::ParentDir)
        {
            return Err(Error::InvalidConfiguration {
                message: "workspace root must not contain parent traversal".into(),
            });
        }
        let root = std::fs::canonicalize(&root).map_err(|error| Error::InvalidConfiguration {
            message: format!("workspace root must be an existing directory: {error}"),
        })?;
        if !root.is_dir() {
            return Err(Error::InvalidConfiguration {
                message: "workspace root must be a directory".into(),
            });
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves a path lexically and rejects traversal outside the workspace.
    pub fn resolve(&self, path: impl AsRef<Path>) -> Result<PathBuf, Error> {
        let path = path.as_ref();
        let relative = if path.is_absolute() {
            path.strip_prefix(&self.root)
                .map_err(|_| Error::PolicyDenied {
                    message: format!(
                        "path '{}' is outside workspace '{}'",
                        path.display(),
                        self.root.display()
                    ),
                })?
        } else {
            path
        };
        let mut resolved = self.root.clone();
        for component in relative.components() {
            match component {
                Component::Normal(part) => resolved.push(part),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(Error::PolicyDenied {
                        message: format!("path '{}' escapes the workspace", path.display()),
                    });
                }
            }
        }
        Ok(resolved)
    }

    /// Resolves an existing path and rejects symlinks that leave the workspace.
    pub fn resolve_existing(&self, path: impl AsRef<Path>) -> Result<PathBuf, Error> {
        let lexical = self.resolve(path)?;
        let canonical_root =
            std::fs::canonicalize(&self.root).map_err(|error| Error::PolicyDenied {
                message: format!("workspace root cannot be resolved: {error}"),
            })?;
        let canonical = std::fs::canonicalize(&lexical).map_err(|error| Error::PolicyDenied {
            message: format!("workspace path cannot be resolved: {error}"),
        })?;
        if !canonical.starts_with(&canonical_root) {
            return Err(Error::PolicyDenied {
                message: format!(
                    "path '{}' resolves outside the workspace",
                    lexical.display()
                ),
            });
        }
        Ok(canonical)
    }
}

/// Security-sensitive capability requested by a tool or adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CapabilityRequest {
    ReadPath {
        path: PathBuf,
    },
    WritePath {
        path: PathBuf,
    },
    ExecuteProcess {
        program: String,
        arguments: Vec<String>,
    },
    NetworkAccess {
        url: String,
    },
}

/// Decision returned by a [`WorkspacePolicy`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PolicyDecision {
    Allow,
    Deny { reason: String },
    RequireApproval { reason: String },
}

/// Host policy for filesystem, process, and network capabilities.
pub trait WorkspacePolicy: Send + Sync {
    fn evaluate(&self, request: &CapabilityRequest) -> PolicyDecision;
}

/// Policy that denies every security-sensitive capability.
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyAllPolicy;

impl WorkspacePolicy for DenyAllPolicy {
    fn evaluate(&self, _request: &CapabilityRequest) -> PolicyDecision {
        PolicyDecision::Deny {
            reason: "capability is denied by default".into(),
        }
    }
}

/// Explicit opt-in policy for common workspace capabilities.
#[derive(Clone, Debug, Default)]
pub struct ScopedWorkspacePolicy {
    read_paths: bool,
    write_paths: bool,
    processes: bool,
    network_hosts: BTreeSet<String>,
    require_approval: BTreeSet<CapabilityClass>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum CapabilityClass {
    Read,
    Write,
    Process,
    Network,
}

impl ScopedWorkspacePolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow_read_paths(mut self) -> Self {
        self.read_paths = true;
        self
    }

    pub fn allow_write_paths(mut self) -> Self {
        self.write_paths = true;
        self
    }

    pub fn allow_processes(mut self) -> Self {
        self.processes = true;
        self
    }

    pub fn allow_network_host(mut self, host: impl Into<String>) -> Self {
        self.network_hosts.insert(host.into().to_ascii_lowercase());
        self
    }

    pub fn require_read_approval(mut self) -> Self {
        self.require_approval.insert(CapabilityClass::Read);
        self
    }

    pub fn require_write_approval(mut self) -> Self {
        self.require_approval.insert(CapabilityClass::Write);
        self
    }

    pub fn require_process_approval(mut self) -> Self {
        self.require_approval.insert(CapabilityClass::Process);
        self
    }

    pub fn require_network_approval(mut self) -> Self {
        self.require_approval.insert(CapabilityClass::Network);
        self
    }
}

impl WorkspacePolicy for ScopedWorkspacePolicy {
    fn evaluate(&self, request: &CapabilityRequest) -> PolicyDecision {
        let (class, allowed) = match request {
            CapabilityRequest::ReadPath { .. } => (CapabilityClass::Read, self.read_paths),
            CapabilityRequest::WritePath { .. } => (CapabilityClass::Write, self.write_paths),
            CapabilityRequest::ExecuteProcess { .. } => (CapabilityClass::Process, self.processes),
            CapabilityRequest::NetworkAccess { url } => {
                let host = url::Url::parse(url)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_ascii_lowercase));
                (
                    CapabilityClass::Network,
                    host.is_some_and(|host| self.network_hosts.contains(&host)),
                )
            }
        };
        if !allowed {
            return PolicyDecision::Deny {
                reason: "capability is outside the configured policy".into(),
            };
        }
        if self.require_approval.contains(&class) {
            PolicyDecision::RequireApproval {
                reason: "host approval is required".into(),
            }
        } else {
            PolicyDecision::Allow
        }
    }
}

/// Host decision for one approval request.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ApprovalDecision {
    AllowOnce,
    Deny { reason: String },
}

/// Owned request supplied to an [`ApprovalHandler`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalRequest {
    capability: CapabilityRequest,
    reason: String,
}

impl ApprovalRequest {
    pub fn capability(&self) -> &CapabilityRequest {
        &self.capability
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// Future returned by approval handlers.
pub type ApprovalFuture<'a> = Pin<Box<dyn Future<Output = ApprovalDecision> + Send + 'a>>;

/// Host extension point for interactive or remote approval decisions.
pub trait ApprovalHandler: Send + Sync {
    fn request<'a>(&'a self, request: ApprovalRequest) -> ApprovalFuture<'a>;
}

/// Approval handler that denies every request.
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyApprovals;

impl ApprovalHandler for DenyApprovals {
    fn request<'a>(&'a self, _request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(async {
            ApprovalDecision::Deny {
                reason: "no approval handler is configured".into(),
            }
        })
    }
}

pub(crate) async fn authorize(
    policy: &Arc<dyn WorkspacePolicy>,
    approvals: &Arc<dyn ApprovalHandler>,
    request: CapabilityRequest,
) -> Result<(), Error> {
    match policy.evaluate(&request) {
        PolicyDecision::Allow => Ok(()),
        PolicyDecision::Deny { reason } => Err(Error::PolicyDenied { message: reason }),
        PolicyDecision::RequireApproval { reason } => {
            match approvals
                .request(ApprovalRequest {
                    capability: request,
                    reason,
                })
                .await
            {
                ApprovalDecision::AllowOnce => Ok(()),
                ApprovalDecision::Deny { reason } => Err(Error::PolicyDenied { message: reason }),
            }
        }
    }
}

impl fmt::Debug for dyn WorkspacePolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WorkspacePolicy(..)")
    }
}

impl fmt::Debug for dyn ApprovalHandler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApprovalHandler(..)")
    }
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
