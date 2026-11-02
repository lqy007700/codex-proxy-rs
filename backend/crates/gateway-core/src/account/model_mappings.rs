//! 账号级精确模型映射的校验与单步解析

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::validation::validate_text;

/// 已校验的账号映射；目标是最终上游 ID，不递归解释其他映射
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct AccountModelMappings(BTreeMap<String, String>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("modelMappings requires at most 256 exact model pairs (at most 256 bytes per ID)")]
pub struct InvalidAccountModelMappings;

impl AccountModelMappings {
    #[must_use]
    pub const fn empty() -> Self {
        Self(BTreeMap::new())
    }

    pub fn new(mappings: BTreeMap<String, String>) -> Result<Self, InvalidAccountModelMappings> {
        if mappings.len() > 256
            || mappings.iter().any(|(source, target)| {
                [source, target].into_iter().any(|model| {
                    model.trim() != model
                        || model.contains('*')
                        || validate_text(model, 256, true, None).is_err()
                })
            })
        {
            return Err(InvalidAccountModelMappings);
        }
        Ok(Self(mappings))
    }

    #[must_use]
    pub fn target(&self, requested: &str) -> Option<&str> {
        self.0.get(requested).map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(source, target)| (source.as_str(), target.as_str()))
    }
}

impl<'de> Deserialize<'de> for AccountModelMappings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(BTreeMap::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
