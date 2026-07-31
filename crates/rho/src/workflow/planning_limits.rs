use serde::{Deserialize, Serialize};

use super::{WorkflowError, WorkflowResult};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Budget {
    pub(crate) name: &'static str,
    pub(crate) limit: u64,
    pub(crate) receipt: String,
}

impl Budget {
    pub(crate) fn measured(
        name: &'static str,
        limit: u64,
        receipt: impl Into<String>,
    ) -> WorkflowResult<Self> {
        if limit == 0 {
            return Err(WorkflowError::BudgetExceeded {
                budget: name,
                limit,
                actual: 1,
            });
        }
        Ok(Self {
            name,
            limit,
            receipt: receipt.into(),
        })
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlanningMeasurements {
    pub(crate) receipt: String,
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
}

impl PlanningLimits {
    /// Build limits from an externally recorded acceptance run. No guessed
    /// default exists: the caller must provide each accepted measured value.
    pub(crate) fn from_measurements(values: PlanningMeasurements) -> WorkflowResult<Self> {
        let receipt = |name: &str| format!("{name}; {}", values.receipt);
        Ok(Self {
            total_source_bytes: Budget::measured(
                "total source bytes",
                values.total_source_bytes,
                receipt("source corpus"),
            )?,
            module_count: Budget::measured(
                "module count",
                values.module_count,
                receipt("source corpus"),
            )?,
            module_depth: Budget::measured(
                "module depth",
                values.module_depth,
                receipt("source corpus"),
            )?,
            evaluator_ticks: Budget::measured(
                "Starlark evaluator ticks",
                values.evaluator_ticks,
                receipt("evaluator counter"),
            )?,
            evaluator_heap_bytes: Budget::measured(
                "Starlark evaluator heap bytes",
                values.evaluator_heap_bytes,
                receipt("evaluator peak heap"),
            )?,
            call_stack_depth: Budget::measured(
                "Starlark call-stack depth",
                values.call_stack_depth,
                receipt("accepted recursion case"),
            )?,
            string_bytes: Budget::measured(
                "Starlark string bytes",
                values.string_bytes,
                receipt("largest accepted string"),
            )?,
            list_items: Budget::measured(
                "Starlark list items",
                values.list_items,
                receipt("largest accepted list"),
            )?,
            dict_items: Budget::measured(
                "Starlark dict items",
                values.dict_items,
                receipt("largest accepted dict"),
            )?,
            input_depth: Budget::measured(
                "input depth",
                values.input_depth,
                receipt("accepted input fixture"),
            )?,
            input_bytes: Budget::measured(
                "input bytes",
                values.input_bytes,
                receipt("accepted input fixture"),
            )?,
            node_count: Budget::measured(
                "node count",
                values.node_count,
                receipt("accepted graph fixture"),
            )?,
            edge_count: Budget::measured(
                "edge count",
                values.edge_count,
                receipt("accepted graph fixture"),
            )?,
            condition_depth: Budget::measured(
                "condition depth",
                values.condition_depth,
                receipt("accepted condition fixture"),
            )?,
            schema_depth: Budget::measured(
                "output schema depth",
                values.schema_depth,
                receipt("accepted schema fixture"),
            )?,
            schema_bytes: Budget::measured(
                "output schema bytes",
                values.schema_bytes,
                receipt("accepted schema fixture"),
            )?,
            graph_bytes: Budget::measured(
                "serialized graph bytes",
                values.graph_bytes,
                receipt("accepted graph fixture"),
            )?,
            worker_wall_millis: Budget::measured(
                "planning worker wall milliseconds",
                values.worker_wall_millis,
                receipt("supervised worker benchmark"),
            )?,
        })
    }
}
