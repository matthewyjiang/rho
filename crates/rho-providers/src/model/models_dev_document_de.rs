//! Lenient serde visitors for the models.dev document.
//!
//! Field type mismatches become `None` / empty instead of failing the catalog.
//! One junk model or provider must not poison the rest of the snapshot.
//!
//! Large objects stream through `visit_map`. `#[serde(untagged)]` `Maybe<T>` is
//! only used for leaf scalars and small objects, where Content buffering stays
//! bounded.

use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    marker::PhantomData,
};

use serde::{
    de::{value::MapAccessDeserializer, IgnoredAny, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};

use super::{
    context_over_threshold, CatalogReasoningOptions, ModelsDevCatalog, ModelsDevCost,
    ModelsDevCostRates, ModelsDevCostTier, ModelsDevModel, ModelsDevProvider,
};

/// Accept `T` or ignore a type mismatch. Untagged, so serde buffers `T` into
/// `Content` first. Use only for values whose size is bounded.
#[derive(Deserialize)]
#[serde(untagged)]
enum Maybe<T> {
    Value(T),
    Invalid(IgnoredAny),
}

/// Skip a non-object without buffering it. Objects deserialize as `T` in place.
struct SkipInvalid<T>(Option<T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for SkipInvalid<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SkipVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for SkipVisitor<T> {
            type Value = SkipInvalid<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an object or a skipped value")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map))
                    .map(|value| SkipInvalid(Some(value)))
            }

            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(SkipInvalid(None))
            }

            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(SkipInvalid(None))
            }

            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(SkipInvalid(None))
            }

            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(SkipInvalid(None))
            }

            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
                Ok(SkipInvalid(None))
            }

            fn visit_bytes<E>(self, _: &[u8]) -> Result<Self::Value, E> {
                Ok(SkipInvalid(None))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(SkipInvalid(None))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(SkipInvalid(None))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(SkipInvalid(None))
            }
        }

        deserializer.deserialize_any(SkipVisitor(PhantomData))
    }
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
                    if let SkipInvalid(Some(provider)) = map.next_value()? {
                        providers.insert(key, provider);
                    }
                }
                Ok(ModelsDevCatalog { providers })
            }
        }

        deserializer.deserialize_map(CatalogVisitor)
    }
}

impl<'de> Deserialize<'de> for ModelsDevProvider {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ProviderVisitor;

        impl<'de> Visitor<'de> for ProviderVisitor {
            type Value = ModelsDevProvider;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a models.dev provider object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut provider = ModelsDevProvider::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "npm" => provider.npm = map.next_value::<LenientOptString>()?.0,
                        "models" => provider.models = map.next_value::<LenientModelMap>()?.0,
                        _ => {
                            let _ = map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(provider)
            }
        }

        deserializer.deserialize_map(ProviderVisitor)
    }
}

struct ModelMap(HashMap<String, ModelsDevModel>);

impl<'de> Deserialize<'de> for ModelMap {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ModelsVisitor;

        impl<'de> Visitor<'de> for ModelsVisitor {
            type Value = ModelMap;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a models.dev model map")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut models = HashMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if let SkipInvalid(Some(model)) = map.next_value()? {
                        models.insert(key, model);
                    }
                }
                Ok(ModelMap(models))
            }
        }

        deserializer.deserialize_map(ModelsVisitor)
    }
}

struct LenientModelMap(HashMap<String, ModelsDevModel>);

impl<'de> Deserialize<'de> for LenientModelMap {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(
            SkipInvalid::<ModelMap>::deserialize(deserializer)?
                .0
                .map(|models| models.0)
                .unwrap_or_default(),
        ))
    }
}

fn read_rate_key<'de, A: MapAccess<'de>>(
    rates: &mut ModelsDevCostRates,
    key: &str,
    map: &mut A,
) -> Result<bool, A::Error> {
    match key {
        "input" => rates.input = map.next_value::<LenientF64>()?.0,
        "output" => rates.output = map.next_value::<LenientF64>()?.0,
        "cache_read" => rates.cache_read = map.next_value::<LenientF64>()?.0,
        "cache_write" => rates.cache_write = map.next_value::<LenientF64>()?.0,
        _ => return Ok(false),
    }
    Ok(true)
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
                let mut rates = ModelsDevCostRates::default();
                let mut tiers = Vec::new();
                let mut context_over = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if read_rate_key(&mut rates, key.as_str(), &mut map)? {
                        continue;
                    }
                    match key.as_str() {
                        "tiers" => tiers = map.next_value::<LenientTiers>()?.0,
                        key if context_over_threshold(key).is_some() => {
                            if let Maybe::Value(tier_rates) =
                                map.next_value::<Maybe<ModelsDevCostRates>>()?
                            {
                                context_over.insert(key.to_string(), tier_rates);
                            }
                        }
                        _ => {
                            let _ = map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(ModelsDevCost::from_parts(rates, tiers, context_over))
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
                    if !read_rate_key(&mut rates, key.as_str(), &mut map)? {
                        let _ = map.next_value::<IgnoredAny>()?;
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
                    if read_rate_key(&mut tier.rates, key.as_str(), &mut map)? {
                        continue;
                    }
                    match key.as_str() {
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
