use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
    sync::{Arc, RwLock},
};

use serde::{Deserialize, Serialize};

use rho_sdk::{
    ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest, CapabilityKind,
    CapabilityOperation, CapabilityRequest, CapabilitySource, PathScope, PolicyDecision,
    WorkspacePolicy,
};

/// Lightweight permission mode that gates the model's most sensitive actions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PermissionMode {
    /// Current behavior: no policy checks; all capabilities are allowed.
    #[default]
    Bypass,
    /// Same capability gate as [`Self::AllowEdits`]; remaining writes, process
    /// execution, reads outside configured workspace roots, and unrecognized
    /// capability classes require classifier approval. In-workspace writes to
    /// git-tracked files and later writes to a path already allowed this
    /// session skip the classifier.
    Auto,
    /// Known workspace-scoped reads, the user's global instruction surfaces,
    /// network access, skills, instruction discovery, and in-workspace writes
    /// to git-tracked files are free. Later writes to a path already allowed
    /// this session are also free. Other writes, process execution, reads
    /// outside configured workspace roots, and unrecognized capability classes
    /// require interactive approval.
    AllowEdits,
    /// Model may investigate the workspace and the user's global instruction
    /// surfaces, but cannot change state. Those reads, network, skill, and
    /// instruction-discovery capabilities are allowed; writes, process
    /// execution, other reads outside configured workspace roots, and
    /// unrecognized capability classes are denied.
    Plan,
    /// Known workspace-scoped reads, the user's global instruction surfaces,
    /// network access, skills, and instruction discovery are free; writes,
    /// process execution, other reads outside configured workspace roots, and
    /// unrecognized capability classes require interactive approval.
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

    /// True for Auto and Allow edits: tracked workspace files and later writes
    /// to a path already allowed this session skip the remaining gate.
    pub(crate) const fn allows_tracked_workspace_edits(self) -> bool {
        self.write_authority().is_some()
    }

    /// Who can grant remembered path authority in this mode.
    ///
    /// Classifier approval and human approval are not interchangeable: a path
    /// remembered under one grantor must not skip the other grantor's gate.
    pub(crate) const fn write_authority(self) -> Option<WriteAuthority> {
        match self {
            Self::Auto => Some(WriteAuthority::Classifier),
            Self::AllowEdits => Some(WriteAuthority::Human),
            Self::Bypass | Self::Plan | Self::Supervised => None,
        }
    }

    /// Whether a grant from `authority` may skip this mode's remaining write gate.
    ///
    /// Allow edits accepts only a human grant. Auto also accepts a human grant
    /// because a person is a stronger approver than the classifier. The reverse
    /// is never true.
    pub(crate) const fn honors_write_authority(self, authority: WriteAuthority) -> bool {
        match self {
            Self::Auto => matches!(
                authority,
                WriteAuthority::Classifier | WriteAuthority::Human
            ),
            Self::AllowEdits => matches!(authority, WriteAuthority::Human),
            Self::Bypass | Self::Plan | Self::Supervised => false,
        }
    }

    /// Remaining gate for a read that resolved outside configured workspace
    /// roots. Bypass is unused here because it never installs a [`ModePolicy`].
    fn outside_workspace_read_decision(self) -> PolicyDecision {
        match self {
            Self::Bypass => PolicyDecision::Allow,
            Self::Plan => PolicyDecision::Deny {
                reason: "read outside the workspace is not allowed in plan mode".into(),
            },
            Self::Auto | Self::AllowEdits | Self::Supervised => PolicyDecision::RequireApproval {
                reason: String::new(),
            },
        }
    }

    /// Pure kind mapping: the default decision when request details do not
    /// refine it. This is **not** authorization for [`CapabilityKind::Read`].
    /// Read and instruction-discovery requests are location-sensitive and must
    /// go through [`ModePolicy::evaluate`]. Callers that treat
    /// `decision_for(Read) == Allow` as "no remaining gate" will miss the
    /// outside-workspace read check.
    ///
    /// [`ModePolicy::evaluate`] may allow in-workspace writes to git-tracked
    /// files, and later writes to a path already allowed this session, for
    /// [`Self::Auto`] and [`Self::AllowEdits`].
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
    /// Snapshots [`crate::paths::UserInstructionSurfaces::from_process`] (HOME)
    /// at construction. The returned policy starts from [`Self::decision_for`]
    /// and, for [`Self::Auto`] and [`Self::AllowEdits`], allows
    /// primary-workspace writes to git-tracked files and to paths already
    /// allowed this session. Reads stay free for configured workspace roots and
    /// for those user instruction surfaces; unrestricted filesystem paths
    /// follow the mode's remaining gate unless the built-in `workflow` tool
    /// requested them. `ScopedWorkspacePolicy` is not used here because it
    /// deny-defaults network destinations behind a per-host allowlist, which
    /// would break the "workspace reads and network are free" contract of the
    /// checked modes.
    ///
    /// `session_writes` carries the paths whose first write already passed the
    /// gate this session.
    pub fn workspace_policy(self, session_writes: SessionWriteLog) -> Option<ModePolicy> {
        match self {
            Self::Bypass => None,
            Self::Auto | Self::AllowEdits | Self::Plan | Self::Supervised => self
                .workspace_policy_with(
                    session_writes,
                    crate::paths::UserInstructionSurfaces::from_process(),
                ),
        }
    }

    fn workspace_policy_with(
        self,
        session_writes: SessionWriteLog,
        user_instructions: crate::paths::UserInstructionSurfaces,
    ) -> Option<ModePolicy> {
        match self {
            Self::Bypass => None,
            Self::Auto | Self::AllowEdits | Self::Plan | Self::Supervised => Some(ModePolicy {
                mode: self,
                session_writes,
                user_instructions,
            }),
        }
    }
}

