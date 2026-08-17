//! Lenient serde visitors for the models.dev document.
//!
//! Field type mismatches become `None` / empty instead of failing the catalog.
//! One junk model or provider must not poison the rest of the snapshot.

use std::{collections::HashMap, fmt};

use serde::{
    de::{IgnoredAny, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};

use super::{
    context_over_threshold, CatalogReasoningOptions, ModelsDevCatalog, ModelsDevCost,
    ModelsDevCostRates, ModelsDevCostTier, ModelsDevModel, ModelsDevProvider,
};

#[derive(Deserialize)]
#[serde(untagged)]
enum Maybe<T> {
    Value(T),
    Invalid(IgnoredAny),
}

impl<'de> Deserialize<'de> for ModelsDevCatalog {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CatalogVisitor;

        impl<'de> Visitor<'de> for CatalogVisitor {
            type Value = ModelsDevCatalog;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a models.dev provider map")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut providers = HashMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if let Maybe::Value(provider) = map.next_value::<Maybe<ModelsDevProvider>>()? {
                        providers.insert(key, provider);
                    }
                }
                Ok(ModelsDevCatalog { providers })
            }
        }

        deserializer.deserialize_map(CatalogVisitor)
    }
}

impl<'de> Deserialize<'de> for ModelsDevCost {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CostVisitor;

        impl<'de> Visitor<'de> for CostVisitor {
            type Value = ModelsDevCost;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a models.dev cost object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut cost = ModelsDevCost::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "input" => cost.input = map.next_value::<LenientF64>()?.0,
                        "output" => cost.output = map.next_value::<LenientF64>()?.0,
                        "cache_read" => cost.cache_read = map.next_value::<LenientF64>()?.0,
                        "cache_write" => cost.cache_write = map.next_value::<LenientF64>()?.0,
                        "tiers" => cost.tiers = map.next_value::<LenientTiers>()?.0,
                        key if context_over_threshold(key).is_some() => {
                            if let Maybe::Value(rates) =
                                map.next_value::<Maybe<ModelsDevCostRates>>()?
                            {
                                cost.context_over.insert(key.to_string(), rates);
                            }
                        }
                        _ => {
                            let _ = map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(cost)
            }
        }

        deserializer.deserialize_map(CostVisitor)
    }
}

impl<'de> Deserialize<'de> for ModelsDevCostRates {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RatesVisitor;

        impl<'de> Visitor<'de> for RatesVisitor {
            type Value = ModelsDevCostRates;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a models.dev cost-rate object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut rates = ModelsDevCostRates::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "input" => rates.input = map.next_value::<LenientF64>()?.0,
                        "output" => rates.output = map.next_value::<LenientF64>()?.0,
                        "cache_read" => rates.cache_read = map.next_value::<LenientF64>()?.0,
                        "cache_write" => rates.cache_write = map.next_value::<LenientF64>()?.0,
                        _ => {
                            let _ = map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(rates)
            }
        }

        deserializer.deserialize_map(RatesVisitor)
    }
}

impl<'de> Deserialize<'de> for ModelsDevCostTier {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TierVisitor;

        impl<'de> Visitor<'de> for TierVisitor {
            type Value = ModelsDevCostTier;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a models.dev cost tier object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut tier = ModelsDevCostTier::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "input" => tier.rates.input = map.next_value::<LenientF64>()?.0,
                        "output" => tier.rates.output = map.next_value::<LenientF64>()?.0,
                        "cache_read" => tier.rates.cache_read = map.next_value::<LenientF64>()?.0,
                        "cache_write" => tier.rates.cache_write = map.next_value::<LenientF64>()?.0,
                        "tier" => {
                            if let Maybe::Value(size) = map.next_value::<Maybe<TierSize>>()? {
                                tier.size = size.size;
                            }
                        }
                        _ => {
                            let _ = map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(tier)
            }
        }

        deserializer.deserialize_map(TierVisitor)
    }
}

