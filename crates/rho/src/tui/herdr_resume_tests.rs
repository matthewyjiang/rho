use std::path::PathBuf;

use clap::Parser;
use pretty_assertions::assert_eq;
use rho_providers::model::{ReasoningCapabilities, ReasoningLevelSet};
use rho_providers::reasoning::ReasoningLevel;

use super::{
    resume_argv, HerdrResumeKey, HerdrSync, HerdrSyncStep, ResumeLaunchOptions, ResumeSelection,
};
use crate::herdr::HerdrSession;
use crate::permission::PermissionMode;

fn selection(
    (provider, model, auth): (&str, &str, &str),
    reasoning: ReasoningLevel,
    permission_mode: PermissionMode,
) -> ResumeSelection {
    ResumeSelection {
        provider: provider.into(),
        model: model.into(),
        auth: auth.into(),
        reasoning,
        permission_mode,
    }
}

// Covers: the Herdr resume command must reopen the same session with the live
// selection where it drifted from config, leave matching values to config (an
// explicit --model is stricter than a plain resume), repeat launch-only flags,
// parse as a real Rho command line, and stay within Herdr's argv rules.
// Owner: resume command policy
#[test]
fn resume_argv_pins_only_drift_from_config() {
    let codex = ("openai-codex", "gpt-6-astra", "codex");
    let levels = ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
        ReasoningLevel::Low,
        ReasoningLevel::High,
    ]));
    let live = selection(codex, ReasoningLevel::High, PermissionMode::AllowEdits);
    let cases = [
        (
            "matches config",
            ResumeLaunchOptions::default(),
            live.clone(),
            Some(live.clone()),
            levels.clone(),
            vec!["rho", "--resume", "session-1"],
        ),
        (
            "model drift pins model, auth, and reasoning",
            ResumeLaunchOptions::default(),
            live.clone(),
            Some(selection(
                ("openai", "gpt-5.5", "api-key"),
                ReasoningLevel::High,
                PermissionMode::AllowEdits,
            )),
            levels.clone(),
            vec![
                "rho",
                "--model",
                "openai-codex/gpt-6-astra",
                "--auth",
                "codex",
                "--reasoning",
                "high",
                "--resume",
                "session-1",
            ],
        ),
        (
            "reasoning and permission drift only",
            ResumeLaunchOptions::default(),
            live.clone(),
            Some(selection(codex, ReasoningLevel::Low, PermissionMode::Plan)),
            levels.clone(),
            vec![
                "rho",
                "--reasoning",
                "high",
                "--permission-mode",
                "allow_edits",
                "--resume",
                "session-1",
            ],
        ),
        (
            "unreadable config pins everything; launch flags repeat",
            ResumeLaunchOptions {
                config: Some(PathBuf::from("/tmp/rho config/config.toml")),
                agent: Some("reviewer".into()),
                definition: None,
                no_system_prompt: true,
                no_tools: true,
                no_subagents: true,
            },
            live.clone(),
            None,
            ReasoningCapabilities::Unknown,
            vec![
                "rho",
                "--config",
                "/tmp/rho config/config.toml",
                "--agent",
                "reviewer",
                "--no-system-prompt",
                "--no-tools",
                "--no-subagents",
                "--model",
                "openai-codex/gpt-6-astra",
                "--auth",
                "codex",
                "--reasoning",
                "high",
                "--permission-mode",
                "allow_edits",
                "--resume",
                "session-1",
            ],
        ),
        (
            "keyless auth and fixed reasoning are not pinned",
            ResumeLaunchOptions::default(),
            selection(
                ("ollama", "team/model:q8", "none"),
                ReasoningLevel::Medium,
                PermissionMode::Plan,
            ),
            Some(live.clone()),
            ReasoningCapabilities::NotConfigurable,
            vec![
                "rho",
                "--model",
                "ollama/team/model:q8",
                "--permission-mode",
                "plan",
                "--resume",
                "session-1",
            ],
        ),
    ];

    for (name, launch, live, configured, capabilities, expected) in cases {
        let argv = resume_argv(
            &launch,
            &live,
            configured.as_ref(),
            &capabilities,
            "session-1",
        );

        assert_eq!(argv, expected, "{name}");
        assert!(crate::herdr::resume_argv_is_valid(&argv), "{name}");
        let cli = crate::cli::Cli::try_parse_from(&argv)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(cli.resume, Some(Some("session-1".into())), "{name}");
        assert!(cli.command.is_none(), "{name}");
    }
}

fn key(session_id: Option<&str>, permission_mode: PermissionMode) -> HerdrResumeKey {
    HerdrResumeKey {
        session_id: session_id.map(str::to_string),
        selection: selection(
            ("openai", "gpt-5.5", "api-key"),
            ReasoningLevel::High,
            permission_mode,
        ),
        config_modified: None,
    }
}

fn session(id: &str) -> HerdrSession {
    HerdrSession {
        id: id.into(),
        resume_argv: Some(vec!["rho".into(), "--resume".into(), id.into()]),
    }
}

