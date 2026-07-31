use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use starlark::{
    environment::{FrozenModule, Module},
    eval::{Evaluator, ReturnFileLoader},
    syntax::{AstModule, Dialect},
    values::{
        dict::{AllocDict, DictRef},
        list::AllocList,
        Heap, Value,
    },
};

use super::{
    starlark_api, AgentNode, CollectedSources, CommandNode, Condition, ExitCodePredicate,
    InputName, InputSchema, Node, NodeExecution, NodeId, ObjectFieldSchema, OutputPath,
    OutputReference, OutputSchema, PlanningLimits, Template, TemplatePart, ValuePredicate,
    WorkflowError, WorkflowGraph, WorkflowName, WorkflowResult, WorkflowValue, WorkspaceAccess,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(test)]
pub(crate) enum IsolationDecision {
    /// The interpreter documents heap and tick limits as best effort. A
    /// supervised process must enforce memory and wall-time limits for shipped use.
    SupervisedProcessRequired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlannedSource {
    pub(crate) graph: WorkflowGraph,
    pub(crate) inputs: BTreeMap<InputName, WorkflowValue>,
    pub(crate) ticks: u64,
    pub(crate) peak_heap_bytes: u64,
}

pub(crate) struct StarlarkPlanner<'a> {
    limits: &'a PlanningLimits,
}

impl<'a> StarlarkPlanner<'a> {
    pub(crate) fn new(limits: &'a PlanningLimits) -> Self {
        Self { limits }
    }

    #[cfg(test)]
    pub(crate) fn isolation_decision(&self) -> IsolationDecision {
        IsolationDecision::SupervisedProcessRequired
    }

    /// Evaluate collected bytes for safety tests only. Product callers must run
    /// this method inside a supervised worker because starlark-rust's own heap
    /// and tick checks do not claim a hard hostile-input bound.
    pub(crate) fn plan_in_process_prototype(
        &self,
        collected: &CollectedSources,
        supplied_inputs: &BTreeMap<InputName, WorkflowValue>,
        cancelled: Arc<AtomicBool>,
    ) -> WorkflowResult<PlannedSource> {
        let globals = starlark_api::globals();
        let mut frozen = HashMap::<String, FrozenModule>::new();
        self.evaluate_dependencies(
            &collected.entry_label,
            collected,
            &globals,
            &mut frozen,
            &cancelled,
        )?;
        let source = collected
            .sources
            .get(&collected.entry_label)
            .ok_or_else(|| WorkflowError::Starlark("entry source was not collected".to_owned()))?;
        let ast = AstModule::parse(&collected.entry_label, source.clone(), &Dialect::Standard)
            .map_err(|error| WorkflowError::Starlark(error.to_string()))?;
        Module::with_temp_heap(|module| {
            let refs = frozen
                .iter()
                .map(|(key, value)| (key.as_str(), value))
                .collect();
            let loader = ReturnFileLoader { modules: &refs };
            let mut eval = Evaluator::new(&module);
            configure_evaluator(&mut eval, self.limits, cancelled.clone())?;
            eval.set_loader(&loader);
            eval.eval_module(ast, &globals).map_err(starlark_error)?;
            let definition = module
                .get("WORKFLOW")
                .ok_or(WorkflowError::MissingWorkflow)?;
            let definition = DictRef::from_value(definition).ok_or_else(|| {
                WorkflowError::Starlark(
                    "WORKFLOW must be the value returned by define()".to_owned(),
                )
            })?;
            let input_specs = definition
                .get_str("inputs")
                .ok_or(WorkflowError::MissingWorkflow)?;
            let build = definition
                .get_str("build")
                .ok_or(WorkflowError::MissingWorkflow)?;
            let schemas = parse_input_schemas(input_specs)?;
            let inputs = validate_inputs(&schemas, supplied_inputs)?;
            check_input_limits(&inputs, self.limits)?;
            let input_value = alloc_workflow_map(module.heap(), &inputs);
            let built = eval
                .eval_function(build, &[input_value], &[])
                .map_err(starlark_error)?;
            let json = built
                .to_json_value()
                .map_err(|error| WorkflowError::Starlark(error.to_string()))?;
            check_collection_limits(&json, self.limits, 1)?;
            let graph = parse_graph(json, self.limits)?;
            let ticks = eval.get_total_tick_count();
            self.limits.evaluator_ticks.check(ticks)?;
            let peak_heap_bytes = module.heap().peak_allocated_bytes() as u64;
            self.limits.evaluator_heap_bytes.check(peak_heap_bytes)?;
            Ok(PlannedSource {
                graph,
                inputs,
                ticks,
                peak_heap_bytes,
            })
        })
    }

