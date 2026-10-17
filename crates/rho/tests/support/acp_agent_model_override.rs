use super::*;
use pretty_assertions::assert_eq;

// Covers: adding an optional key must not prevent loading a keyless child or
// silently change the saved authentication selection.
// Owner: ACP restoration through the real binder and stdio server.
#[test]
fn overridden_keyless_child_survives_new_optional_credentials() {
    use rho_providers::{
        credentials::{save_provider_api_key, FileCredentialStore},
        model::provider_models::{
            replace_cached_provider_models_for_tests, with_provider_models_cache_dir_for_tests,
            ProviderModel,
        },
        provider::{
            intern_custom_openai_compatible_providers, CustomProviderSpec,
            CustomProviderThreadScope,
        },
    };

    let workspace = Workspace::new();
    let cache = workspace.home.path().join(".cache");
    let names = intern_custom_openai_compatible_providers([CustomProviderSpec::new(
        "review-fixture",
        None,
    )])
    .unwrap();
    let _scope = CustomProviderThreadScope::enter(names);
    with_provider_models_cache_dir_for_tests(cache.join("rho"), || {
        replace_cached_provider_models_for_tests(
            "review-fixture",
            &[ProviderModel {
                provider: "review-fixture".into(),
                model: "vendor/review".into(),
                display_name: "Review fixture".into(),
                context_window: None,
                max_output_tokens: None,
                reasoning_capabilities: Default::default(),
            }],
        )
        .unwrap();
    });
    let config = workspace.home.path().join("config.toml");
    std::fs::write(
        &config,
        r#"provider = "openai-codex"
model = "gpt-6.1-sol"
auth = "codex"
[providers.custom.review-fixture]
base_url = "http://127.0.0.1:9/v1"
[behavior]
permission_mode = "plan"
check_for_updates = false
"#,
    )
    .unwrap();
    let rho_home = workspace.home.path().join(".rho");
    let mut command = Command::new(env!("CARGO_BIN_EXE_rho"));
    command
        .current_dir(&workspace.project)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", workspace.home.path())
        .env("RHO_HOME", &rho_home)
        .env("XDG_CACHE_HOME", &cache)
        .env("RHO_CREDENTIAL_STORE", "file")
        .env("RHO_AUTOMATION_TEST_MODE", "inspect")
        .arg("--config")
        .arg(config)
        .arg("acp");
    let mut server = AcpServer::from_command(&mut command);
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    let created = server
        .request(
            "session/new",
            json!({"cwd": workspace.project, "mcpServers": []}),
        )
        .0
        .unwrap();
    let id = created["sessionId"].as_str().unwrap();
    server.prompt(
        id,
        "/agent:reviewer --model review-fixture/vendor/review -- Review this.",
    );
    let store = FileCredentialStore::with_rho_home(rho_home).unwrap();
    save_provider_api_key(&store, "review-fixture-api-key", "fixture-key").unwrap();
    server
        .request(
            "session/load",
            json!({"sessionId": id, "cwd": workspace.project, "mcpServers": []}),
        )
        .0
        .unwrap();
    let reply = server.prompt(id, "Continue the review.");
    let inspection: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(inspection["identity"]["provider"], "review-fixture");
    assert_eq!(inspection["identity"]["model"], "vendor/review");
}

// Covers: restoring an overridden child must preserve its auth profile through
// another turn/checkpoint and a second host load, not revert to process auth.
// An acknowledged host model switch must also survive an immediate reload.
// Owner: ACP lifecycle over the actual stdio server and fixture provider.
#[test]
fn overridden_child_survives_load_save_load_with_foreign_parent_auth() {
    let workspace = Workspace::new();
    std::fs::write(
        workspace.home.path().join("config.toml"),
        "provider = \"openai-codex\"\nmodel = \"gpt-6.1-sol\"\nauth = \"codex\"\n[behavior]\npermission_mode = \"plan\"\ncheck_for_updates = false\n",
    ).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_rho"));
    command
        .current_dir(&workspace.project)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", workspace.home.path())
        .env("RHO_HOME", workspace.home.path().join(".rho"))
        .env("RHO_CREDENTIAL_STORE", "file")
        .env("RHO_AUTOMATION_TEST_MODE", "inspect")
        .env("XAI_API_KEY", "fixture-key-not-a-real-credential")
        .arg("--config")
        .arg(workspace.home.path().join("config.toml"))
        .arg("acp");
    let mut server = AcpServer::from_command(&mut command);
    server
        .request("initialize", json!({"protocolVersion": 1}))
        .0
        .unwrap();
    let created = server
        .request(
            "session/new",
            json!({"cwd": workspace.project, "mcpServers": []}),
        )
        .0
        .unwrap();
    let id = created["sessionId"].as_str().unwrap();
    let reply = server.prompt(id, "/agent:reviewer --model xai/grok-4.7 -- Review this.");
    let inspection: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(inspection["identity"]["provider"], "xai");
    assert_eq!(inspection["identity"]["model"], "grok-4.7");

    for _ in 0..2 {
        let loaded = server
            .request(
                "session/load",
                json!({"sessionId": id, "cwd": workspace.project, "mcpServers": []}),
            )
            .0
            .unwrap();
        let model = loaded["configOptions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|option| option["id"] == "model")
            .unwrap();
        assert_eq!(model["currentValue"], "xai/grok-4.7");
        let reply = server.prompt(id, "Continue the review.");
        let inspection: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(inspection["identity"]["provider"], "xai");
        assert_eq!(inspection["identity"]["model"], "grok-4.7");
    }
    server
        .request(
            "session/set_config_option",
            json!({"sessionId": id, "configId": "model", "value": "xai/grok-4.5"}),
        )
        .0
        .unwrap();
    let loaded = server
        .request(
            "session/load",
            json!({"sessionId": id, "cwd": workspace.project, "mcpServers": []}),
        )
        .0
        .unwrap();
    let model = loaded["configOptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|option| option["id"] == "model")
        .unwrap();
    assert_eq!(model["currentValue"], "xai/grok-4.5");
}
