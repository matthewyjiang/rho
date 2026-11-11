use super::*;
use agent_client_protocol::v1::schema::UsageUpdate;
use pretty_assertions::assert_eq;

fn shape(system_prompt: PromptPolicy, persistence: SessionPersistence) -> ClaudeSessionShape {
    ClaudeSessionShape {
        system_prompt,
        model: Some("opus".into()),
        reasoning: Some(crate::agent::ReasoningLevel::High),
        tools: vec!["Read".into(), "Bash(git *)".into(), "Task".into()],
        max_turns: 40,
        persistence,
    }
}

// Covers: a delegated run carries identity, the turn cap, tools, the settings fence,
// and the extended system prompt in `session/new` `_meta`, under the mode the
// fence picked. Owner: Claude ACP policy (the whole session contract).
#[test]
fn delegated_session_carries_identity_and_fence_in_meta() {
    let dir = tempfile::tempdir().unwrap();
    let policy = ClaudeAcpPolicy::delegated(
        shape(
            PromptPolicy::Extend("be brief".into()),
            SessionPersistence::Keep,
        ),
        PermissionMode::Bypass,
        /*inherit_claude_config*/ false,
        dir.path().into(),
        None,
    );
    assert_eq!(
        policy.session_mode(),
        Some(SessionModeId::new("bypassPermissions"))
    );
    assert_eq!(
        Value::Object(policy.session_meta().unwrap()),
        json!({
            "systemPrompt": {"append": "be brief"},
            "claudeCode": {"options": {
                "tools": ["Read", "Bash"],
                "allowedTools": ["Read", "Bash(git *)"],
                "disallowedTools": ["Task", "Agent"],
                "settingSources": ["project"],
                "strictMcpConfig": true,
                "maxTurns": 40,
                "promptSuggestions": false,
                "model": "opus",
                "effort": "high",
            }},
        })
    );
}

// Covers: Rho's own one-shot replaces Claude's system prompt, loads no
// settings, rejects every ask, and leaves no resumable session.
// Owner: Claude ACP policy for internal agents.
#[test]
fn headless_one_shot_replaces_the_prompt_and_discards_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut shape = shape(
        PromptPolicy::Replace("advise".into()),
        SessionPersistence::Discard,
    );
    shape.tools = Vec::new();
    shape.max_turns = 1;
    let policy = ClaudeAcpPolicy::headless(shape, dir.path().into());
    assert_eq!(
        (
            policy.session_mode(),
            Value::Object(policy.session_meta().unwrap())
        ),
        (
            Some(SessionModeId::new("default")),
            json!({
                "systemPrompt": "advise",
                "claudeCode": {"options": {
                    "tools": [],
                    "disallowedTools": ["Task", "Agent"],
                    "settingSources": [],
                    "strictMcpConfig": true,
                    "maxTurns": 1,
                    "promptSuggestions": false,
                    "model": "opus",
                    "effort": "high",
                    "persistSession": false,
                }},
            })
        )
    );
}

// Covers: a usage update's `_meta` yields the rate-limit window (with
// utilization from `unifiedWindows` when the top level omits it) and the
// concrete model an alias resolved to; updates without them yield nothing.
// Owner: Claude ACP policy (feeds `/limits` and the run's resolved model).
#[test]
fn usage_meta_yields_rate_limit_and_resolved_model() {
    let dir = tempfile::tempdir().unwrap();
    let policy = ClaudeAcpPolicy::headless(
        shape(
            PromptPolicy::Extend(String::new()),
            SessionPersistence::Discard,
        ),
        dir.path().into(),
    );
    let with_meta = |meta: Value| {
        SessionUpdate::UsageUpdate(UsageUpdate::new(10, 100).meta(meta.as_object().cloned()))
    };
    let event = with_meta(json!({
        "_claude/model": "claude-opus-5-5",
        "_claude/rateLimit": {
            "status": "allowed",
            "resetsAt": 1_793_866_200,
            "rateLimitType": "five_hour",
            "isUsingOverage": false,
            "unifiedWindows": {
                "five_hour": {"utilization": 0.25, "resetsAt": 1_793_866_200},
                "seven_day": {"utilization": 0.18, "resetsAt": 1_794_063_600},
            },
        },
    }));
    assert_eq!(
        policy.status_effects(&event),
        vec![
            StreamEffect::RateLimit(RateLimitInfo {
                status: Some("allowed".into()),
                rate_limit_type: Some("five_hour".into()),
                resets_at: Some(1_793_866_200),
                utilization: Some(0.25),
                overage_status: None,
                overage_resets_at: None,
                is_using_overage: Some(false),
            }),
            StreamEffect::Status(StatusPatch {
                runtime_model: Some("claude-opus-5-5".into()),
                ..StatusPatch::default()
            }),
        ]
    );
    assert_eq!(
        policy.status_effects(&SessionUpdate::UsageUpdate(UsageUpdate::new(10, 100))),
        vec![]
    );
}

// Covers: a model pin reaches the adapter's highest-priority model source, so
// a `model` in the user's Claude settings cannot replace it; an inherited model
// leaves the user's choice alone.
// Owner: Claude ACP policy (spawn contract).
#[test]
fn model_pin_outranks_claude_settings() {
    let dir = tempfile::tempdir().unwrap();
    let env_for = |model: Option<&str>| {
        let mut shape = shape(
            PromptPolicy::Extend(String::new()),
            SessionPersistence::Keep,
        );
        shape.model = model.map(str::to_owned);
        ClaudeAcpPolicy::headless(shape, dir.path().into())
            .spawn_plan(None)
            .unwrap()
            .env
    };
    assert_eq!(
        (env_for(Some("haiku")), env_for(None)),
        (vec![("ANTHROPIC_MODEL".into(), "haiku".into())], vec![])
    );
}