    fn evaluate_dependencies(
        &self,
        entry: &str,
        collected: &CollectedSources,
        globals: &starlark::environment::Globals,
        frozen: &mut HashMap<String, FrozenModule>,
        cancelled: &Arc<AtomicBool>,
    ) -> WorkflowResult<()> {
        fn visit(
            planner: &StarlarkPlanner<'_>,
            label: &str,
            collected: &CollectedSources,
            globals: &starlark::environment::Globals,
            frozen: &mut HashMap<String, FrozenModule>,
            in_progress: &mut HashSet<String>,
            cancelled: &Arc<AtomicBool>,
        ) -> WorkflowResult<()> {
            if frozen.contains_key(label) {
                return Ok(());
            }
            if !in_progress.insert(label.to_owned()) {
                return Err(WorkflowError::ImportCycle {
                    chain: label.to_owned(),
                });
            }
            let source =
                collected
                    .sources
                    .get(label)
                    .ok_or_else(|| WorkflowError::InvalidModuleLabel {
                        label: label.to_owned(),
                        reason: "loaded module is missing from the collected source set".to_owned(),
                    })?;
            let ast = AstModule::parse(label, source.clone(), &Dialect::Standard)
                .map_err(|error| WorkflowError::Starlark(error.to_string()))?;
            for load in ast.loads() {
                visit(
                    planner,
                    load.module_id,
                    collected,
                    globals,
                    frozen,
                    in_progress,
                    cancelled,
                )?;
            }
            let refs = frozen
                .iter()
                .map(|(key, value)| (key.as_str(), value))
                .collect();
            let loader = ReturnFileLoader { modules: &refs };
            let result = Module::with_temp_heap(|module| {
                {
                    let mut eval = Evaluator::new(&module);
                    configure_evaluator(&mut eval, planner.limits, cancelled.clone())?;
                    eval.set_loader(&loader);
                    eval.eval_module(ast, globals).map_err(starlark_error)?;
                }
                module
                    .freeze()
                    .map_err(|error| WorkflowError::Starlark(format!("{error:?}")))
            })?;
            frozen.insert(label.to_owned(), result);
            in_progress.remove(label);
            Ok(())
        }
        let entry_source =
            collected
                .sources
                .get(entry)
                .ok_or_else(|| WorkflowError::InvalidModuleLabel {
                    label: entry.to_owned(),
                    reason: "entry module is missing from the collected source set".to_owned(),
                })?;
        let ast = AstModule::parse(entry, entry_source.clone(), &Dialect::Standard)
            .map_err(|error| WorkflowError::Starlark(error.to_string()))?;
        let mut in_progress = HashSet::from([entry.to_owned()]);
        for load in ast.loads() {
            visit(
                self,
                load.module_id,
                collected,
                globals,
                frozen,
                &mut in_progress,
                cancelled,
            )?;
        }
        in_progress.remove(entry);
        Ok(())
    }
}

