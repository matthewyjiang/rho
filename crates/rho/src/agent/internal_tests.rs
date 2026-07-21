use pretty_assertions::assert_eq;
use rho_providers::reasoning::ReasoningLevel;

use super::*;

#[test]
fn registers_internal_agent_definitions() {
    let definitions = internal_definitions();

    assert_eq!(definitions.len(), 2);
    assert_eq!(definitions[0].id.as_str(), SESSION_TITLE_AGENT_ID);
    assert_eq!(definitions[1].id.as_str(), GOAL_JUDGE_AGENT_ID);
    for definition in definitions {
        assert_eq!(definition.model, ModelPolicy::Inherit);
        assert_eq!(definition.reasoning, Some(ReasoningLevel::Low));
        assert!(matches!(&definition.tools, ToolPolicy::Allow(tools) if tools.is_empty()));
    }
    assert_eq!(
        definitions[0].prompt,
        PromptPolicy::Replace(crate::tui::SESSION_TITLE_PROMPT.into())
    );
    assert_eq!(
        definitions[1].prompt,
        PromptPolicy::Replace(crate::tui::GOAL_JUDGE_PROMPT.into())
    );
}
