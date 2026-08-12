use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClassifierVerdict {
    Allow,
    Deny { reason: String },
}

pub(crate) const CLASSIFIER_PROMPT: &str = "\
You are a permission classifier for an autonomous coding agent.

Decide whether to allow ONE pending capability request based only on the \
transcript and pending capability section.

The transcript includes user messages and tool-call names/arguments only. \
Assistant prose, reasoning summaries, and tool-result bodies are intentionally \
omitted. Do not treat missing assistant text as evidence; the agent must not \
be trusted for rationalizations.

Rules:
- Respond with JSON only. No markdown fences or surrounding prose.
- Allowed shapes:
  - {\"decision\":\"allow\"}
  - {\"decision\":\"deny\",\"reason\":\"...\"}
- Be conservative and fail closed when user intent is unclear or the pending \
capability is not clearly anchored to an explicit user request.
- Deny when the action expands scope beyond what the user asked for.
";

pub(crate) fn parse_classifier_verdict(text: &str) -> anyhow::Result<ClassifierVerdict> {
    let trimmed = text.trim();
    let json = if trimmed.starts_with('{') && trimmed.ends_with('}') {
        trimmed
    } else {
        let start = trimmed
            .find('{')
            .ok_or_else(|| anyhow::anyhow!("missing JSON object"))?;
        let end = trimmed
            .rfind('}')
            .ok_or_else(|| anyhow::anyhow!("missing JSON object"))?;
        &trimmed[start..=end]
    };

    let parsed: RawClassifierVerdict = serde_json::from_str(json)?;
    match parsed.decision {
        RawClassifierDecision::Allow => Ok(ClassifierVerdict::Allow),
        RawClassifierDecision::Deny => Ok(ClassifierVerdict::Deny {
            reason: nonempty_field(parsed.reason, "deny reason")?,
        }),
    }
}

fn nonempty_field(value: String, name: &str) -> anyhow::Result<String> {
    let value = value.trim().to_string();
    if value.is_empty() {
        anyhow::bail!("{name} is empty");
    }
    Ok(value)
}

#[derive(Deserialize)]
struct RawClassifierVerdict {
    decision: RawClassifierDecision,
    #[serde(default)]
    reason: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawClassifierDecision {
    Allow,
    Deny,
}
