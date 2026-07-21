use rho_providers::model::{models_dev::ModelCost, ModelMetadata, ModelUsage};

fn priced_metadata() -> ModelMetadata {
    ModelMetadata {
        cost_default: Some(ModelCost {
            input_micros_per_m: Some(1_000_000),
            output_micros_per_m: Some(2_000_000),
            cache_read_micros_per_m: Some(100_000),
            cache_write_micros_per_m: None,
        }),
        ..ModelMetadata::default()
    }
}

#[test]
fn estimated_cost_uses_normalized_input_and_cache_read() {
    let usage = ModelUsage {
        input_tokens: Some(300_000),
        cache_read_tokens: Some(700_000),
        output_tokens: Some(100_000),
        ..ModelUsage::default()
    };

    assert_eq!(
        super::estimated_cost_usd_micros(&usage, Some(&priced_metadata())),
        Some(570_000)
    );
}

#[test]
fn formats_usd_for_compact_display() {
    assert_eq!(super::format_usd(570_000), "$0.570");
    assert_eq!(super::format_usd(12_340_000), "$12.34");
    assert_eq!(super::format_usd(123_400_000), "$123");
}
