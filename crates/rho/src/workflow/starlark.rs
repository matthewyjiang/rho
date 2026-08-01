use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
    starlark_api, CollectedSources, InputName, InputSchema, PlanningLimits, WorkflowError,
    WorkflowGraph, WorkflowResult, WorkflowValue,
};

#[path = "starlark_parse.rs"]
mod starlark_parse;

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
            let input_value = alloc_workflow_map(&module.heap(), &inputs);
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
    heap: &Heap<'v>,
    values: &BTreeMap<InputName, WorkflowValue>,
) -> Value<'v> {
    heap.alloc(AllocDict(values.iter().map(|(key, value)| {
        (heap.alloc(key.as_str()), alloc_workflow_value(heap, value))
    })))
}

fn alloc_workflow_value<'v>(heap: &Heap<'v>, value: &WorkflowValue) -> Value<'v> {
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
    starlark_parse::parse_input_schema(value)
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
    starlark_parse::parse_graph(value, limits)
}

#[cfg(test)]
#[path = "starlark_tests.rs"]
mod tests;
