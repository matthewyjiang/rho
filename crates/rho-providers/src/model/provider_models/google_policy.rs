use crate::{
    model::{ReasoningCapabilities, ReasoningLevelSet},
    reasoning::ReasoningLevel,
};

/// Returns whether a Google model id is suitable for Rho's text coding-agent path.
pub(crate) fn is_text_chat_model(model: &str) -> bool {
    let model = model.strip_prefix("models/").unwrap_or(model);
    let lower = model.to_ascii_lowercase();
    const BLOCKED: &[&str] = &[
        "-image",
        "tts",
        "lyria",
        "deep-research",
        "computer-use",
        "robotics",
        "embedding",
        "imagen",
        "veo",
        "aqa",
        "nano-banana",
        "omni-flash",
        "antigravity",
    ];
    !BLOCKED.iter().any(|needle| lower.contains(needle))
}

/// Advertised selectable reasoning levels for a Google model id.
pub(crate) fn reasoning_capabilities(model: &str, thinking: Option<bool>) -> ReasoningCapabilities {
    if thinking == Some(false) {
        return ReasoningCapabilities::NotConfigurable;
    }
    match thinking_policy(model) {
        ThinkingPolicy::Level { levels } | ThinkingPolicy::Budget { levels, .. } => {
            ReasoningCapabilities::Levels(ReasoningLevelSet::new(levels.to_vec()))
        }
        ThinkingPolicy::None => ReasoningCapabilities::NotConfigurable,
    }
}

/// Wire-oriented thinking controls derived from a model id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThinkingPolicy {
    Level {
        levels: &'static [ReasoningLevel],
    },
    Budget {
        levels: &'static [ReasoningLevel],
        flash_cap: bool,
    },
    None,
}

impl ThinkingPolicy {
    pub(crate) fn allows(self, level: ReasoningLevel) -> bool {
        match self {
            Self::Level { levels } | Self::Budget { levels, .. } => levels.contains(&level),
            Self::None => level == ReasoningLevel::Off,
        }
    }
}

pub(crate) fn thinking_policy(model: &str) -> ThinkingPolicy {
    match family(model) {
        Family::Gemini3(variant) => ThinkingPolicy::Level {
            levels: gemini3_levels(variant),
        },
        Family::Gemini25(variant) => ThinkingPolicy::Budget {
            levels: gemini25_levels(variant),
            flash_cap: matches!(variant, Gemini25Variant::Flash),
        },
        Family::Other => ThinkingPolicy::None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Gemini3(Gemini3Variant),
    Gemini25(Gemini25Variant),
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gemini3Variant {
    Pro,
    FlashLiteImage,
    Standard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gemini25Variant {
    Pro,
    Flash,
}

fn family(model: &str) -> Family {
    let model = model.strip_prefix("models/").unwrap_or(model);
    let lower = model.to_ascii_lowercase();
    if lower.starts_with("gemini-3") {
        return Family::Gemini3(if lower.contains("flash-lite-image") {
            Gemini3Variant::FlashLiteImage
        } else if lower.contains("pro") {
            Gemini3Variant::Pro
        } else {
            Gemini3Variant::Standard
        });
    }
    if lower.starts_with("gemini-2.5") {
        return Family::Gemini25(if lower.contains("pro") {
            Gemini25Variant::Pro
        } else {
            Gemini25Variant::Flash
        });
    }
    Family::Other
}

fn gemini3_levels(variant: Gemini3Variant) -> &'static [ReasoningLevel] {
    match variant {
        Gemini3Variant::Pro => &[
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
        ],
        Gemini3Variant::FlashLiteImage => &[ReasoningLevel::Minimal, ReasoningLevel::High],
        Gemini3Variant::Standard => &[
            ReasoningLevel::Minimal,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
        ],
    }
}

fn gemini25_levels(variant: Gemini25Variant) -> &'static [ReasoningLevel] {
    match variant {
        Gemini25Variant::Pro => &[
            ReasoningLevel::Minimal,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::Xhigh,
            ReasoningLevel::Max,
        ],
        Gemini25Variant::Flash => &[
            ReasoningLevel::Off,
            ReasoningLevel::Minimal,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::Xhigh,
            ReasoningLevel::Max,
        ],
    }
}

#[cfg(test)]
#[path = "google_policy_tests.rs"]
mod tests;
