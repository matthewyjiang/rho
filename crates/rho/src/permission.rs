use std::{
    fmt,
    path::Path,
    process::{Command, Stdio},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

use rho_sdk::{
    CapabilityKind, CapabilityOperation, CapabilityRequest, PathScope, PolicyDecision,
    WorkspacePolicy,
};

/// Lightweight permission mode that gates the model's most sensitive actions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PermissionMode {
    /// Current behavior: no policy checks; all capabilities are allowed.
    #[default]
    Bypass,
    /// Same capability gate as [`Self::AllowEdits`]; remaining writes, process
    /// execution, and unrecognized capability classes require classifier
    /// approval.
    Auto,
    /// Known reads, network access, skills, instruction discovery, and
    /// in-workspace writes to git-tracked files are free; other writes, process
    /// execution, and unrecognized capability classes require interactive
    /// approval.
    AllowEdits,
    /// Model may investigate but cannot change state. Known read, network,
    /// skill, and instruction-discovery capabilities are allowed; writes,
    /// process execution, and unrecognized capability classes are denied.
    Plan,
    /// Known reads, network access, skills, and instruction discovery are free;
    /// writes, process execution, and unrecognized capability classes require
    /// interactive approval.
    Supervised,
}

impl PermissionMode {
    pub const ALL: [Self; 5] = [
        Self::Bypass,
        Self::Auto,
        Self::AllowEdits,
        Self::Plan,
        Self::Supervised,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bypass => "bypass",
            Self::Auto => "auto",
            Self::AllowEdits => "allow_edits",
            Self::Plan => "plan",
            Self::Supervised => "supervised",
        }
    }

    /// Human-facing label shown in settings and TUI pickers.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bypass => "Bypass",
            Self::Auto => "Auto",
            Self::AllowEdits => "Allow edits",
            Self::Plan => "Plan",
            Self::Supervised => "Supervised",
        }
    }

    /// Lower is more restrictive. Used to compose a frozen ceiling with the
    /// current session mode.
    pub const fn restrictiveness_rank(self) -> u8 {
        match self {
            Self::Plan => 0,
            Self::Supervised => 1,
            Self::AllowEdits => 2,
            Self::Auto => 3,
            Self::Bypass => 4,
        }
    }

    /// Pure kind mapping: the default decision when request details do not
    /// refine it. [`ModePolicy::evaluate`] may allow in-workspace writes to
    /// git-tracked files for [`Self::Auto`] and [`Self::AllowEdits`].
    ///
    /// The wildcard arms intentionally fail closed if the non-exhaustive SDK
    /// enum gains a capability this application has not classified yet.
    pub fn decision_for(self, kind: CapabilityKind) -> PolicyDecision {
        match self {
            Self::Bypass => PolicyDecision::Allow,
            Self::Plan => match kind {
                CapabilityKind::Write | CapabilityKind::Process => PolicyDecision::Deny {
                    reason: "capability is not allowed in plan mode".into(),
                },
                CapabilityKind::Read
                | CapabilityKind::Network
                | CapabilityKind::Skill
                | CapabilityKind::InstructionDiscovery => PolicyDecision::Allow,
                _ => PolicyDecision::Deny {
                    reason: "unknown capability is not allowed in plan mode".into(),
                },
            },
            Self::Auto | Self::AllowEdits | Self::Supervised => match kind {
                // Empty reason: the approval prompt itself is the signal. Keep a
                // specific reason only when it adds information the chrome lacks.
                CapabilityKind::Write | CapabilityKind::Process => {
                    PolicyDecision::RequireApproval {
                        reason: String::new(),
                    }
                }
                CapabilityKind::Read
                | CapabilityKind::Network
                | CapabilityKind::Skill
                | CapabilityKind::InstructionDiscovery => PolicyDecision::Allow,
                _ => PolicyDecision::RequireApproval {
                    reason: "unknown capability requires host approval".into(),
                },
            },
        }
    }

    /// Builds the SDK policy that enforces this mode. Returns `None` for
    /// [`Self::Bypass`] so the caller can preserve its existing allow-everything
    /// path.
    ///
    /// The returned policy starts from [`Self::decision_for`] and, for
    /// [`Self::Auto`] and [`Self::AllowEdits`], allows primary-workspace writes
    /// to git-tracked files. `ScopedWorkspacePolicy` is not used here because it
    /// deny-defaults network destinations behind a per-host allowlist, which
    /// would break the "reads and network are free" contract of the checked
    /// modes.
    pub fn workspace_policy(self) -> Option<ModePolicy> {
        match self {
            Self::Bypass => None,
            Self::Auto | Self::AllowEdits | Self::Plan | Self::Supervised => {
                Some(ModePolicy { mode: self })
            }
        }
    }

    fn allows_tracked_workspace_edits(self) -> bool {
        matches!(self, Self::Auto | Self::AllowEdits)
    }
}

/// Policy that enforces a single [`PermissionMode`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct ModePolicy {
    mode: PermissionMode,
}

impl WorkspacePolicy for ModePolicy {
    fn evaluate(&self, request: &CapabilityRequest) -> PolicyDecision {
        if self.mode.allows_tracked_workspace_edits() && is_tracked_workspace_write(request) {
            return PolicyDecision::Allow;
        }
        self.mode.decision_for(request.kind())
    }
}

fn is_tracked_workspace_write(request: &CapabilityRequest) -> bool {
    match request.operation() {
        CapabilityOperation::WritePath {
            path,
            scope: PathScope::PrimaryWorkspace,
        } => path_is_git_tracked(path),
        _ => false,
    }
}

/// `git ls-files --error-unmatch` from the file's parent. Missing git, a
/// non-repo, or an untracked path all return false so the skip fails closed.
fn path_is_git_tracked(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let Some(file_name) = path.file_name() else {
        return false;
    };
    Command::new("git")
        .args(["ls-files", "--error-unmatch", "-z", "--"])
        .arg(file_name)
        .current_dir(parent)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PermissionModeParseError {
    value: String,
}

impl fmt::Display for PermissionModeParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown permission mode {:?}; expected bypass, auto, allow_edits, plan, or supervised",
            self.value
        )
    }
}

impl std::error::Error for PermissionModeParseError {}

impl FromStr for PermissionMode {
    type Err = PermissionModeParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "bypass" => Ok(Self::Bypass),
            "auto" => Ok(Self::Auto),
            "allow_edits" | "allow-edits" => Ok(Self::AllowEdits),
            "plan" => Ok(Self::Plan),
            "supervised" => Ok(Self::Supervised),
            _ => Err(PermissionModeParseError {
                value: value.to_string(),
            }),
        }
    }
}

impl Serialize for PermissionMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PermissionMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
#[path = "permission_tests.rs"]
mod tests;
