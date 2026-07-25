use super::*;

fn stored_session(
    agent_id: Option<&str>,
    fingerprint: &str,
) -> (tempfile::TempDir, tempfile::TempDir, Session) {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = match agent_id {
        Some(agent_id) => {
            Session::create_in_root_with_agent(root.path(), cwd.path(), agent_id, fingerprint)
                .unwrap()
        }
        None => Session::create_in_root(root.path(), cwd.path()).unwrap(),
    };
    (root, cwd, session)
}

fn sample_definition(id: &str) -> crate::agent::AgentDefinition {
    crate::agent::AgentDefinition {
        id: crate::agent::AgentId::new(id).unwrap(),
        description: "test".into(),
        prompt: crate::agent::PromptPolicy::Extend(String::new()),
        model: crate::agent::ModelPolicy::Inherit,
        runtime: crate::agent::AgentRuntimeSpec::Rho {
            tools: crate::agent::ToolPolicy::All,
        },
        reasoning: None,
    }
}

fn bound_agent(definition: crate::agent::AgentDefinition) -> crate::app::agent_binding::BoundAgent {
    crate::app::agent_binding::AgentBinder::bind(
        std::sync::Arc::new(definition),
        crate::app::agent_binding::AgentInvocation {
            role: crate::app::agent_binding::AgentRole::InteractiveRoot,
            available_tools: crate::agent::AgentCapabilities::all_host_tools(),
        },
        &crate::config::Config::default(),
    )
    .unwrap()
}

#[test]
fn resume_accepts_unchanged_agent_definition() {
    let definition = sample_definition("reviewer");
    let fingerprint = definition.fingerprint().to_string();
    let (_root, _cwd, session) = stored_session(Some("reviewer"), &fingerprint);
    validate_resume_agent(&session, &bound_agent(definition)).unwrap();
}

#[test]
fn resume_accepts_legacy_v1_fingerprint_for_compatible_rho_definition() {
    let definition = sample_definition("reviewer");
    let legacy = definition
        .legacy_v1_fingerprint()
        .expect("rho default tools encode legacy v1")
        .to_string();
    let current = definition.fingerprint().to_string();
    assert_ne!(legacy, current);
    let (_root, _cwd, session) = stored_session(Some("reviewer"), &legacy);
    validate_resume_agent(&session, &bound_agent(definition)).unwrap();
}

#[test]
fn resume_reports_changed_agent_definition() {
    let definition = sample_definition("reviewer");
    let (_root, _cwd, session) = stored_session(Some("reviewer"), "fingerprint-b");
    let error = validate_resume_agent(&session, &bound_agent(definition)).unwrap_err();
    assert!(error.to_string().contains("definition changed"));
}

#[test]
fn resume_reports_missing_agent_identity() {
    let definition = sample_definition("default");
    let (_root, _cwd, session) = stored_session(None, "unused");
    let error = validate_resume_agent(&session, &bound_agent(definition)).unwrap_err();
    assert!(error
        .to_string()
        .contains("no stored agent definition identity"));
}