/// Who granted a remembered in-workspace write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteAuthority {
    /// Auto-mode classifier (or its human escalation).
    Classifier,
    /// Interactive Allow edits approval.
    Human,
}

/// Which git fact a cached verdict describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum GitVerdict {
    Tracked,
    Ignored,
}

impl GitVerdict {
    /// The one git question each kind answers, so a cached kind can never be
    /// paired with the wrong probe.
    fn check(self, path: &Path) -> bool {
        match self {
            Self::Tracked => path_is_git_tracked(path),
            Self::Ignored => path_is_git_ignored(path),
        }
    }
}

/// Session-scoped paths whose first in-workspace write already passed the gate.
///
/// Each path is bound to the grantor that allowed it. Evaluation only skips the
/// gate when the current mode's grantor matches, so classifier approval cannot
/// become a human-approval bypass.
///
/// Also caches per-path git tracked/ignored verdicts for the session: every
/// uncached check spawns a synchronous `git` process on the authorize path, and
/// repeated edits to the same files would pay that every time. Verdicts are
/// accepted as stale for the session: a path that becomes untracked, or that a
/// mid-session `.gitignore` edit starts ignoring, keeps the verdict it was first
/// given until the session is cleared.
#[derive(Clone, Default)]
pub(crate) struct SessionWriteLog {
    paths: Arc<RwLock<HashMap<PathBuf, WriteAuthority>>>,
    git_verdicts: Arc<RwLock<HashMap<(PathBuf, GitVerdict), bool>>>,
}

impl SessionWriteLog {
    pub(crate) fn remember(&self, request: &CapabilityRequest, authority: WriteAuthority) {
        let Some(path) = rememberable_workspace_write(request, self) else {
            return;
        };
        self.paths
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(path, authority);
    }

    /// Approvals stay with the session when both modes can remember writes.
    /// Evaluation still filters by [`PermissionMode::honors_write_authority`],
    /// so a classifier grant cannot skip Allow edits.
    pub(crate) fn carried_across(self, from: PermissionMode, to: PermissionMode) -> Self {
        if from.allows_tracked_workspace_edits() && to.allows_tracked_workspace_edits() {
            self
        } else {
            Self::default()
        }
    }

