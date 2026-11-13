use pretty_assertions::assert_eq;
use rho_protocol::ModelChoice;

use super::*;

fn choice(provider: &str, model: &str) -> ModelChoice {
    ModelChoice {
        model: ModelRef {
            provider: provider.into(),
            model: model.into(),
        },
        label: model.into(),
    }
}

// Covers: `new`'s model is found as provider/model, or as a model id only
// one provider offers; an ambiguous or unknown one is refused with what the
// session offers.
// Owner: session-control new.
#[test]
fn a_model_is_chosen_by_provider_and_id() {
    let offered = [
        choice("openai", "gpt"),
        choice("azure", "gpt"),
        choice("xai", "grok"),
    ];
    let options = rho_protocol::ModelOptions {
        current: offered[0].model.clone(),
        auth: "api-key".into(),
        available: offered.to_vec(),
    };
    let chosen = |wanted| choose_model(&options, wanted).map_err(|error| error.to_string());

    assert_eq!(
        [chosen("azure/gpt"), chosen("grok"), chosen("gpt")],
        [
            Ok(offered[1].model.clone()),
            Ok(offered[2].model.clone()),
            Err("model \"gpt\" does not name one model the session offers; name it as provider/model, one of: openai/gpt, azure/gpt, xai/grok".into()),
        ]
    );
}
