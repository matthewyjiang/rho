use super::*;

fn measurements() -> PlanningMeasurements {
    PlanningMeasurements {
        receipt: "reproduce command".to_owned(),
        total_source_bytes: 1,
        module_count: 1,
        module_depth: 1,
        evaluator_ticks: 1,
        evaluator_heap_bytes: 1,
        call_stack_depth: 1,
        string_bytes: 1,
        list_items: 1,
        dict_items: 1,
        input_depth: 1,
        input_bytes: 1,
        node_count: 1,
        edge_count: 1,
        condition_depth: 1,
        schema_depth: 1,
        schema_bytes: 1,
        graph_bytes: 1,
        worker_wall_millis: 1,
        retained_output_per_stream_bytes: 1,
        retained_output_total_bytes: 1,
        rendered_template_bytes: 1,
        node_timeout_seconds: 1,
        prompt_expansion_bytes: 1,
        argv_expansion_bytes: 1,
        environment_expansion_bytes: 1,
    }
}

// Covers: a planner budget could lose its exact checked receipt entry.
// Owner: workflow planning limit policy.
#[test]
fn every_planning_budget_names_its_receipt_entry() {
    let limits = PlanningLimits::from_measurements(measurements()).unwrap();
    let limits = serde_json::to_value(limits).unwrap();
    let limits = limits.as_object().unwrap();

    for (entry, budget) in limits {
        assert_eq!(
            budget["receipt"],
            format!("limit_receipt.json planning.accepted.{entry}; reproduce command")
        );
    }
}
