//! Canonical Claude usage-window families.
//!
//! Parse keys, `/limits` labels, and display sort order live here so the
//! `/usage` screen parser, stream observations, and cache stay aligned.

/// One Claude rate-limit window family.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WindowKind {
    FiveHour,
    SevenDay,
    SevenDaySonnet,
    SevenDayOpus,
    SevenDayFable,
    /// Stream aliases `seven_day_all` / `seven_day_all_models`.
    SevenDayAll,
    ExtraUsage,
    UsageWindow,
    Other(String),
}

impl WindowKind {
    /// Classify a stored or stream `rate_limit_type` key.
    pub(crate) fn from_key(key: &str) -> Self {
        match key {
            "five_hour" => Self::FiveHour,
            "seven_day" => Self::SevenDay,
            "seven_day_sonnet" => Self::SevenDaySonnet,
            "seven_day_opus" => Self::SevenDayOpus,
            "seven_day_fable" => Self::SevenDayFable,
            "seven_day_all" | "seven_day_all_models" => Self::SevenDayAll,
            "extra_usage" => Self::ExtraUsage,
            "usage_window" => Self::UsageWindow,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Classify the inner text of a `Current week (...)` `/usage` header.
    pub(crate) fn from_week_inner(inner: &str) -> Self {
        let inner = inner.trim();
        if inner.contains("all model") {
            Self::SevenDay
        } else if inner.contains("sonnet") {
            Self::SevenDaySonnet
        } else if inner.contains("opus") {
            Self::SevenDayOpus
        } else if inner.contains("fable") {
            Self::SevenDayFable
        } else {
            let slug: String = inner
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                .collect();
            let slug = slug.trim_matches('_').replace("__", "_");
            if slug.is_empty() {
                Self::SevenDay
            } else {
                Self::Other(format!("seven_day_{slug}"))
            }
        }
    }

    /// Stable merge key written to cache and matched against stream events.
    pub(crate) fn key(&self) -> &str {
        match self {
            Self::FiveHour => "five_hour",
            Self::SevenDay => "seven_day",
            Self::SevenDaySonnet => "seven_day_sonnet",
            Self::SevenDayOpus => "seven_day_opus",
            Self::SevenDayFable => "seven_day_fable",
            Self::SevenDayAll => "seven_day_all_models",
            Self::ExtraUsage => "extra_usage",
            Self::UsageWindow => "usage_window",
            Self::Other(key) => key,
        }
    }

    /// `/limits` and notice label for this family.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::FiveHour => "5-hour".into(),
            Self::SevenDay | Self::SevenDayAll => "Weekly".into(),
            Self::SevenDaySonnet => "Weekly Sonnet".into(),
            Self::SevenDayOpus => "Weekly Opus".into(),
            Self::SevenDayFable => "Fable".into(),
            Self::ExtraUsage => "Extra usage".into(),
            Self::UsageWindow => "Usage window".into(),
            Self::Other(key) => other_label(key),
        }
    }

    /// Stable `/limits` row order. Unknown families sort last, then by key.
    pub(crate) fn sort_rank(&self) -> u8 {
        match self {
            Self::FiveHour => 0,
            Self::SevenDay => 1,
            Self::SevenDaySonnet => 2,
            Self::SevenDayOpus => 3,
            Self::SevenDayFable => 4,
            Self::SevenDayAll => 5,
            Self::ExtraUsage | Self::UsageWindow | Self::Other(_) => 50,
        }
    }
}

fn other_label(value: &str) -> String {
    if let Some(slug) = value.strip_prefix("seven_day_") {
        return format!("Weekly {}", title_case_slug(slug));
    }
    let replaced = value.replace('_', " ");
    let mut chars = replaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Usage window".into(),
    }
}

fn title_case_slug(slug: &str) -> String {
    slug.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
