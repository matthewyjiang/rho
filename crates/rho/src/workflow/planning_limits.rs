use serde::{Deserialize, Serialize};

use super::{WorkflowError, WorkflowResult};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Budget {
    pub(crate) name: &'static str,
    pub(crate) limit: u64,
}

impl Budget {
    pub(crate) fn new(name: &'static str, limit: u64) -> WorkflowResult<Self> {
        if limit == 0 {
            return Err(WorkflowError::BudgetExceeded {
                budget: name,
                limit,
                actual: 1,
            });
        }
        Ok(Self { name, limit })
    }

    pub(crate) fn check(&self, actual: u64) -> WorkflowResult<()> {
        if actual > self.limit {
            Err(WorkflowError::BudgetExceeded {
                budget: self.name,
                limit: self.limit,
                actual,
            })
        } else {
            Ok(())
        }
    }

    pub(crate) fn check_nonzero(&self, actual: u64) -> WorkflowResult<()> {
        if actual == 0 || actual > self.limit {
            Err(WorkflowError::BudgetExceeded {
                budget: self.name,
                limit: self.limit,
                actual,
            })
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlanningLimitValues {
    pub(crate) total_source_bytes: u64,
    pub(crate) module_count: u64,
    pub(crate) module_depth: u64,
    pub(crate) evaluator_ticks: u64,
    pub(crate) evaluator_heap_bytes: u64,
    pub(crate) call_stack_depth: u64,
    pub(crate) string_bytes: u64,
    pub(crate) list_items: u64,
    pub(crate) dict_items: u64,
    pub(crate) input_depth: u64,
    pub(crate) input_bytes: u64,
    pub(crate) node_count: u64,
    pub(crate) edge_count: u64,
    pub(crate) condition_depth: u64,
    pub(crate) schema_depth: u64,
    pub(crate) schema_bytes: u64,
    pub(crate) graph_bytes: u64,
    pub(crate) worker_wall_millis: u64,
    pub(crate) retained_output_per_stream_bytes: u64,
    pub(crate) retained_output_total_bytes: u64,
    pub(crate) rendered_template_bytes: u64,
    pub(crate) node_timeout_seconds: u64,
    pub(crate) prompt_expansion_bytes: u64,
    pub(crate) argv_expansion_bytes: u64,
    pub(crate) environment_expansion_bytes: u64,
}

impl PlanningLimitValues {
    /// Workflow planning policy, including the limits frozen for runtime execution.
    pub(crate) const STANDARD: Self = Self {
        total_source_bytes: 1_000_000,
        module_count: 100,
        module_depth: 20,
        evaluator_ticks: 1_000_000,
        evaluator_heap_bytes: 80_796_392,
        call_stack_depth: 25,
        string_bytes: 1_000_000,
        list_items: 10_000,
        dict_items: 10_000,
        input_depth: 20,
        input_bytes: 1_000_000,
        node_count: 1_000,
        edge_count: 10_000,
        condition_depth: 2,
        schema_depth: 6,
        schema_bytes: 1_000_000,
        graph_bytes: 10_000_000,
        worker_wall_millis: 10_000,
        retained_output_per_stream_bytes: 8 * 1024 * 1024,
        retained_output_total_bytes: 64 * 1024 * 1024,
        rendered_template_bytes: 4 * 1024 * 1024,
        node_timeout_seconds: 24 * 60 * 60,
        prompt_expansion_bytes: 8 * 1024 * 1024,
        argv_expansion_bytes: 8 * 1024 * 1024,
        // Schema v1 forbids command environment entries; one byte is the budget floor.
        environment_expansion_bytes: 1,
    };
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PlanningLimits {
    pub(crate) total_source_bytes: Budget,
    pub(crate) module_count: Budget,
    pub(crate) module_depth: Budget,
    pub(crate) evaluator_ticks: Budget,
    pub(crate) evaluator_heap_bytes: Budget,
    pub(crate) call_stack_depth: Budget,
    pub(crate) string_bytes: Budget,
    pub(crate) list_items: Budget,
    pub(crate) dict_items: Budget,
    pub(crate) input_depth: Budget,
    pub(crate) input_bytes: Budget,
    pub(crate) node_count: Budget,
    pub(crate) edge_count: Budget,
    pub(crate) condition_depth: Budget,
    pub(crate) schema_depth: Budget,
    pub(crate) schema_bytes: Budget,
    pub(crate) graph_bytes: Budget,
    pub(crate) worker_wall_millis: Budget,
    pub(crate) retained_output_per_stream_bytes: Budget,
    pub(crate) retained_output_total_bytes: Budget,
    pub(crate) rendered_template_bytes: Budget,
    pub(crate) node_timeout_seconds: Budget,
    pub(crate) prompt_expansion_bytes: Budget,
    pub(crate) argv_expansion_bytes: Budget,
    pub(crate) environment_expansion_bytes: Budget,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FrozenRuntimeLimits {
    pub(crate) retained_output_per_stream_bytes: u64,
    pub(crate) retained_output_total_bytes: u64,
    pub(crate) rendered_template_bytes: u64,
    pub(crate) node_timeout_seconds: u64,
    pub(crate) prompt_expansion_bytes: u64,
    pub(crate) argv_expansion_bytes: u64,
    pub(crate) environment_expansion_bytes: u64,
}

impl FrozenRuntimeLimits {
    pub(crate) fn validate(&self) -> WorkflowResult<()> {
        for (name, limit) in [
            (
                "retained output per stream bytes",
                self.retained_output_per_stream_bytes,
            ),
            (
                "retained output total bytes",
                self.retained_output_total_bytes,
            ),
            ("rendered template bytes", self.rendered_template_bytes),
            ("node timeout seconds", self.node_timeout_seconds),
            ("prompt expansion bytes", self.prompt_expansion_bytes),
            ("argv expansion bytes", self.argv_expansion_bytes),
            (
                "environment expansion bytes",
                self.environment_expansion_bytes,
            ),
        ] {
            if limit == 0 {
                return Err(WorkflowError::BudgetExceeded {
                    budget: name,
                    limit,
                    actual: 1,
                });
            }
        }
        Ok(())
    }
}

impl PlanningLimits {
    pub(crate) fn frozen_runtime_limits(&self) -> FrozenRuntimeLimits {
        FrozenRuntimeLimits {
            retained_output_per_stream_bytes: self.retained_output_per_stream_bytes.limit,
            retained_output_total_bytes: self.retained_output_total_bytes.limit,
            rendered_template_bytes: self.rendered_template_bytes.limit,
            node_timeout_seconds: self.node_timeout_seconds.limit,
            prompt_expansion_bytes: self.prompt_expansion_bytes.limit,
            argv_expansion_bytes: self.argv_expansion_bytes.limit,
            environment_expansion_bytes: self.environment_expansion_bytes.limit,
        }
    }

    /// Validate the policy values and attach the budget names used in errors.
    pub(crate) fn from_values(values: PlanningLimitValues) -> WorkflowResult<Self> {
        Ok(Self {
            total_source_bytes: Budget::new("total source bytes", values.total_source_bytes)?,
            module_count: Budget::new("module count", values.module_count)?,
            module_depth: Budget::new("module depth", values.module_depth)?,
            evaluator_ticks: Budget::new("Starlark evaluator ticks", values.evaluator_ticks)?,
            evaluator_heap_bytes: Budget::new(
                "Starlark evaluator heap bytes",
                values.evaluator_heap_bytes,
            )?,
            call_stack_depth: Budget::new("Starlark call-stack depth", values.call_stack_depth)?,
            string_bytes: Budget::new("Starlark string bytes", values.string_bytes)?,
            list_items: Budget::new("Starlark list items", values.list_items)?,
            dict_items: Budget::new("Starlark dict items", values.dict_items)?,
            input_depth: Budget::new("input depth", values.input_depth)?,
            input_bytes: Budget::new("input bytes", values.input_bytes)?,
            node_count: Budget::new("node count", values.node_count)?,
            edge_count: Budget::new("edge count", values.edge_count)?,
            condition_depth: Budget::new("condition depth", values.condition_depth)?,
            schema_depth: Budget::new("output schema depth", values.schema_depth)?,
            schema_bytes: Budget::new("output schema bytes", values.schema_bytes)?,
            graph_bytes: Budget::new("serialized graph bytes", values.graph_bytes)?,
            worker_wall_millis: Budget::new(
                "planning worker wall milliseconds",
                values.worker_wall_millis,
            )?,
            retained_output_per_stream_bytes: Budget::new(
                "retained output bytes per stream",
                values.retained_output_per_stream_bytes,
            )?,
            retained_output_total_bytes: Budget::new(
                "total retained workflow output bytes",
                values.retained_output_total_bytes,
            )?,
            rendered_template_bytes: Budget::new(
                "rendered template bytes",
                values.rendered_template_bytes,
            )?,
            node_timeout_seconds: Budget::new("node timeout seconds", values.node_timeout_seconds)?,
            prompt_expansion_bytes: Budget::new(
                "expanded prompt bytes",
                values.prompt_expansion_bytes,
            )?,
            argv_expansion_bytes: Budget::new("expanded argv bytes", values.argv_expansion_bytes)?,
            environment_expansion_bytes: Budget::new(
                "expanded environment bytes",
                values.environment_expansion_bytes,
            )?,
        })
    }
}