fn check_collection_limits(
    value: &serde_json::Value,
    limits: &PlanningLimits,
    depth: u64,
) -> WorkflowResult<()> {
    limits.input_depth.check(depth)?;
    match value {
        serde_json::Value::String(value) => limits.string_bytes.check(value.len() as u64),
        serde_json::Value::Array(values) => {
            limits.list_items.check(values.len() as u64)?;
            for value in values {
                check_collection_limits(value, limits, depth + 1)?;
            }
            Ok(())
        }
        serde_json::Value::Object(values) => {
            limits.dict_items.check(values.len() as u64)?;
            for (key, value) in values {
                limits.string_bytes.check(key.len() as u64)?;
                check_collection_limits(value, limits, depth + 1)?;
            }
            Ok(())
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            Ok(())
        }
    }
}

fn configure_evaluator<'v>(
    eval: &mut Evaluator<'v, '_, '_>,
    limits: &PlanningLimits,
    cancelled: Arc<AtomicBool>,
) -> WorkflowResult<()> {
    eval.set_max_tick_count(limits.evaluator_ticks.limit)
        .map_err(|error| WorkflowError::Starlark(error.to_string()))?;
    eval.set_max_heap_size(
        usize::try_from(limits.evaluator_heap_bytes.limit).map_err(|_| {
            WorkflowError::BudgetExceeded {
                budget: limits.evaluator_heap_bytes.name,
                limit: usize::MAX as u64,
                actual: limits.evaluator_heap_bytes.limit,
            }
        })?,
    )
    .map_err(|error| WorkflowError::Starlark(error.to_string()))?;
    eval.set_max_callstack_size(usize::try_from(limits.call_stack_depth.limit).map_err(|_| {
        WorkflowError::BudgetExceeded {
            budget: limits.call_stack_depth.name,
            limit: usize::MAX as u64,
            actual: limits.call_stack_depth.limit,
        }
    })?)
    .map_err(|error| WorkflowError::Starlark(error.to_string()))?;
    eval.set_check_cancelled(Box::new(move || cancelled.load(Ordering::Acquire)));
    Ok(())
}

fn starlark_error(error: impl std::fmt::Display) -> WorkflowError {
    WorkflowError::Starlark(error.to_string())
}

fn alloc_workflow_map<'v>(
    heap: Heap<'v>,
    values: &BTreeMap<InputName, WorkflowValue>,
) -> Value<'v> {
    heap.alloc(AllocDict(values.iter().map(|(key, value)| {
        (heap.alloc(key.as_str()), alloc_workflow_value(heap, value))
    })))
}

fn alloc_workflow_value<'v>(heap: Heap<'v>, value: &WorkflowValue) -> Value<'v> {
    match value {
        WorkflowValue::Null => Value::new_none(),
        WorkflowValue::Bool(value) => heap.alloc(*value),
        WorkflowValue::Integer(value) => heap.alloc(*value),
        WorkflowValue::String(value) => heap.alloc(value.as_str()),
        WorkflowValue::List(values) => heap.alloc(AllocList(
            values.iter().map(|value| alloc_workflow_value(heap, value)),
        )),
        WorkflowValue::Object(values) => {
            heap.alloc(AllocDict(values.iter().map(|(key, value)| {
                (heap.alloc(key.as_str()), alloc_workflow_value(heap, value))
            })))
        }
    }
}

fn parse_input_schemas(value: Value<'_>) -> WorkflowResult<BTreeMap<InputName, InputSchema>> {
    let json = value.to_json_value().map_err(starlark_error)?;
    let object = json.as_object().ok_or_else(|| {
        WorkflowError::Starlark("define(inputs=...) must receive a string-keyed dict".to_owned())
    })?;
    object
        .iter()
        .map(|(name, value)| Ok((InputName::new(name)?, parse_input_schema(value)?)))
        .collect()
}

