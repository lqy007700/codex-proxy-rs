//! 账号 Fast 四态策略

use serde::{Deserialize, Serialize};

/// Fast 策略；默认保留客户端选择
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FastMode {
    #[default]
    Default,
    Enabled,
    Disabled,
    Ultrafast,
}

impl FastMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Enabled => "enabled",
            Self::Ultrafast => "ultrafast",
            Self::Disabled => "disabled",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "default" => Some(Self::Default),
            "enabled" => Some(Self::Enabled),
            "ultrafast" => Some(Self::Ultrafast),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}
