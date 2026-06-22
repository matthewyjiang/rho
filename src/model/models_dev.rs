use std::{fs, path::PathBuf};

use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelMetadata {
    pub advertised_context_window: Option<u64>,
    pub effective_context_window: Option<u64>,
    pub usable_context_window: Option<u64>,
    pub long_context_threshold: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub cost_default: Option<ModelCost>,
    pub cost_long_context: Option<ModelCost>,
}

impl ModelMetadata {
    pub fn display_context_window(&self) -> Option<u64> {
        self.usable_context_window
            .or(self.effective_context_window)
            .or(self.advertised_context_window)
    }

    pub fn cost_for_input_tokens(&self, input_tokens: u64) -> Option<ModelCost> {
        if self
            .long_context_threshold
            .is_some_and(|threshold| input_tokens > threshold)
        {
            self.cost_long_context.or(self.cost_default)
        } else {
            self.cost_default
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelCost {
    pub input_micros_per_m: Option<u64>,
    pub output_micros_per_m: Option<u64>,
    pub cache_read_micros_per_m: Option<u64>,
    pub cache_write_micros_per_m: Option<u64>,
}

pub async fn fetch_model_metadata(provider: &str, model: &str) -> Option<ModelMetadata> {
    let upstream_provider = match provider {
        "openai" | "openai-codex" => "openai",
        other => other,
    };
    let response = reqwest::Client::new()
        .get("https://models.dev/api.json")
        .header("User-Agent", concat!("rho/", env!("CARGO_PKG_VERSION")))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<Value>()
        .await
        .ok()?;
    let metadata = model_metadata_from_api(&response, upstream_provider, model)?;
    let metadata = apply_builtin_overrides(provider, model, metadata);
    Some(apply_local_overrides(provider, model, metadata))
}

fn model_metadata_from_api(api: &Value, provider: &str, model: &str) -> Option<ModelMetadata> {
    let model = api.get(provider)?.get("models")?.get(model).or_else(|| {
        api.get(provider)?
            .get("models")?
            .get(model.strip_prefix("openai/")?)
    })?;
    let limit = model.get("limit");
    let cost = model.get("cost");
    Some(ModelMetadata {
        advertised_context_window: limit
            .and_then(|limit| limit.get("context"))
            .and_then(|value| value.as_u64()),
        effective_context_window: limit
            .and_then(|limit| limit.get("input").or_else(|| limit.get("context")))
            .and_then(|value| value.as_u64()),
        usable_context_window: None,
        long_context_threshold: None,
        max_output_tokens: limit
            .and_then(|limit| limit.get("output"))
            .and_then(|value| value.as_u64()),
        cost_default: Some(ModelCost {
            input_micros_per_m: cost
                .and_then(|cost| cost.get("input"))
                .and_then(cost_micros_per_million),
            output_micros_per_m: cost
                .and_then(|cost| cost.get("output"))
                .and_then(cost_micros_per_million),
            cache_read_micros_per_m: cost
                .and_then(|cost| cost.get("cache_read"))
                .and_then(cost_micros_per_million),
            cache_write_micros_per_m: cost
                .and_then(|cost| cost.get("cache_write"))
                .and_then(cost_micros_per_million),
        }),
        cost_long_context: None,
    })
}

fn apply_builtin_overrides(
    provider: &str,
    model: &str,
    mut metadata: ModelMetadata,
) -> ModelMetadata {
    match (provider, model) {
        ("openai", "gpt-5.5") => {
            metadata.advertised_context_window = Some(1_050_000);
            metadata.effective_context_window = Some(272_000);
            metadata.usable_context_window = Some(272_000);
            metadata.long_context_threshold = Some(272_000);
            metadata.max_output_tokens = Some(128_000);
            metadata.cost_default = Some(ModelCost {
                input_micros_per_m: Some(5_000_000),
                output_micros_per_m: Some(30_000_000),
                cache_read_micros_per_m: Some(500_000),
                cache_write_micros_per_m: None,
            });
            metadata.cost_long_context = Some(ModelCost {
                input_micros_per_m: Some(10_000_000),
                output_micros_per_m: Some(45_000_000),
                cache_read_micros_per_m: Some(1_000_000),
                cache_write_micros_per_m: None,
            });
        }
        ("openai-codex", "gpt-5.5") => {
            metadata.advertised_context_window = Some(1_050_000);
            metadata.effective_context_window = Some(400_000);
            metadata.usable_context_window = Some(272_000);
            metadata.long_context_threshold = Some(272_000);
            metadata.max_output_tokens = Some(128_000);
            metadata.cost_default = Some(ModelCost {
                input_micros_per_m: Some(5_000_000),
                output_micros_per_m: Some(30_000_000),
                cache_read_micros_per_m: Some(500_000),
                cache_write_micros_per_m: None,
            });
            metadata.cost_long_context = Some(ModelCost {
                input_micros_per_m: Some(10_000_000),
                output_micros_per_m: Some(45_000_000),
                cache_read_micros_per_m: Some(1_000_000),
                cache_write_micros_per_m: None,
            });
        }
        _ => {}
    }
    metadata
}

fn apply_local_overrides(provider: &str, model: &str, metadata: ModelMetadata) -> ModelMetadata {
    let Some(path) = local_overrides_path() else {
        return metadata;
    };
    let Ok(contents) = fs::read_to_string(path) else {
        return metadata;
    };
    let Ok(value) = contents.parse::<toml::Value>() else {
        return metadata;
    };
    let key = format!("{provider}/{model}");
    let Some(table) = value
        .get("models")
        .and_then(|models| models.get(&key))
        .and_then(|value| value.as_table())
    else {
        return metadata;
    };

    merge_toml_override(metadata, table)
}

fn local_overrides_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("RHO_MODELS_PATH") {
        return Some(path.into());
    }
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".rho/models.toml"))
}