fn parse_input_schema(value: &serde_json::Value) -> WorkflowResult<InputSchema> {
    let kind = field_str(value, "__rho_type")?;
    let default = value
        .get("default")
        .filter(|value| !value.is_null())
        .cloned();
    Ok(match kind {
        "input_string" => InputSchema::String {
            default: default
                .map(|value| {
                    value.as_str().map(str::to_owned).ok_or_else(|| {
                        WorkflowError::Starlark("input_string default must be a string".to_owned())
                    })
                })
                .transpose()?,
        },
        "input_integer" => InputSchema::Integer {
            default: default
                .map(|value| {
                    value.as_i64().ok_or_else(|| {
                        WorkflowError::Starlark(
                            "input_integer default must be an integer".to_owned(),
                        )
                    })
                })
                .transpose()?,
        },
        "input_bool" => InputSchema::Bool {
            default: default
                .map(|value| {
                    value.as_bool().ok_or_else(|| {
                        WorkflowError::Starlark("input_bool default must be a bool".to_owned())
                    })
                })
                .transpose()?,
        },
        "input_enum" => {
            let members = array(value, "members")?
                .iter()
                .cloned()
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
        other => {
            return Err(WorkflowError::Starlark(format!(
                "unsupported input schema '{other}'"
            )))
        }
    })
}

fn validate_inputs(
    schemas: &BTreeMap<InputName, InputSchema>,
    supplied: &BTreeMap<InputName, WorkflowValue>,
) -> WorkflowResult<BTreeMap<InputName, WorkflowValue>> {
    if let Some(name) = supplied.keys().find(|name| !schemas.contains_key(*name)) {
        return Err(WorkflowError::UnknownInput(name.to_string()));
    }
    schemas
        .iter()
        .map(|(name, schema)| {
            let value = supplied
                .get(name)
                .cloned()
                .or_else(|| schema.default_value())
                .ok_or_else(|| WorkflowError::MissingInput(name.to_string()))?;
            if !schema.validate(&value) {
                return Err(WorkflowError::InvalidInput {
                    name: name.to_string(),
                    reason: format!("expected declared input type, got {}", value.kind()),
                });
            }
            Ok((name.clone(), value))
        })
        .collect()
}

fn check_input_limits(
    inputs: &BTreeMap<InputName, WorkflowValue>,
    limits: &PlanningLimits,
) -> WorkflowResult<()> {
    let json = serde_json::to_value(inputs)?;
    let bytes = serde_json::to_vec(&json)?.len() as u64;
    limits.input_bytes.check(bytes)?;
    check_collection_limits(&json, limits, 1)?;
    fn depth(value: &WorkflowValue) -> u64 {
        match value {
            WorkflowValue::List(values) => 1 + values.iter().map(depth).max().unwrap_or(0),
            WorkflowValue::Object(values) => 1 + values.values().map(depth).max().unwrap_or(0),
            _ => 1,
        }
    }
    limits
        .input_depth
        .check(inputs.values().map(depth).max().unwrap_or(1))
}

fn parse_graph(value: serde_json::Value, limits: &PlanningLimits) -> WorkflowResult<WorkflowGraph> {
    if field_str(&value, "__rho_type")? != "workflow" {
        return Err(WorkflowError::Starlark(
            "build() must return workflow()".to_owned(),
        ));
    }
    let name = WorkflowName::new(field_str(&value, "name")?)?;
    let nodes_json = array(&value, "nodes")?;
    limits.node_count.check(nodes_json.len() as u64)?;
    let mut nodes = BTreeMap::new();
    let mut edges = 0_u64;
    for value in nodes_json {
        let node = parse_node(value, limits)?;
        edges = edges.saturating_add(node.needs.len() as u64);
        if nodes.insert(node.id.clone(), node).is_some() {
            return Err(WorkflowError::Starlark(
                "workflow contains duplicate node IDs".to_owned(),
            ));
        }
    }
    limits.edge_count.check(edges)?;
    let graph = WorkflowGraph { name, nodes };
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

fn parse_node(value: &serde_json::Value, limits: &PlanningLimits) -> WorkflowResult<Node> {
    let kind = field_str(value, "__rho_type")?;
    let id = NodeId::new(field_str(value, "name")?)?;
    let needs = optional_array(value, "needs")?
        .iter()
        .map(|value| {
            NodeId::new(value.as_str().ok_or_else(|| {
                WorkflowError::Starlark("needs entries must be strings".to_owned())
            })?)
        })
        .collect::<WorkflowResult<Vec<_>>>()?;
    let condition = value
        .get("when")
        .filter(|value| !value.is_null())
        .map(|value| parse_condition(value, limits, 1))
        .transpose()?;
    let allow_failure = value
        .get("allow_failure")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let timeout_seconds = required_u64(value, "timeout_seconds")?;
    let max_output_bytes = required_u64(value, "max_output_bytes")?;
    let (execution, access) = match kind {
        "agent" => (
            NodeExecution::Agent(AgentNode {
                agent: field_str(value, "agent")?.to_owned(),
                prompt: parse_template(value.get("prompt").ok_or_else(|| missing("prompt"))?)?,
                output: value
                    .get("output")
                    .filter(|value| !value.is_null())
                    .map(|value| parse_output_wrapper(value, limits))
                    .transpose()?,
            }),
            match field_str(value, "access")? {
                "read_only" => WorkspaceAccess::ReadOnly,
                "mutating" => WorkspaceAccess::Mutating,
                other => {
                    return Err(WorkflowError::Starlark(format!(
                        "unknown access mode '{other}'"
                    )))
                }
            },
        ),
        "command" => {
            let argv = array(value, "argv")?;
            let executable = argv
                .first()
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    WorkflowError::Starlark(
                        "command argv must start with a static executable string".to_owned(),
                    )
                })?;
            let arguments = argv[1..]
                .iter()
                .map(parse_template)
                .collect::<WorkflowResult<_>>()?;
            (
                NodeExecution::Command(CommandNode::Direct {
                    executable: executable.to_owned(),
                    arguments,
                    cwd: field_str(value, "cwd")?.to_owned(),
                    output: value
                        .get("output")
                        .filter(|value| !value.is_null())
                        .map(|value| parse_output_wrapper(value, limits))
                        .transpose()?,
                }),
                WorkspaceAccess::Mutating,
            )
        }
        "shell" => (
            NodeExecution::Command(CommandNode::Shell {
                executable: field_str(value, "executable")?.to_owned(),
                arguments: array(value, "arguments")?
                    .iter()
                    .map(|value| {
                        value.as_str().map(str::to_owned).ok_or_else(|| {
                            WorkflowError::Starlark(
                                "shell arguments must be static strings".to_owned(),
                            )
                        })
                    })
                    .collect::<WorkflowResult<_>>()?,
                command: field_str(value, "command")?.to_owned(),
                cwd: field_str(value, "cwd")?.to_owned(),
                output: value
                    .get("output")
                    .filter(|value| !value.is_null())
                    .map(|value| parse_output_wrapper(value, limits))
                    .transpose()?,
            }),
            WorkspaceAccess::Mutating,
        ),
        other => {
            return Err(WorkflowError::Starlark(format!(
                "unknown node kind '{other}'"
            )))
        }
    };
    Ok(Node {
        id: id.clone(),
        display_name: id.to_string(),
        needs,
        condition,
        execution,
        access,
        allow_failure,
        timeout_seconds,
        max_output_bytes,
    })
}

