//! Deserialize Starlark `__rho_type` JSON into domain workflow types.
//!
//! Shape validation uses Serde. Planning limits and domain checks stay in the
//! conversion step so error policy is not lost to naive derives on live types.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer};

use crate::workflow::{
    AgentNode, CommandNode, Condition, ExitCodePredicate, InputSchema, Node, NodeExecution, NodeId,
    NodeTerminalState, ObjectFieldSchema, OutputPath, OutputReference, OutputSchema,
    PlanningLimits, Template, TemplatePart, ValuePredicate, WorkflowError, WorkflowGraph,
    WorkflowName, WorkflowResult, WorkflowValue, WorkspaceAccess,
};

pub(super) fn parse_graph(
    value: serde_json::Value,
    limits: &PlanningLimits,
) -> WorkflowResult<WorkflowGraph> {
    let wire: RhoWorkflow = decode(value)?;
    wire.into_graph(limits)
}

pub(super) fn parse_input_schema(value: &serde_json::Value) -> WorkflowResult<InputSchema> {
    let wire: RhoInputSchema = decode_ref(value)?;
    wire.into_schema()
}

fn decode<T: for<'de> Deserialize<'de>>(value: serde_json::Value) -> WorkflowResult<T> {
    serde_json::from_value(value).map_err(decode_error)
}

fn decode_ref<T: for<'de> Deserialize<'de>>(value: &serde_json::Value) -> WorkflowResult<T> {
    T::deserialize(value).map_err(decode_error)
}

fn decode_error(error: serde_json::Error) -> WorkflowError {
    WorkflowError::Starlark(error.to_string())
}

