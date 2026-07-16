use crate::{
    model::{catalog, ModelError},
    subagent::{OnExit, Preset, PresetSource},
};

use super::{apply_preset_overrides, is_interactive_startup_unavailable_error};

#[test]
fn provider_only_preset_selects_the_provider_default_model() {
    let mut config = crate::config::Config {
        provider: "openai-codex".into(),
        model: "gpt-5-codex".into(),
        ..crate::config::Config::default()
    };
    let preset = Preset {
        name: "anthropic-reviewer".into(),
        description: "review".into(),
        model: None,
        provider: Some("anthropic".into()),
        reasoning: None,
        tools: None,
        on_exit: OnExit::Keep,
        prompt: String::new(),
        source: PresetSource::BuiltIn,
    };

    apply_preset_overrides(&mut config, &preset).unwrap();

    assert_eq!(config.provider, "anthropic");
    assert_eq!(
        config.model,
        catalog::default_model_for_provider("anthropic").unwrap()
    );
}

#[test]
fn unsupported_provider_is_nonfatal_for_interactive_startup() {
    assert!(is_interactive_startup_unavailable_error(
        &ModelError::UnsupportedProvider("anthropic".into())
    ));
}
