//! `/create-agent` and `/agents create`: the guided agent creator through the
//! built-in `rho-agent-creator` skill. Hosts check the active agent's tools
//! and build the model prompt here, then start the turn with a
//! host-requested `skill` call for [`SKILL`].

/// Built-in skill the creator turn loads.
pub(crate) const SKILL: &str = "rho-agent-creator";

/// Tools the guided workflow uses: it asks with `questionnaire`, writes the
/// definition, and checks it with `rho`.
const REQUIRED_TOOLS: [&str; 5] = ["skill", "questionnaire", "read_file", "write", "rho"];

/// The active agent lacks tools the creator needs. Displays as a transcript
/// error.
#[derive(Debug)]
pub(crate) struct MissingTools(Vec<&'static str>);

impl std::fmt::Display for MissingTools {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "could not start agent creator: active agent is missing required tools: {}",
            self.0.join(", ")
        )
    }
}

impl std::error::Error for MissingTools {}

/// The creator turn's model prompt for the user's `request` (empty for
/// none), or the tools the active agent lacks.
pub(crate) fn model_prompt(
    request: &str,
    has_tool: impl Fn(&str) -> bool,
) -> Result<String, MissingTools> {
    let missing = REQUIRED_TOOLS
        .into_iter()
        .filter(|name| !has_tool(name))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(MissingTools(missing));
    }
    Ok(if request.is_empty() {
        "Create a new Rho agent through the guided workflow.".to_string()
    } else {
        format!("Create a new Rho agent through the guided workflow. User request: {request}")
    })
}