fn parse_template(value: &serde_json::Value) -> WorkflowResult<Template> {
    let parts = if value.as_str().is_some() {
        std::slice::from_ref(value)
    } else if field_str(value, "__rho_type")? == "template" {
        array(value, "parts")?
    } else {
        return Err(WorkflowError::Starlark(
            "expected a string or template()".to_owned(),
        ));
    };
    parts
        .iter()
        .map(|part| {
            if let Some(value) = part.as_str() {
                Ok(TemplatePart::Literal {
                    value: value.to_owned(),
                })
            } else {
                Ok(TemplatePart::Output {
                    reference: parse_output_reference(part)?,
                })
            }
        })
        .collect::<WorkflowResult<Vec<_>>>()
        .map(Template)
}

fn parse_output_reference(value: &serde_json::Value) -> WorkflowResult<OutputReference> {
    if field_str(value, "__rho_type")? != "output_ref" {
        return Err(WorkflowError::Starlark(
            "template values must be strings or output references".to_owned(),
        ));
    }
    Ok(OutputReference {
        node: NodeId::new(field_str(value, "node")?)?,
        path: OutputPath(
            array(value, "path")?
                .iter()
                .map(|value| {
                    value.as_str().map(str::to_owned).ok_or_else(|| {
                        WorkflowError::Starlark("output path entries must be strings".to_owned())
                    })
                })
                .collect::<WorkflowResult<_>>()?,
        ),
    })
}

