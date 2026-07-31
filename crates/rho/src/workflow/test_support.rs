use std::collections::BTreeMap;

use super::*;

pub(crate) fn runtime_limits() -> FrozenRuntimeLimits {
    FrozenRuntimeLimits {
        retained_output_per_stream_bytes: 1024 * 1024,
        retained_output_total_bytes: 2 * 1024 * 1024,
        rendered_template_bytes: 1024 * 1024,
        node_timeout_seconds: 24 * 60 * 60,
        prompt_expansion_bytes: 1024 * 1024,
        argv_expansion_bytes: 1024 * 1024,
        environment_expansion_bytes: 1024 * 1024,
    }
}

pub(crate) fn id(value: &str) -> NodeId {
    NodeId::new(value).unwrap()
}

pub(crate) fn agent_node(name: &str, needs: &[&str], access: WorkspaceAccess) -> Node {
    let id = id(name);
    Node {
        id: id.clone(),
        display_name: name.to_owned(),
        needs: needs.iter().map(|value| self::id(value)).collect(),
        condition: None,
        execution: NodeExecution::Agent(AgentNode {
            agent: "reviewer".to_owned(),
            prompt: Template(vec![TemplatePart::Literal {
                value: "review".to_owned(),
            }]),
            output: None,
        }),
        access,
        allow_failure: false,
        timeout_seconds: 60,
        max_output_bytes: 1024,
    }
}

pub(crate) fn workflow(nodes: Vec<Node>) -> FrozenWorkflow {
    let graph = WorkflowGraph {
        name: WorkflowName::new("test").unwrap(),
        nodes: nodes
            .into_iter()
            .map(|node| (node.id.clone(), node))
            .collect(),
    };
    let resolved_nodes = graph
        .nodes
        .keys()
        .cloned()
        .map(|id| {
            (
                id,
                ResolvedNode::Agent(Box::new(ResolvedAgent {
                    agent_id: "reviewer".to_owned(),
                    fingerprint: "fingerprint".to_owned(),
                    runtime: AgentRuntime::Rho,
                    source_origin: "builtin".to_owned(),
                    trust_required: false,
                    prompt_policy: "review".to_owned(),
                    provider: None,
                    model: None,
                    reasoning: None,
                    step_limit: 100,
                    capabilities: Default::default(),
                    permission_ceiling: "auto".to_owned(),
                    auth_profile: None,
                    executable: None,
                    executable_identity: None,
                    arguments: Vec::new(),
                })),
            )
        })
        .collect();
    let mut workflow = FrozenWorkflow {
        schema_version: FROZEN_WORKFLOW_SCHEMA_VERSION,
        planner: PlannerIdentity {
            name: "rho".to_owned(),
            format_version: 1,
            starlark_version: "0.14.2".to_owned(),
        },
        graph_digest: Digest(String::new()),
        sources: SourceManifest {
            entry_label: "//workflow.star".to_owned(),
            modules: BTreeMap::from([(
                "//workflow.star".to_owned(),
                SourceFile {
                    digest: Digest(
                        "sha256:a5c059fd4fd0193f7778541d9f8baecd730bbb76a1b3ed86ca5a5eeea33085b6"
                            .to_owned(),
                    ),
                    bytes: "WORKFLOW = None".len() as u64,
                },
            )]),
        },
        inputs: BTreeMap::new(),
        graph,
        resolved_nodes,
        scheduler: FrozenSchedulerSettings {
            max_parallel_nodes: 8,
            max_parallel_agents: 8,
            max_parallel_commands: 8,
        },
        runtime_limits: runtime_limits(),
    };
    workflow.graph_digest = graph_digest(&workflow).unwrap();
    workflow
}

pub(crate) fn state(workflow: &FrozenWorkflow) -> WorkflowState {
    WorkflowState {
        revision: 0,
        lifecycle: RunLifecycle::Running,
        outcome: None,
        cancellation_requested: false,
        nodes: workflow
            .graph
            .nodes
            .keys()
            .cloned()
            .map(|id| (id, NodeState::Pending))
            .collect(),
        command_exits: BTreeMap::new(),
        outputs: BTreeMap::new(),
        completions: BTreeMap::new(),
    }
}

pub(crate) fn limits() -> PlanningLimits {
    PlanningLimits::from_measurements(PlanningMeasurements {
        receipt: "workflow test fixture measured from its exact source and graph".to_owned(),
        total_source_bytes: 1_000_000,
        module_count: 100,
        module_depth: 20,
        evaluator_ticks: 1_000_000,
        evaluator_heap_bytes: 64_000_000,
        call_stack_depth: 100,
        string_bytes: 1_000_000,
        list_items: 10_000,
        dict_items: 10_000,
        input_depth: 20,
        input_bytes: 1_000_000,
        node_count: 1_000,
        edge_count: 10_000,
        condition_depth: 20,
        schema_depth: 20,
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
    })
    .unwrap()
}