fn merge_toml_override(
    mut metadata: ModelMetadata,
    table: &toml::map::Map<String, toml::Value>,
) -> ModelMetadata {
    metadata.advertised_context_window =
        toml_u64(table, "advertised_context_window").or(metadata.advertised_context_window);
    metadata.effective_context_window =
        toml_u64(table, "effective_context_window").or(metadata.effective_context_window);
    metadata.usable_context_window =
        toml_u64(table, "usable_context_window").or(metadata.usable_context_window);
    metadata.long_context_threshold =
        toml_u64(table, "long_context_threshold").or(metadata.long_context_threshold);
    metadata.max_output_tokens =
        toml_u64(table, "max_output_tokens").or(metadata.max_output_tokens);
    metadata.cost_default = toml_cost(table, "cost_default").or(metadata.cost_default);
    metadata.cost_long_context =
        toml_cost(table, "cost_long_context").or(metadata.cost_long_context);
    metadata
}

fn toml_u64(table: &toml::map::Map<String, toml::Value>, key: &str) -> Option<u64> {
    table
        .get(key)
        .and_then(|value| value.as_integer())
        .and_then(|value| u64::try_from(value).ok())
}

fn toml_cost(table: &toml::map::Map<String, toml::Value>, key: &str) -> Option<ModelCost> {
    let table = table.get(key)?.as_table()?;
    Some(ModelCost {
        input_micros_per_m: toml_cost_value(table, "input"),
        output_micros_per_m: toml_cost_value(table, "output"),
        cache_read_micros_per_m: toml_cost_value(table, "cache_read"),
        cache_write_micros_per_m: toml_cost_value(table, "cache_write"),
    })
}

fn toml_cost_value(table: &toml::map::Map<String, toml::Value>, key: &str) -> Option<u64> {
    let dollars = table.get(key).and_then(|value| {
        value
            .as_float()
            .or_else(|| value.as_integer().map(|v| v as f64))
    })?;
    dollars
        .is_finite()
        .then(|| (dollars.max(0.0) * 1_000_000.0).round() as u64)
}

fn cost_micros_per_million(value: &Value) -> Option<u64> {
    let dollars = value.as_f64().or_else(|| {
        value
            .as_str()?
            .trim_start_matches('$')
            .replace(',', "")
            .parse()
            .ok()
    })?;
    dollars
        .is_finite()
        .then(|| (dollars.max(0.0) * 1_000_000.0).round() as u64)
}