fn parse_condition(
    value: &serde_json::Value,
    limits: &PlanningLimits,
    depth: u64,
) -> WorkflowResult<Condition> {
    limits.condition_depth.check(depth)?;
    match field_str(value, "__rho_type")? {
        "equals" | "is_one_of" => {
            let reference = value.get("reference").ok_or_else(|| missing("reference"))?;
            match field_str(reference, "__rho_type")? {
                "output_ref" => {
                    let reference = parse_output_reference(reference)?;
                    let predicate = if field_str(value, "__rho_type")? == "equals" {
                        ValuePredicate::Equals(WorkflowValue::from_json(value["value"].clone())?)
                    } else {
                        ValuePredicate::IsOneOf(
                            array(value, "values")?
                                .iter()
                                .cloned()
                                .map(WorkflowValue::from_json)
                                .collect::<WorkflowResult<_>>()?,
                        )
                    };
                    Ok(Condition::Output {
                        node: reference.node,
                        path: reference.path,
                        predicate,
                    })
                }
                "status_ref" => {
                    let matches = if field_str(value, "__rho_type")? == "equals" {
                        vec![&value["value"]]
                    } else {
                        array(value, "values")?.iter().collect()
                    };
                    Ok(Condition::NodeStatus {
                        node: NodeId::new(field_str(reference, "node")?)?,
                        matches: matches
                            .into_iter()
                            .map(parse_terminal_state)
                            .collect::<WorkflowResult<_>>()?,
                    })
                }
                "exit_code_ref" => {
                    let predicate = if field_str(value, "__rho_type")? == "equals" {
                        ExitCodePredicate::Equals(required_i32(value, "value")?)
                    } else {
                        ExitCodePredicate::IsOneOf(
                            array(value, "values")?
                                .iter()
                                .map(|value| json_i32(value, "command exit condition"))
                                .collect::<WorkflowResult<_>>()?,
                        )
                    };
                    Ok(Condition::CommandExit {
                        node: NodeId::new(field_str(reference, "node")?)?,
                        predicate,
                    })
                }
                other => Err(WorkflowError::Starlark(format!(
                    "unsupported condition reference '{other}'"
                ))),
            }
        }
        "all" => Ok(Condition::All {
            conditions: array(value, "conditions")?
                .iter()
                .map(|condition| parse_condition(condition, limits, depth + 1))
                .collect::<WorkflowResult<_>>()?,
        }),
        "any" => Ok(Condition::Any {
            conditions: array(value, "conditions")?
                .iter()
                .map(|condition| parse_condition(condition, limits, depth + 1))
                .collect::<WorkflowResult<_>>()?,
        }),
        "not" => Ok(Condition::Not {
            condition: Box::new(parse_condition(&value["condition"], limits, depth + 1)?),
        }),
        other => Err(WorkflowError::Starlark(format!(
            "unsupported condition '{other}'"
        ))),
    }
}

fn parse_terminal_state(value: &serde_json::Value) -> WorkflowResult<super::NodeTerminalState> {
    match value.as_str() {
        Some("success") => Ok(super::NodeTerminalState::Success),
        Some("failure") => Ok(super::NodeTerminalState::Failure),
        Some("denial") => Ok(super::NodeTerminalState::Denial),
        Some("cancellation") => Ok(super::NodeTerminalState::Cancellation),
        Some("skipped") => Ok(super::NodeTerminalState::Skipped),
        Some("blocked") => Ok(super::NodeTerminalState::Blocked),
        _ => Err(WorkflowError::Starlark(
            "invalid terminal node state".to_owned(),
        )),
    }
}

