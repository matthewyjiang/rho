//! Small local history with deliberately large provider-reported prompt usage.

use rho_sdk::{
    model::{ModelEvent, ModelResponse, ModelUsage},
    provider::ProviderEventSender,
    ProviderError,
};

pub(super) async fn intercept(
    prompt: &str,
    events: &ProviderEventSender,
) -> Option<Result<ModelResponse, ProviderError>> {
    let (prompt, phase) = prompt.rsplit_once(' ')?;
    if !matches!(phase, "fresh" | "resumed") {
        return None;
    }
    let text = match prompt {
        "fixture calibrated context history" => {
            // Twelve short notes give the summarizer useful older history to
            // remove without approaching the 98,304-token automatic threshold.
            let mut notes = (1..=12)
                .map(|index| {
                    format!(
                        "Inspection note {index}: the controller records sensor timestamps, \
                         checks actuator feedback, and keeps the original trace for review.\n"
                    )
                })
                .collect::<String>();
            notes.push_str(&format!("Calibration history ready: {phase}."));
            notes
        }
        "fixture calibrated context usage" => {
            // 100,000 / 131,072 = 76.3%, above the configured 75% threshold.
            // Omit context_window so the scenario also exercises models.toml.
            if let Err(error) = events
                .send(ModelEvent::Usage(ModelUsage {
                    input_tokens: Some(100_000),
                    ..ModelUsage::default()
                }))
                .await
            {
                return Some(Err(error));
            }
            format!("Calibrated response complete: {phase}.")
        }
        _ => return None,
    };
    if let Err(error) = events.send(ModelEvent::OutputDelta(text.clone())).await {
        return Some(Err(error));
    }
    Some(super::completed(text))
}
