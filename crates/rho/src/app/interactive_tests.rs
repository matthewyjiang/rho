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
        runtime: crate::agent::AgentRuntimeSpec::Rho {
            tools: crate::agent::ToolPolicy::All,
            model: crate::agent::ModelPolicy::Inherit,
            reasoning: None,
        },
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

// Covers: resume accepts the stored agent's current or legacy v1 fingerprint
// and refuses a changed definition or a session with no stored identity.
// Owner: interactive resume validation
#[test]
fn resume_validates_the_stored_agent_definition() {
    let definition = sample_definition("reviewer");
    let current = definition.fingerprint().to_string();
    let legacy = definition
        .legacy_v1_fingerprint()
        .expect("rho default tools encode legacy v1")
        .to_string();
    assert_ne!(legacy, current);
    for (stored_id, fingerprint, expected_error) in [
        (Some("reviewer"), current.as_str(), None),
        (Some("reviewer"), legacy.as_str(), None),
        (
            Some("reviewer"),
            "fingerprint-b",
            Some("definition changed"),
        ),
        (None, "unused", Some("no stored agent definition identity")),
    ] {
        let (_root, _cwd, session) = stored_session(stored_id, fingerprint);
        let result = validate_resume_agent(&session, &bound_agent(definition.clone()));
        match expected_error {
            None => result.unwrap(),
            Some(expected) => {
                let error = result.unwrap_err().to_string();
                assert!(error.contains(expected), "{error}");
            }
        }
    }
}