    /// Drops every remembered path. Used when the live session identity changes
    /// so a later session cannot inherit another session's grants.
    pub(crate) fn clear(&self) {
        self.paths
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.git_verdicts
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    fn granted_by(&self, path: &Path) -> Option<WriteAuthority> {
        self.paths
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(path)
            .copied()
    }

    /// The session's answer for this path and git fact, running the git spawn
    /// for it only on the first ask.
    fn git_verdict(&self, path: &Path, kind: GitVerdict) -> bool {
        let key = (path.to_path_buf(), kind);
        if let Some(verdict) = self
            .git_verdicts
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&key)
        {
            return *verdict;
        }
        let verdict = kind.check(path);
        self.git_verdicts
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key, verdict);
        verdict
    }
}

impl fmt::Debug for SessionWriteLog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let len = self
            .paths
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        formatter
            .debug_struct("SessionWriteLog")
            .field("path_count", &len)
            .finish()
    }
}

/// Policy that enforces a single [`PermissionMode`].
#[derive(Clone, Debug)]
pub(crate) struct ModePolicy {
    mode: PermissionMode,
    session_writes: SessionWriteLog,
    user_instructions: crate::paths::UserInstructionSurfaces,
}

/// Where a path-bearing request sits for this mode's remaining gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathLocation {
    WorkspaceRooted,
    UserInstruction,
    Unrestricted,
}

impl ModePolicy {
    #[cfg(test)]
    fn for_home(mode: PermissionMode, home: &Path) -> Self {
        mode.workspace_policy_with(
            SessionWriteLog::default(),
            crate::paths::UserInstructionSurfaces::from_home(Some(home)),
        )
        .expect("checked mode has a policy")
    }

    #[cfg(test)]
    /// Records a write that already passed classifier or human approval.
    pub(crate) fn remember_approved_write(&self, request: &CapabilityRequest) {
        let Some(authority) = self.mode.write_authority() else {
            return;
        };
        self.session_writes.remember(request, authority);
    }

    fn location(&self, request: &CapabilityRequest) -> Option<PathLocation> {
        let (path, scope) = match request.operation() {
            CapabilityOperation::ReadPath { path, scope }
            | CapabilityOperation::WritePath { path, scope }
            | CapabilityOperation::DiscoverInstructions { path, scope } => (path.as_path(), scope),
            _ => return None,
        };
        Some(if path_scope_is_workspace_rooted(scope) {
            PathLocation::WorkspaceRooted
        } else if self.user_instructions.contains(path) {
            PathLocation::UserInstruction
        } else {
            PathLocation::Unrestricted
        })
    }

    fn evaluate_located(
        &self,
        request: &CapabilityRequest,
        location: PathLocation,
    ) -> PolicyDecision {
        match (request.kind(), location) {
            (
                CapabilityKind::Read | CapabilityKind::InstructionDiscovery,
                PathLocation::WorkspaceRooted | PathLocation::UserInstruction,
            ) => PolicyDecision::Allow,
            (
                CapabilityKind::Read | CapabilityKind::InstructionDiscovery,
                PathLocation::Unrestricted,
            ) => {
                if is_workflow_tool_read(request) {
                    PolicyDecision::Allow
                } else {
                    self.mode.outside_workspace_read_decision()
                }
            }
            (CapabilityKind::Write, PathLocation::WorkspaceRooted)
                if self.mode.allows_tracked_workspace_edits()
                    && is_free_workspace_write(request, &self.session_writes, self.mode) =>
            {
                PolicyDecision::Allow
            }
            _ => self.mode.decision_for(request.kind()),
        }
    }
}

impl WorkspacePolicy for ModePolicy {
    fn evaluate(&self, request: &CapabilityRequest) -> PolicyDecision {
        match self.location(request) {
            Some(location) => self.evaluate_located(request, location),
            None => self.mode.decision_for(request.kind()),
        }
    }
}