fn starlark(message: impl Into<String>) -> WorkflowError {
    WorkflowError::Starlark(message.into())
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
#[serde(tag = "__rho_type")]
enum RhoWorkflow {
    #[serde(rename = "workflow")]
    Workflow { name: String, nodes: Vec<RhoNode> },
}

impl RhoWorkflow {
    fn into_graph(self, limits: &PlanningLimits) -> WorkflowResult<WorkflowGraph> {
        let Self::Workflow { name, nodes } = self;
        limits.node_count.check(nodes.len() as u64)?;
        let mut graph_nodes = BTreeMap::new();
        let mut edges = 0_u64;
        for node in nodes {
            let node = node.into_node(limits)?;
            edges = edges.saturating_add(node.needs.len() as u64);
            if graph_nodes.insert(node.id.clone(), node).is_some() {
                return Err(starlark("workflow contains duplicate node IDs"));
            }
        }
        limits.edge_count.check(edges)?;
        let graph = WorkflowGraph {
            name: WorkflowName::new(name)?,
            nodes: graph_nodes,
        };
        for node in graph.nodes.values() {
            if let Some(condition) = &node.condition {
                limits.condition_depth.check(condition.depth() as u64)?;
            }
            if let Some(schema) = node.output_schema() {
                limits.schema_depth.check(schema.depth() as u64)?;
                limits
                    .schema_bytes
                    .check(serde_json::to_vec(schema)?.len() as u64)?;
            }
        }
        limits
            .graph_bytes
            .check(serde_json::to_vec(&graph)?.len() as u64)?;
        Ok(graph)
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "__rho_type")]
enum RhoNode {
    #[serde(rename = "agent")]
    Agent {
        name: String,
        #[serde(default, deserialize_with = "null_default")]
        needs: Vec<String>,
        #[serde(default)]
        when: Option<RhoCondition>,
        #[serde(default)]
        allow_failure: bool,
        timeout_seconds: u64,
        max_output_bytes: u64,
        agent: String,
        prompt: RhoTemplate,
        #[serde(default)]
        output: Option<RhoOutputSpec>,
        access: RhoAccess,
    },
    #[serde(rename = "command")]
    Command {
        name: String,
        #[serde(default, deserialize_with = "null_default")]
        needs: Vec<String>,
        #[serde(default)]
        when: Option<RhoCondition>,
        #[serde(default)]
        allow_failure: bool,
        timeout_seconds: u64,
        max_output_bytes: u64,
        argv: Vec<RhoTemplate>,
        cwd: String,
        #[serde(default)]
        output: Option<RhoOutputSpec>,
    },
    #[serde(rename = "shell")]
    Shell {
        name: String,
        #[serde(default, deserialize_with = "null_default")]
        needs: Vec<String>,
        #[serde(default)]
        when: Option<RhoCondition>,
        #[serde(default)]
        allow_failure: bool,
        timeout_seconds: u64,
        max_output_bytes: u64,
        executable: String,
        arguments: Vec<String>,
        command: String,
        cwd: String,
        #[serde(default)]
        output: Option<RhoOutputSpec>,
    },
}

impl RhoNode {
    fn into_node(self, limits: &PlanningLimits) -> WorkflowResult<Node> {
        let (
            name,
            needs,
            when,
            allow_failure,
            timeout_seconds,
            max_output_bytes,
            execution,
            access,
        ) = match self {
            Self::Agent {
                name,
                needs,
                when,
                allow_failure,
                timeout_seconds,
                max_output_bytes,
                agent,
                prompt,
                output,
                access,
            } => (
                name,
                needs,
                when,
                allow_failure,
                timeout_seconds,
                max_output_bytes,
                NodeExecution::Agent(AgentNode {
                    agent,
                    prompt: prompt.into_template()?,
                    output: output.map(|spec| spec.into_schema(limits)).transpose()?,
                }),
                access.into_access()?,
            ),
            Self::Command {
                name,
                needs,
                when,
                allow_failure,
                timeout_seconds,
                max_output_bytes,
                argv,
                cwd,
                output,
            } => {
                let mut argv = argv.into_iter();
                let executable = match argv.next() {
                    Some(RhoTemplate::Literal(value)) => value,
                    _ => {
                        return Err(starlark(
                            "command argv must start with a static executable string",
                        ))
                    }
                };
                let arguments = argv
                    .map(RhoTemplate::into_template)
                    .collect::<WorkflowResult<_>>()?;
                (
                    name,
                    needs,
                    when,
                    allow_failure,
                    timeout_seconds,
                    max_output_bytes,
                    NodeExecution::Command(CommandNode::Direct {
                        executable,
                        arguments,
                        cwd,
                        output: output.map(|spec| spec.into_schema(limits)).transpose()?,
                    }),
                    WorkspaceAccess::Mutating,
                )
            }
            Self::Shell {
                name,
                needs,
                when,
                allow_failure,
                timeout_seconds,
                max_output_bytes,
                executable,
                arguments,
                command,
                cwd,
                output,
            } => (
                name,
                needs,
                when,
                allow_failure,
                timeout_seconds,
                max_output_bytes,
                NodeExecution::Command(CommandNode::Shell {
                    executable,
                    arguments,
                    command,
                    cwd,
                    output: output.map(|spec| spec.into_schema(limits)).transpose()?,
                }),
                WorkspaceAccess::Mutating,
            ),
        };

        limits.node_timeout_seconds.check_nonzero(timeout_seconds)?;
        limits
            .retained_output_per_stream_bytes
            .check_nonzero(max_output_bytes)?;

        let id = NodeId::new(name)?;
        Ok(Node {
            id: id.clone(),
            display_name: id.to_string(),
            needs: needs
                .into_iter()
                .map(NodeId::new)
                .collect::<WorkflowResult<_>>()?,
            condition: when
                .map(|condition| condition.into_condition(limits, 1))
                .transpose()?,
            execution,
            access,
            allow_failure,
            timeout_seconds,
            max_output_bytes,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RhoAccess {
    ReadOnly,
    Mutating,
}

impl RhoAccess {
    fn into_access(self) -> WorkflowResult<WorkspaceAccess> {
        Ok(match self {
            Self::ReadOnly => WorkspaceAccess::ReadOnly,
            Self::Mutating => WorkspaceAccess::Mutating,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RhoTemplate {
    Literal(String),
    Structured {
        #[serde(rename = "__rho_type")]
        kind: String,
        parts: Vec<RhoTemplatePart>,
    },
}

impl RhoTemplate {
    fn into_template(self) -> WorkflowResult<Template> {
        match self {
            Self::Literal(value) => Ok(Template(vec![TemplatePart::Literal { value }])),
            Self::Structured { kind, parts } => {
                if kind != "template" {
                    return Err(starlark("expected a string or template()"));
                }
                parts
                    .into_iter()
                    .map(RhoTemplatePart::into_part)
                    .collect::<WorkflowResult<Vec<_>>>()
                    .map(Template)
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RhoTemplatePart {
    Literal(String),
    Output(RhoOutputRef),
}

impl RhoTemplatePart {
    fn into_part(self) -> WorkflowResult<TemplatePart> {
        match self {
            Self::Literal(value) => Ok(TemplatePart::Literal { value }),
            Self::Output(reference) => Ok(TemplatePart::Output {
                reference: reference.into_reference()?,
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RhoOutputRef {
    #[serde(rename = "__rho_type")]
    kind: String,
    node: String,
    path: Vec<String>,
}

impl RhoOutputRef {
    fn into_reference(self) -> WorkflowResult<OutputReference> {
        if self.kind != "output_ref" {
            return Err(starlark(
                "template values must be strings or output references",
            ));
        }
        Ok(OutputReference {
            node: NodeId::new(self.node)?,
            path: OutputPath(self.path),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "__rho_type")]
enum RhoCondition {
    #[serde(rename = "equals")]
    Equals {
        reference: RhoConditionReference,
        value: serde_json::Value,
    },
    #[serde(rename = "is_one_of")]
    IsOneOf {
        reference: RhoConditionReference,
        values: Vec<serde_json::Value>,
    },
    #[serde(rename = "all")]
    All { conditions: Vec<RhoCondition> },
    #[serde(rename = "any")]
    Any { conditions: Vec<RhoCondition> },
    #[serde(rename = "not")]
    Not { condition: Box<RhoCondition> },
}

enum Comparison {
    Equals(serde_json::Value),
    IsOneOf(Vec<serde_json::Value>),
}

impl RhoCondition {
    fn into_condition(self, limits: &PlanningLimits, depth: u64) -> WorkflowResult<Condition> {
        limits.condition_depth.check(depth)?;
        match self {
            Self::Equals { reference, value } => {
                reference.into_predicate_condition(Comparison::Equals(value))
            }
            Self::IsOneOf { reference, values } => {
                reference.into_predicate_condition(Comparison::IsOneOf(values))
            }
            Self::All { conditions } => Ok(Condition::All {
                conditions: conditions
                    .into_iter()
                    .map(|condition| condition.into_condition(limits, depth + 1))
                    .collect::<WorkflowResult<_>>()?,
            }),
            Self::Any { conditions } => Ok(Condition::Any {
                conditions: conditions
                    .into_iter()
                    .map(|condition| condition.into_condition(limits, depth + 1))
                    .collect::<WorkflowResult<_>>()?,
            }),
            Self::Not { condition } => Ok(Condition::Not {
                condition: Box::new(condition.into_condition(limits, depth + 1)?),
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "__rho_type")]
enum RhoConditionReference {
    #[serde(rename = "output_ref")]
    Output { node: String, path: Vec<String> },
    #[serde(rename = "status_ref")]
    Status { node: String },
    #[serde(rename = "exit_code_ref")]
    ExitCode { node: String },
}

impl RhoConditionReference {
    fn into_predicate_condition(self, comparison: Comparison) -> WorkflowResult<Condition> {
        match self {
            Self::Output { node, path } => {
                let predicate = match comparison {
                    Comparison::Equals(value) => {
                        ValuePredicate::Equals(WorkflowValue::from_json(value)?)
                    }
                    Comparison::IsOneOf(values) => ValuePredicate::IsOneOf(
                        values
                            .into_iter()
                            .map(WorkflowValue::from_json)
                            .collect::<WorkflowResult<_>>()?,
                    ),
                };
                Ok(Condition::Output {
                    node: NodeId::new(node)?,
                    path: OutputPath(path),
                    predicate,
                })
            }
            Self::Status { node } => {
                let matches = match comparison {
                    Comparison::Equals(value) => vec![value],
                    Comparison::IsOneOf(values) => values,
                };
                Ok(Condition::NodeStatus {
                    node: NodeId::new(node)?,
                    matches: matches
                        .iter()
                        .map(parse_terminal_state)
                        .collect::<WorkflowResult<_>>()?,
                })
            }
            Self::ExitCode { node } => {
                let predicate = match comparison {
                    Comparison::Equals(value) => {
                        ExitCodePredicate::Equals(json_i32(&value, "value")?)
                    }
                    Comparison::IsOneOf(values) => ExitCodePredicate::IsOneOf(
                        values
                            .iter()
                            .map(|value| json_i32(value, "command exit condition"))
                            .collect::<WorkflowResult<_>>()?,
                    ),
                };
                Ok(Condition::CommandExit {
                    node: NodeId::new(node)?,
                    predicate,
                })
            }
        }
    }
}

fn parse_terminal_state(value: &serde_json::Value) -> WorkflowResult<NodeTerminalState> {
    NodeTerminalState::deserialize(value).map_err(|_| starlark("invalid terminal node state"))
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RhoOutputSpec {
    Tagged(RhoSchema),
}

impl RhoOutputSpec {
    fn into_schema(self, limits: &PlanningLimits) -> WorkflowResult<OutputSchema> {
        match self {
            Self::Tagged(RhoSchema::StdoutJson { schema }) => schema.into_schema(limits, 1),
            Self::Tagged(schema) => schema.into_schema(limits, 1),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "__rho_type")]
enum RhoSchema {
    #[serde(rename = "schema_null")]
    Null,
    #[serde(rename = "schema_bool")]
    Bool,
    #[serde(rename = "schema_integer")]
    Integer,
    #[serde(rename = "schema_string")]
    String,
    #[serde(rename = "schema_enum")]
    Enum { members: Vec<serde_json::Value> },
    #[serde(rename = "schema_list")]
    List { item: Box<RhoSchema> },
    #[serde(rename = "schema_record")]
    Record { fields: BTreeMap<String, RhoSchema> },
    #[serde(rename = "schema_optional")]
    Optional { schema: Box<RhoSchema> },
    #[serde(rename = "stdout_json")]
    StdoutJson { schema: Box<RhoSchema> },
}

impl RhoSchema {
    fn into_schema(self, limits: &PlanningLimits, depth: u64) -> WorkflowResult<OutputSchema> {
        limits.schema_depth.check(depth)?;
        Ok(match self {
            Self::Null => OutputSchema::Null,
            Self::Bool => OutputSchema::Bool,
            Self::Integer => OutputSchema::Integer,
            Self::String => OutputSchema::String,
            Self::Enum { members } => OutputSchema::Enum {
                members: members
                    .into_iter()
                    .map(WorkflowValue::from_json)
                    .collect::<WorkflowResult<_>>()?,
            },
            Self::List { item } => OutputSchema::List {
                item: Box::new(item.into_schema(limits, depth + 1)?),
            },
            Self::Record { fields } => OutputSchema::Object {
                fields: fields
                    .into_iter()
                    .map(|(name, value)| {
                        let (required, schema) = match value {
                            Self::Optional { schema } => {
                                (false, schema.into_schema(limits, depth + 1)?)
                            }
                            other => (true, other.into_schema(limits, depth + 1)?),
                        };
                        Ok((name, ObjectFieldSchema { schema, required }))
                    })
                    .collect::<WorkflowResult<_>>()?,
            },
            Self::Optional { .. } => {
                return Err(starlark(
                    "schema_optional is only valid inside schema_record fields",
                ))
            }
            Self::StdoutJson { .. } => {
                return Err(starlark(
                    "stdout_json is only valid as a node output wrapper",
                ))
            }
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "__rho_type")]
enum RhoInputSchema {
    #[serde(rename = "input_string")]
    String {
        #[serde(default)]
        default: Option<serde_json::Value>,
    },
    #[serde(rename = "input_integer")]
    Integer {
        #[serde(default)]
        default: Option<serde_json::Value>,
    },
    #[serde(rename = "input_bool")]
    Bool {
        #[serde(default)]
        default: Option<serde_json::Value>,
    },
    #[serde(rename = "input_enum")]
    Enum {
        members: Vec<serde_json::Value>,
        #[serde(default)]
        default: Option<serde_json::Value>,
    },
}

impl RhoInputSchema {
    fn into_schema(self) -> WorkflowResult<InputSchema> {
        Ok(match self {
            Self::String { default } => InputSchema::String {
                default: default
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| starlark("input_string default must be a string"))
                    })
                    .transpose()?,
            },
            Self::Integer { default } => InputSchema::Integer {
                default: default
                    .map(|value| {
                        value
                            .as_i64()
                            .ok_or_else(|| starlark("input_integer default must be an integer"))
                    })
                    .transpose()?,
            },
            Self::Bool { default } => InputSchema::Bool {
                default: default
                    .map(|value| {
                        value
                            .as_bool()
                            .ok_or_else(|| starlark("input_bool default must be a bool"))
                    })
                    .transpose()?,
            },
            Self::Enum { members, default } => {
                let members = members
                    .into_iter()
                    .map(WorkflowValue::from_json)
                    .collect::<WorkflowResult<BTreeSet<_>>>()?;
                if members.iter().any(|member| !member.scalar()) {
                    return Err(WorkflowError::Schema {
                        path: "$input.enum".to_owned(),
                        reason: "enum members must be scalar".to_owned(),
                    });
                }
                let default = default.map(WorkflowValue::from_json).transpose()?;
                InputSchema::Enum { members, default }
            }
        })
    }
}

fn json_i32(value: &serde_json::Value, field: &str) -> WorkflowResult<i32> {
    value
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| starlark(format!("'{field}' must be a 32-bit integer")))
}
