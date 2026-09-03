use std::collections::{BTreeMap, BTreeSet};

use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

use super::{
    AttemptNumber, InputName, NodeCompletion, NodeId, OutputSchema, WorkflowName, WorkflowValue,
};

pub(crate) const FROZEN_WORKFLOW_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Digest(pub(crate) String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlannerIdentity {
    pub(crate) name: String,
    pub(crate) format_version: u32,
    pub(crate) starlark_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourceManifest {
    pub(crate) entry_label: String,
    pub(crate) modules: BTreeMap<String, SourceFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourceFile {
    pub(crate) digest: Digest,
    pub(crate) bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FrozenWorkflow {
    pub(crate) schema_version: u32,
    pub(crate) planner: PlannerIdentity,
    pub(crate) graph_digest: Digest,
    pub(crate) sources: SourceManifest,
    pub(crate) inputs: BTreeMap<InputName, WorkflowValue>,
    pub(crate) graph: WorkflowGraph,
    pub(crate) resolved_nodes: BTreeMap<NodeId, ResolvedNode>,
    pub(crate) scheduler: FrozenSchedulerSettings,
    pub(crate) runtime_limits: super::FrozenRuntimeLimits,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkflowGraph {
    pub(crate) name: WorkflowName,
    pub(crate) nodes: BTreeMap<NodeId, Node>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Node {
    pub(crate) id: NodeId,
    pub(crate) display_name: String,
    pub(crate) needs: Vec<NodeId>,
    pub(crate) condition: Option<Condition>,
    pub(crate) execution: NodeExecution,
    pub(crate) access: WorkspaceAccess,
    pub(crate) allow_failure: bool,
    pub(crate) timeout_seconds: u64,
    pub(crate) max_output_bytes: u64,
}

impl Node {
    pub(crate) fn output_schema(&self) -> Option<&OutputSchema> {
        self.execution.output_schema()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum NodeExecution {
    Agent(AgentNode),
    Command(CommandNode),
}

impl NodeExecution {
    pub(crate) fn output_schema(&self) -> Option<&OutputSchema> {
        match self {
            Self::Agent(node) => node.output.as_ref(),
            Self::Command(node) => node.output(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkspaceAccess {
    ReadOnly,
    Mutating,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AgentNode {
    pub(crate) agent: String,
    pub(crate) prompt: Template,
    pub(crate) output: Option<OutputSchema>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "invocation", rename_all = "snake_case")]
pub(crate) enum CommandNode {
    Direct {
        executable: String,
        arguments: Vec<Template>,
        cwd: String,
        output: Option<OutputSchema>,
    },
    Shell {
        executable: String,
        arguments: Vec<String>,
        command: String,
        cwd: String,
        output: Option<OutputSchema>,
    },
}

impl CommandNode {
    pub(crate) fn output(&self) -> Option<&OutputSchema> {
        match self {
            Self::Direct { output, .. } | Self::Shell { output, .. } => output.as_ref(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Template(pub(crate) Vec<TemplatePart>);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum TemplatePart {
    Literal { value: String },
    Output { reference: OutputReference },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OutputReference {
    pub(crate) node: NodeId,
    pub(crate) path: OutputPath,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct OutputPath(pub(crate) Vec<String>);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Condition {
    NodeStatus {
        node: NodeId,
        matches: BTreeSet<NodeTerminalState>,
    },
    CommandExit {
        node: NodeId,
        predicate: ExitCodePredicate,
    },
    Output {
        node: NodeId,
        path: OutputPath,
        predicate: ValuePredicate,
    },
    All {
        conditions: Vec<Condition>,
    },
    Any {
        conditions: Vec<Condition>,
    },
    Not {
        condition: Box<Condition>,
    },
}

// Receipt: limit_receipt.json planning.accepted.condition_depth. This matches
// the planning budget, so stored data cannot bypass the planning contract.
pub(crate) const CONDITION_DEPTH_LIMIT: usize = 2;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ConditionWire {
    NodeStatus {
        node: NodeId,
        matches: BTreeSet<NodeTerminalState>,
    },
    CommandExit {
        node: NodeId,
        predicate: ExitCodePredicate,
    },
    Output {
        node: NodeId,
        path: OutputPath,
        predicate: ValuePredicate,
    },
    All {
        conditions: Vec<ConditionWire>,
    },
    Any {
        conditions: Vec<ConditionWire>,
    },
    Not {
        condition: Box<ConditionWire>,
    },
}

impl ConditionWire {
    fn into_condition(self) -> Condition {
        match self {
            Self::NodeStatus { node, matches } => Condition::NodeStatus { node, matches },
            Self::CommandExit { node, predicate } => Condition::CommandExit { node, predicate },
            Self::Output {
                node,
                path,
                predicate,
            } => Condition::Output {
                node,
                path,
                predicate,
            },
            Self::All { conditions } => Condition::All {
                conditions: conditions.into_iter().map(Self::into_condition).collect(),
            },
            Self::Any { conditions } => Condition::Any {
                conditions: conditions.into_iter().map(Self::into_condition).collect(),
            },
            Self::Not { condition } => Condition::Not {
                condition: Box::new(condition.into_condition()),
            },
        }
    }
}

impl<'de> Deserialize<'de> for Condition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let mut pending = vec![(&value, 1_usize)];
        while let Some((value, depth)) = pending.pop() {
            if depth > CONDITION_DEPTH_LIMIT {
                return Err(D::Error::custom(format!(
                    "condition depth {depth} exceeds limit {CONDITION_DEPTH_LIMIT}"
                )));
            }
            match value.get("type").and_then(serde_json::Value::as_str) {
                Some("all" | "any") => {
                    if let Some(conditions) = value
                        .get("conditions")
                        .and_then(serde_json::Value::as_array)
                    {
                        pending.extend(conditions.iter().map(|condition| (condition, depth + 1)));
                    }
                }
                Some("not") => {
                    if let Some(condition) = value.get("condition") {
                        pending.push((condition, depth + 1));
                    }
                }
                _ => {}
            }
        }
        serde_json::from_value::<ConditionWire>(value)
            .map(ConditionWire::into_condition)
            .map_err(D::Error::custom)
    }
}

impl Condition {
    pub(crate) fn referenced_nodes(&self, nodes: &mut BTreeSet<NodeId>) {
        match self {
            Self::NodeStatus { node, .. }
            | Self::CommandExit { node, .. }
            | Self::Output { node, .. } => {
                nodes.insert(node.clone());
            }
            Self::All { conditions } | Self::Any { conditions } => {
                for condition in conditions {
                    condition.referenced_nodes(nodes);
                }
            }
            Self::Not { condition } => condition.referenced_nodes(nodes),
        }
    }

    pub(crate) fn depth(&self) -> usize {
        match self {
            Self::All { conditions } | Self::Any { conditions } => {
                1 + conditions.iter().map(Self::depth).max().unwrap_or(0)
            }
            Self::Not { condition } => 1 + condition.depth(),
            Self::NodeStatus { .. } | Self::CommandExit { .. } | Self::Output { .. } => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub(crate) enum ExitCodePredicate {
    Equals(i32),
    IsOneOf(BTreeSet<i32>),
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub(crate) enum ValuePredicate {
    Equals(WorkflowValue),
    NotEquals(WorkflowValue),
    IsOneOf(BTreeSet<WorkflowValue>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub(crate) enum ResolvedNode {
    Agent(Box<ResolvedAgent>),
    Command(Box<ResolvedCommand>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResolvedAgent {
    pub(crate) agent_id: String,
    pub(crate) fingerprint: String,
    pub(crate) runtime: AgentRuntime,
    pub(crate) source_origin: String,
    pub(crate) trust_required: bool,
    pub(crate) prompt_policy: String,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) reasoning: Option<String>,
    pub(crate) step_limit: u64,
    pub(crate) capabilities: BTreeSet<String>,
    pub(crate) permission_ceiling: String,
    pub(crate) auth_profile: Option<String>,
    pub(crate) executable: Option<String>,
    pub(crate) executable_identity: Option<ExecutableIdentity>,
    pub(crate) arguments: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentRuntime {
    Rho,
    ClaudeCli,
    Cursor,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResolvedCommand {
    pub(crate) executable: String,
    pub(crate) executable_identity: ExecutableIdentity,
    pub(crate) exact_path: bool,
    pub(crate) cwd: String,
    pub(crate) cwd_identity: FrozenPathIdentity,
    pub(crate) environment_policy: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ExecutableIdentity {
    pub(crate) file: FrozenPathIdentity,
    pub(crate) interpreter: Option<FrozenPathIdentity>,
    pub(crate) interpreter_arguments: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FrozenPathIdentity {
    pub(crate) canonical_path: String,
    pub(crate) kind: FrozenPathKind,
    pub(crate) byte_len: u64,
    pub(crate) content_digest: Option<Digest>,
    pub(crate) platform_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FrozenPathKind {
    File,
    Directory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FrozenSchedulerSettings {
    pub(crate) max_parallel_nodes: u32,
    pub(crate) max_parallel_agents: u32,
    pub(crate) max_parallel_commands: u32,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NodeTerminalState {
    Success,
    Failure,
    Denial,
    Cancellation,
    Skipped,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum NodeState {
    Pending,
    Ready,
    Running { attempt: AttemptNumber },
    Terminal { outcome: NodeTerminalState },
}

impl NodeTerminalState {
    /// The public terminal-outcome token, identical to the serialized form.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Denial => "denial",
            Self::Cancellation => "cancellation",
            Self::Skipped => "skipped",
            Self::Blocked => "blocked",
        }
    }
}

impl NodeState {
    pub(crate) fn terminal(&self) -> Option<NodeTerminalState> {
        match self {
            Self::Terminal { outcome } => Some(*outcome),
            _ => None,
        }
    }

    /// The public node-state token, flattening `Terminal` to its outcome.
    ///
    /// Shared by the model-facing workflow tool and workflow notifications so
    /// one node vocabulary cannot drift across call sites.
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Running { .. } => "running",
            Self::Terminal { outcome } => outcome.as_str(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CommandExit {
    Code { code: i32 },
    Signal { signal: i32 },
    Timeout,
    Cancellation,
    Abnormal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkflowOutcome {
    Success,
    Failure,
    Denial,
    Cancellation,
    Blocked,
}

impl WorkflowOutcome {
    /// The public run-outcome token, identical to the serialized form.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Denial => "denial",
            Self::Cancellation => "cancellation",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunLifecycle {
    Planned,
    Running,
    Cancelling,
    Completed,
    NeedsRecovery,
}

impl RunLifecycle {
    /// True while an owner may still be driving the run (`Running` / `Cancelling`).
    ///
    /// Shared by TUI leave/cancel policy, delete guards, and matrix adapters so
    /// the live set cannot drift across call sites.
    pub(crate) const fn is_live(self) -> bool {
        matches!(self, Self::Running | Self::Cancelling)
    }

    /// The public lifecycle token, identical to the serialized form.
    ///
    /// Every model-facing, CLI, and hook surface renders lifecycle through this
    /// method so one vocabulary cannot drift across call sites. TUI prose
    /// (`finished`, `needs recovery`) is deliberately separate.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Completed => "completed",
            Self::NeedsRecovery => "needs_recovery",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkflowState {
    pub(crate) revision: u64,
    pub(crate) lifecycle: RunLifecycle,
    pub(crate) outcome: Option<WorkflowOutcome>,
    pub(crate) cancellation_requested: bool,
    pub(crate) nodes: BTreeMap<NodeId, NodeState>,
    pub(crate) command_exits: BTreeMap<NodeId, CommandExit>,
    pub(crate) outputs: BTreeMap<NodeId, WorkflowValue>,
    pub(crate) completions: BTreeMap<NodeId, NodeCompletion>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SchedulerCapacity {
    pub(crate) total: u32,
    pub(crate) agents: u32,
    pub(crate) commands: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SchedulerAction {
    MarkReady {
        node: NodeId,
    },
    MarkTerminal {
        node: NodeId,
        outcome: NodeTerminalState,
    },
    Launch {
        node: NodeId,
        access: WorkspaceAccess,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SchedulerEvent {
    MarkReady {
        node: NodeId,
    },
    Launched {
        node: NodeId,
        attempt: AttemptNumber,
    },
    Finished {
        node: NodeId,
        completion: Box<NodeCompletion>,
    },
    CancellationRequested,
    ResetNode {
        node: NodeId,
        reason: NodeResetReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NodeResetReason {
    InterruptedRecovery,
    CleanCancellation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TruthValue {
    True,
    False,
    Unavailable,
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