/// Records allowed primary-workspace writes so later edits skip the gate.
pub(crate) fn remember_allowed_workspace_writes(
    inner: Arc<dyn ApprovalHandler>,
    writes: SessionWriteLog,
    authority: WriteAuthority,
) -> Arc<dyn ApprovalHandler> {
    Arc::new(RememberingApprovals {
        inner,
        writes,
        authority,
    })
}

struct RememberingApprovals {
    inner: Arc<dyn ApprovalHandler>,
    writes: SessionWriteLog,
    authority: WriteAuthority,
}

impl ApprovalHandler for RememberingApprovals {
    fn request<'a>(&'a self, request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(async move {
            let capability = request.capability().clone();
            let decision = self.inner.request(request).await;
            if matches!(
                decision,
                ApprovalDecision::AllowOnce | ApprovalDecision::AllowForSession
            ) {
                self.writes.remember(&capability, self.authority);
            }
            decision
        })
    }

    fn reads_live_history(&self) -> bool {
        self.inner.reads_live_history()
    }
}

fn path_scope_is_workspace_rooted(scope: &PathScope) -> bool {
    match scope {
        PathScope::PrimaryWorkspace | PathScope::GrantedRoot { .. } => true,
        PathScope::UnrestrictedFilesystem => false,
        _ => false,
    }
}

/// Host-owned reads declared by the built-in `workflow` tool (plans, runs,
/// config, PATH resolution). The model's `read_file` of the same paths still
/// follows the unrestricted-filesystem gate.
fn is_workflow_tool_read(request: &CapabilityRequest) -> bool {
    matches!(request.operation(), CapabilityOperation::ReadPath { .. })
        && matches!(
            request.source(),
            CapabilitySource::BuiltInTool { name } if name == "workflow"
        )
}

fn is_free_workspace_write(
    request: &CapabilityRequest,
    session_writes: &SessionWriteLog,
    mode: PermissionMode,
) -> bool {
    match request.operation() {
        CapabilityOperation::WritePath { path, scope } => {
            if path_is_symlink(path) {
                return false;
            }
            matches!(scope, PathScope::PrimaryWorkspace)
                && (session_writes.git_verdict(path, GitVerdict::Tracked)
                    || (session_writes
                        .granted_by(path)
                        .is_some_and(|authority| mode.honors_write_authority(authority))
                        && !session_writes.git_verdict(path, GitVerdict::Ignored)))
        }
        _ => false,
    }
}

fn rememberable_workspace_write(
    request: &CapabilityRequest,
    session_writes: &SessionWriteLog,
) -> Option<PathBuf> {
    match request.operation() {
        CapabilityOperation::WritePath {
            path,
            scope: PathScope::PrimaryWorkspace,
        } if !session_writes.git_verdict(path, GitVerdict::Ignored) => Some(path.clone()),
        _ => None,
    }
}

/// Reports whether the path is a symbolic link. Git tracks symlinks, and a
/// tracked link can point outside the workspace, so the free-write skip never
/// applies to one. Only a missing path is treated as "not a link"; any other
/// metadata error keeps the write gated.
fn path_is_symlink(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata.file_type().is_symlink(),
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

/// `git ls-files --error-unmatch` from the file's parent. Missing git, a
/// non-repo, or an untracked path all return false so the skip fails closed.
fn path_is_git_tracked(path: &Path) -> bool {
    git_status_check(path, &["ls-files", "--error-unmatch", "-z", "--"])
}

/// `git check-ignore -q` from the file's parent. Missing git or a non-repo
/// means the path is not ignored, so the skip stays available.
fn path_is_git_ignored(path: &Path) -> bool {
    git_status_check(path, &["check-ignore", "-q", "--"])
}

/// Runs `git <args> <file_name>` from the path's parent and reports whether it
/// exited successfully. A path without a parent or file name, missing git, or a
/// non-repo all return false.
fn git_status_check(path: &Path, args: &[&str]) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let Some(file_name) = path.file_name() else {
        return false;
    };
    Command::new("git")
        .args(args)
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
