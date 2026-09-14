use serde::{Deserialize, Serialize};

/// When the interactive TUI reveals received assistant and reasoning text.
/// This does not change provider streaming or persisted response content.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StreamingMode {
    #[default]
    Live,
    Paragraph,
    Off,
}

impl StreamingMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Paragraph => "paragraph",
            Self::Off => "off",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Live => Self::Paragraph,
            Self::Paragraph => Self::Off,
            Self::Off => Self::Live,
        }
    }
}