// Covers: Herdr keeps the first session id a source reports and ignores a
// different one, and takes a resume command only on a pane claim. So until a
// claim is confirmed Rho claims again (a timed-out startup claim is retried),
// a session Herdr may hold that differs from the current one is released and
// reclaimed (even if its report was never confirmed), a same-session pin change
// is updated in place, and unchanged state sends nothing.
// Owner: herdr sync policy
#[test]
fn herdr_sync_steps_by_session_transition() {
    let plan = PermissionMode::Plan;
    let edits = PermissionMode::AllowEdits;
    let a = || Some("a".to_string());
    let cases = [
        (
            "unchanged",
            Some(key(Some("a"), plan)),
            a(),
            key(Some("a"), plan),
            Some(session("a")),
            HerdrSyncStep::InSync,
        ),
        (
            "claim unconfirmed, no session",
            None,
            None,
            key(None, plan),
            None,
            HerdrSyncStep::Claim(None),
        ),
        (
            "claim unconfirmed; retry with session",
            None,
            a(),
            key(Some("a"), plan),
            Some(session("a")),
            HerdrSyncStep::Claim(Some(session("a"))),
        ),
        (
            "claimed, first session",
            Some(key(None, plan)),
            None,
            key(Some("a"), plan),
            Some(session("a")),
            HerdrSyncStep::Report(session("a")),
        ),
        (
            "same session, new pins",
            Some(key(Some("a"), plan)),
            a(),
            key(Some("a"), edits),
            Some(session("a")),
            HerdrSyncStep::Report(session("a")),
        ),
        (
            "session switched",
            Some(key(Some("a"), plan)),
            a(),
            key(Some("b"), plan),
            Some(session("b")),
            HerdrSyncStep::Reclaim(Some(session("b"))),
        ),
        (
            "session dropped",
            Some(key(Some("a"), plan)),
            a(),
            key(None, plan),
            None,
            HerdrSyncStep::Reclaim(None),
        ),
        (
            "switched before first confirm",
            None,
            a(),
            key(Some("b"), plan),
            Some(session("b")),
            HerdrSyncStep::Reclaim(Some(session("b"))),
        ),
        (
            "dropped before first confirm",
            None,
            a(),
            key(None, plan),
            None,
            HerdrSyncStep::Reclaim(None),
        ),
    ];

    for (name, accepted, held, current, current_session, expected) in cases {
        let sync = HerdrSync {
            built: None,
            accepted,
            held,
        };
        assert_eq!(sync.step(&current, current_session), expected, "{name}");
    }
}

// Covers: after a confirmed release the pane is unclaimed, so a re-claim that
// failed must be retried as a claim (Herdr rejects session updates until one
// lands), for both a switched and a dropped session.
// Owner: herdr sync policy
#[test]
fn failed_reclaim_after_release_claims_again() {
    let plan = PermissionMode::Plan;
    let cases = [
        (
            "switched",
            key(Some("b"), plan),
            Some(session("b")),
            HerdrSyncStep::Claim(Some(session("b"))),
        ),
        ("dropped", key(None, plan), None, HerdrSyncStep::Claim(None)),
    ];

    for (name, current, current_session, expected) in cases {
        let mut sync = HerdrSync {
            built: None,
            accepted: Some(key(Some("a"), plan)),
            held: Some("a".into()),
        };
        sync.note_released();
        sync.note_sent(current_session.as_ref());

        assert_eq!(sync.step(&current, current_session), expected, "{name}");
    }
}

// Covers: a session started with `--agent` whose live model is what the agent
// binds must not pin `--model`, since the relaunch's agent binding already
// selects it and an explicit pin only makes restore stricter.
// Owner: resume command policy (agent baseline)
#[test]
fn agent_bound_model_is_not_drift() {
    use crate::agent::{
        AgentDefinition, AgentId, AgentRuntimeSpec, ModelPolicy, ModelSelection, PromptPolicy,
        ToolPolicy,
    };

    let definition = AgentDefinition {
        id: AgentId::new("local").unwrap(),
        description: "local".into(),
        prompt: PromptPolicy::Extend("instructions".into()),
        runtime: AgentRuntimeSpec::Rho {
            tools: ToolPolicy::All,
            model: ModelPolicy::Select(ModelSelection {
                provider: Some("openai".into()),
                model: "gpt-5.5-mini".into(),
                auth: Some("api-key".into()),
            }),
            reasoning: Some(ReasoningLevel::Low),
            fast: false,
        },
    };
    let host = crate::config::Config {
        provider: "anthropic".into(),
        model: "claude-opus-4-8".into(),
        auth: "anthropic-api-key".into(),
        reasoning: ReasoningLevel::High,
        permission_mode: PermissionMode::Plan,
        ..crate::config::Config::default()
    };
    let baseline = ResumeSelection::from_config(&crate::app::relaunch_config(&definition, &host));
    let live = selection(
        ("openai", "gpt-5.5-mini", "api-key"),
        ReasoningLevel::Low,
        PermissionMode::Plan,
    );
    let launch = ResumeLaunchOptions {
        agent: Some("local".into()),
        ..ResumeLaunchOptions::default()
    };

    let argv = resume_argv(
        &launch,
        &live,
        Some(&baseline),
        &ReasoningCapabilities::Unknown,
        "session-1",
    );

    assert_eq!(
        argv,
        vec!["rho", "--agent", "local", "--resume", "session-1"]
    );
}
