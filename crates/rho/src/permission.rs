use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use rho_sdk::{CapabilityKind, CapabilityRequest, PolicyDecision, WorkspacePolicy};

/// Lightweight permission mode that gates the model's most sensitive actions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PermissionMode {
    /// Current behavior: no policy checks; all capabilities are allowed.
    #[default]
    Auto,
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
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Plan => "plan",
            Self::Supervised => "supervised",
        }
    }

    /// Human-facing label shown in settings and TUI pickers.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Plan => "Plan",
            Self::Supervised => "Supervised",
        }
    }

    /// Pure decision mapping: the single source of truth for what each mode does
    /// for a given capability class. The wildcard arms intentionally fail closed
    /// if the non-exhaustive SDK enum gains a capability this application has not
    /// classified yet.
    pub fn decision_for(self, kind: CapabilityKind) -> PolicyDecision {
        match self {
            Self::Auto => PolicyDecision::Allow,
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
            Self::Supervised => match kind {
                CapabilityKind::Write | CapabilityKind::Process => {
                    PolicyDecision::RequireApproval {
                        reason: "host approval is required".into(),
                    }
                }
                CapabilityKind::Read
                | CapabilityKind::Network
                | CapabilityKind::Skill
                | CapabilityKind::InstructionDiscovery => PolicyDecision::Allow,
                _ => PolicyDecision::RequireApproval {
                    reason: "host approval is required for unknown capability".into(),
                },
            },
        }
    }

    /// Builds the SDK policy that enforces this mode. Returns `None` for
    /// [`Self::Auto`] so the caller can preserve its existing allow-everything
    /// path.
    ///
    /// The returned policy delegates every request to [`Self::decision_for`], so
    /// it allows network access freely. `ScopedWorkspacePolicy` is not used here
    /// because it deny-defaults network destinations behind a per-host allowlist,
    /// which would break the "reads and network are free" contract of both
    /// non-auto modes.
    pub fn workspace_policy(self) -> Option<ModePolicy> {
        match self {
            Self::Auto => None,
            Self::Plan | Self::Supervised => Some(ModePolicy { mode: self }),
        }
    }
}

/// Policy that enforces a single [`PermissionMode`] by delegating to
/// [`PermissionMode::decision_for`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct ModePolicy {
    mode: PermissionMode,
}

impl WorkspacePolicy for ModePolicy {
    fn evaluate(&self, request: &CapabilityRequest) -> PolicyDecision {
        self.mode.decision_for(request.kind())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PermissionModeParseError {
    value: String,
}

impl fmt::Display for PermissionModeParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown permission mode {:?}; expected auto, plan, or supervised",
            self.value
        )
    }
}

impl std::error::Error for PermissionModeParseError {}

impl FromStr for PermissionMode {
    type Err = PermissionModeParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
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