impl<'de> Deserialize<'de> for CatalogReasoningOptions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OptionsVisitor;

        impl<'de> Visitor<'de> for OptionsVisitor {
            type Value = CatalogReasoningOptions;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a models.dev reasoning_options array")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut options = CatalogReasoningOptions {
                    empty: true,
                    ..CatalogReasoningOptions::default()
                };
                while let Some(option) = seq.next_element::<Maybe<RawReasoningOption>>()? {
                    let Maybe::Value(option) = option else {
                        options.empty = false;
                        continue;
                    };
                    options.empty = false;
                    match option.kind.as_deref() {
                        Some("toggle") => options.has_toggle = true,
                        Some("effort") if !options.has_effort => {
                            options.has_effort = true;
                            options.effort_values = option.values;
                        }
                        _ => {}
                    }
                }
                Ok(options)
            }
        }

        deserializer.deserialize_seq(OptionsVisitor)
    }
}

pub(super) fn lenient_or_default<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: Deserialize<'de> + Default,
    D: Deserializer<'de>,
{
    Ok(match Maybe::<T>::deserialize(deserializer)? {
        Maybe::Value(value) => value,
        Maybe::Invalid(_) => T::default(),
    })
}

pub(super) fn lenient_model_map<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, ModelsDevModel>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(
        match Maybe::<HashMap<String, Maybe<ModelsDevModel>>>::deserialize(deserializer)? {
            Maybe::Value(models) => models
                .into_iter()
                .filter_map(|(id, model)| match model {
                    Maybe::Value(model) => Some((id, model)),
                    Maybe::Invalid(_) => None,
                })
                .collect(),
            Maybe::Invalid(_) => HashMap::new(),
        },
    )
}

struct LenientF64(Option<f64>);

impl<'de> Deserialize<'de> for LenientF64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(
            match Maybe::<NumberOrDollar>::deserialize(deserializer)? {
                Maybe::Value(value) => Some(value.0),
                Maybe::Invalid(_) => None,
            },
        ))
    }
}

struct NumberOrDollar(f64);

impl<'de> Deserialize<'de> for NumberOrDollar {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NumberVisitor;

        impl Visitor<'_> for NumberVisitor {
            type Value = NumberOrDollar;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a number or dollar string")
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
                Ok(NumberOrDollar(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(NumberOrDollar(value as f64))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(NumberOrDollar(value as f64))
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                value
                    .trim_start_matches('$')
                    .replace(',', "")
                    .parse()
                    .map(NumberOrDollar)
                    .map_err(E::custom)
            }
        }

        deserializer.deserialize_any(NumberVisitor)
    }
}

struct LenientTiers(Vec<ModelsDevCostTier>);

impl<'de> Deserialize<'de> for LenientTiers {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(
            match Maybe::<Vec<Maybe<ModelsDevCostTier>>>::deserialize(deserializer)? {
                Maybe::Value(tiers) => tiers
                    .into_iter()
                    .filter_map(|tier| match tier {
                        Maybe::Value(tier) => Some(tier),
                        Maybe::Invalid(_) => None,
                    })
                    .collect(),
                Maybe::Invalid(_) => Vec::new(),
            },
        ))
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct TierSize {
    #[serde(deserialize_with = "lenient_or_default")]
    size: Option<u64>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawReasoningOption {
    #[serde(rename = "type", deserialize_with = "lenient_or_default")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "lenient_opt_effort_values")]
    values: Option<Vec<Option<String>>>,
}

fn lenient_opt_effort_values<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<Option<String>>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(
        match Maybe::<Vec<LenientOptString>>::deserialize(deserializer)? {
            Maybe::Value(values) => Some(values.into_iter().map(|value| value.0).collect()),
            Maybe::Invalid(_) => None,
        },
    )
}

struct LenientOptString(Option<String>);

impl<'de> Deserialize<'de> for LenientOptString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        lenient_or_default(deserializer).map(Self)
    }
}