fn parse_output_wrapper(
    value: &serde_json::Value,
    limits: &PlanningLimits,
) -> WorkflowResult<OutputSchema> {
    if field_str(value, "__rho_type")? == "stdout_json" {
        parse_schema(&value["schema"], limits, 1)
    } else {
        parse_schema(value, limits, 1)
    }
}

fn parse_schema(
    value: &serde_json::Value,
    limits: &PlanningLimits,
    depth: u64,
) -> WorkflowResult<OutputSchema> {
    limits.schema_depth.check(depth)?;
    Ok(match field_str(value, "__rho_type")? {
        "schema_null" => OutputSchema::Null,
        "schema_bool" => OutputSchema::Bool,
        "schema_integer" => OutputSchema::Integer,
        "schema_string" => OutputSchema::String,
        "schema_enum" => OutputSchema::Enum {
            members: array(value, "members")?
                .iter()
                .cloned()
                .map(WorkflowValue::from_json)
                .collect::<WorkflowResult<_>>()?,
        },
        "schema_list" => OutputSchema::List {
            item: Box::new(parse_schema(&value["item"], limits, depth + 1)?),
        },
        "schema_record" => {
            let fields = value["fields"].as_object().ok_or_else(|| {
                WorkflowError::Starlark("record fields must be a dict".to_owned())
            })?;
            OutputSchema::Object {
                fields: fields
                    .iter()
                    .map(|(name, value)| {
                        let (required, schema) =
                            if field_str(value, "__rho_type").ok() == Some("schema_optional") {
                                (false, parse_schema(&value["schema"], limits, depth + 1)?)
                            } else {
                                (true, parse_schema(value, limits, depth + 1)?)
                            };
                        Ok((name.clone(), ObjectFieldSchema { schema, required }))
                    })
                    .collect::<WorkflowResult<_>>()?,
            }
        }
        other => {
            return Err(WorkflowError::Starlark(format!(
                "unsupported output schema '{other}'"
            )))
        }
    })
}

fn field_str<'a>(value: &'a serde_json::Value, field: &str) -> WorkflowResult<&'a str> {
    value
        .get(field)
        .and_then(|value| value.as_str())
        .ok_or_else(|| missing(field))
}
fn array<'a>(value: &'a serde_json::Value, field: &str) -> WorkflowResult<&'a [serde_json::Value]> {
    value
        .get(field)
        .and_then(|value| value.as_array())
        .map(Vec::as_slice)
        .ok_or_else(|| missing(field))
}
fn optional_array<'a>(
    value: &'a serde_json::Value,
    field: &str,
) -> WorkflowResult<&'a [serde_json::Value]> {
    match value.get(field) {
        None | Some(serde_json::Value::Null) => Ok(&[]),
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| missing(field)),
    }
}
fn required_u64(value: &serde_json::Value, field: &str) -> WorkflowResult<u64> {
    value.get(field).and_then(|value| value.as_u64()).ok_or_else(|| WorkflowError::Starlark(format!("node field '{field}' must be set to a non-negative integer; no unmeasured default is enabled")))
}

fn required_i32(value: &serde_json::Value, field: &str) -> WorkflowResult<i32> {
    json_i32(value.get(field).ok_or_else(|| missing(field))?, field)
}

fn json_i32(value: &serde_json::Value, field: &str) -> WorkflowResult<i32> {
    value
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| WorkflowError::Starlark(format!("'{field}' must be a 32-bit integer")))
}
fn missing(field: &str) -> WorkflowError {
    WorkflowError::Starlark(format!("missing or invalid field '{field}'"))
}

#[cfg(test)]
#[path = "starlark_tests.rs"]
mod tests;
