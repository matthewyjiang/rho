use super::*;
use pretty_assertions::assert_eq;

// Covers: filesystem command changes and /agent rebinding must replace the
// host's menus, without resending unchanged advertisements on every prompt.
// Owner: ACP session advertisements, over the real stdio server.
#[test]
fn session_menus_follow_discovery_and_agent_rebinding() {
    let workspace = Workspace::new();
    let agents = workspace.home.path().join(".rho/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("menu-child.md"),
        "---\ndescription: Menu test child\nmodel-policy: select\nprovider: xai\nmodel: grok-child\ntools: [read_file]\n---\nRead only.\n",
    )
    .unwrap();
    let mut server = workspace.server();
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    let (created, _) = server.request(
        "session/new",
        json!({"cwd": workspace.project, "mcpServers": []}),
    );
    let created = created.unwrap();
    let session_id = created["sessionId"].as_str().unwrap();
    // This request also drains the initial command update, sent after new.
    let (_, initial) = server.request(
        "session/prompt",
        json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "/model"}]}),
    );
    let initial = menu_updates(initial);
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0]["sessionUpdate"], "available_commands_update");
    let commands = initial[0]["availableCommands"].as_array().unwrap();

    let skill = workspace.project.join(".agents/skills/menu-skill");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: menu-skill\ndescription: A newly installed skill\n---\nInstructions.\n",
    )
    .unwrap();
    let mut with_skill = commands.clone();
    let skill_offset = with_skill
        .iter()
        .position(|command| command["name"].as_str().unwrap().starts_with('$'))
        .unwrap();
    with_skill.insert(
        skill_offset,
        json!({"name": "$menu-skill", "description": "A newly installed skill"}),
    );
    for expected in [
        vec![json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": with_skill,
        })],
        vec![],
    ] {
        let (_, updates) = server.request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "/model"}]}),
        );
        assert_eq!(menu_updates(updates), expected);
    }

    let (reply, updates) = server.request(
        "session/prompt",
        json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "/agent:menu-child read the notes"}]}),
    );
    assert_eq!(reply.unwrap()["stopReason"], "end_turn");
    let last_menu = updates
        .iter()
        .rposition(|update| {
            matches!(
                update["sessionUpdate"].as_str(),
                Some("config_option_update" | "available_commands_update")
            )
        })
        .unwrap();
    let first_activity = updates
        .iter()
        .position(|update| {
            matches!(
                update["sessionUpdate"].as_str(),
                Some("tool_call" | "agent_message_chunk")
            )
        })
        .unwrap();
    assert!(
        last_menu < first_activity,
        "rebind menus precede the model turn"
    );
    let updates = menu_updates(updates);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0]["sessionUpdate"], "config_option_update");
    let model = updates[0]["configOptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|option| option["id"] == "model")
        .unwrap();
    assert_eq!(model["currentValue"], "xai/grok-child");
    assert_eq!(updates[1]["sessionUpdate"], "available_commands_update");
    assert_eq!(
        updates[1]["availableCommands"],
        Value::Array(
            commands
                .iter()
                .filter(|command| !command["name"].as_str().unwrap().starts_with('$'))
                .cloned()
                .collect()
        )
    );
}

fn menu_updates(updates: Vec<Value>) -> Vec<Value> {
    updates
        .into_iter()
        .filter(|update| {
            matches!(
                update["sessionUpdate"].as_str(),
                Some("config_option_update" | "available_commands_update")
            )
        })
        .collect()
}

// Covers: both settings APIs must notify hosts of the complete current config,
// including the legacy mode setter, and no-op/rejected requests must not spam it.
// Owner: ACP session advertisements, over the real stdio server.
#[test]
fn settings_changes_publish_config_updates() {
    let workspace = Workspace::new();
    let mut server = workspace.server();
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    let (created, _) = server.request(
        "session/new",
        json!({"cwd": workspace.project, "mcpServers": []}),
    );
    let created = created.unwrap();
    let session_id = created["sessionId"].as_str().unwrap();
    // Drain the command list after the new-session response.
    let _ = server.request(
        "session/prompt",
        json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "/model"}]}),
    );
    let mut options = created["configOptions"].clone();
    for (method, args, expected_mode, changed, success) in [
        (
            "session/set_mode",
            json!({"modeId": "bypass"}),
            "bypass",
            true,
            true,
        ),
        (
            "session/set_config_option",
            json!({"configId": "mode", "value": "plan"}),
            "plan",
            true,
            true,
        ),
        (
            "session/set_config_option",
            json!({"configId": "mode", "value": "plan"}),
            "plan",
            false,
            true,
        ),
        (
            "session/set_mode",
            json!({"modeId": "unknown"}),
            "plan",
            false,
            false,
        ),
    ] {
        let mut params = args;
        params["sessionId"] = json!(session_id);
        let (reply, updates) = server.request(method, params);
        assert_eq!(reply.is_ok(), success);
        options
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|option| option["id"] == "mode")
            .unwrap()["currentValue"] = json!(expected_mode);
        let expected = if changed {
            vec![json!({"sessionUpdate": "config_option_update", "configOptions": options})]
        } else {
            vec![]
        };
        assert_eq!(menu_updates(updates), expected);
        if method == "session/set_config_option" && success {
            assert_eq!(reply.unwrap()["configOptions"], options);
        }
    }
}

// Covers: commands created by a running tool must reach the host before the
// prompt response, not wait until the user sends another prompt.
// Owner: ACP session advertisements, over the real stdio server.
#[test]
fn tool_created_commands_are_published_before_prompt_returns() {
    let workspace = Workspace::new();
    let templates = workspace.project.join(".rho/prompts");
    std::fs::create_dir_all(&templates).unwrap();
    let mut server = AcpServer::spawn(
        workspace.home.path(),
        &workspace.project,
        "write-path",
        &templates.join("created.md"),
    );
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    let (created, _) = server.request(
        "session/new",
        json!({"cwd": workspace.project, "mcpServers": []}),
    );
    let created = created.unwrap();
    let session_id = created["sessionId"].as_str().unwrap();
    let (changed, initial) = server.request(
        "session/set_mode",
        json!({"sessionId": session_id, "modeId": "bypass"}),
    );
    changed.unwrap();
    let mut names = initial
        .iter()
        .find(|update| update["sessionUpdate"] == "available_commands_update")
        .unwrap()["availableCommands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command["name"].clone())
        .collect::<Vec<_>>();
    let (reply, updates) = server.request(
        "session/prompt",
        json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "create a prompt template"}]}),
    );
    assert_eq!(reply.unwrap()["stopReason"], "end_turn");
    let updates = menu_updates(updates);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["sessionUpdate"], "available_commands_update");
    let init_offset = names.iter().position(|name| name == "init").unwrap();
    names.insert(init_offset + 1, json!("prompt:created"));
    assert_eq!(
        updates[0]["availableCommands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|command| command["name"].clone())
            .collect::<Vec<_>>(),
        names
    );
}
