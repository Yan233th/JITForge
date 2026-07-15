use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_TOOL_NAME_LEN: usize = 64;
const MAX_DESCRIPTION_LEN: usize = 4_096;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ToolName(String);

impl ToolName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ToolName {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() || value.len() > MAX_TOOL_NAME_LEN {
            return Err(DomainError::InvalidToolName);
        }

        let mut characters = value.chars();
        let first = characters.next().ok_or(DomainError::InvalidToolName)?;
        if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
            return Err(DomainError::InvalidToolName);
        }

        if !characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || character == '_'
        }) {
            return Err(DomainError::InvalidToolName);
        }

        Ok(Self(value.to_owned()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolDescription(String);

impl ToolDescription {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ToolDescription {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        if value.is_empty() || value.len() > MAX_DESCRIPTION_LEN {
            return Err(DomainError::InvalidDescription);
        }
        Ok(Self(value.to_owned()))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolVersionStatus {
    Draft,
    ContractReady,
    Synthesizing,
    Building,
    Validating,
    Ready,
    Rejected,
    Deprecated,
    Revoked,
}

impl ToolVersionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::ContractReady => "contract_ready",
            Self::Synthesizing => "synthesizing",
            Self::Building => "building",
            Self::Validating => "validating",
            Self::Ready => "ready",
            Self::Rejected => "rejected",
            Self::Deprecated => "deprecated",
            Self::Revoked => "revoked",
        }
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Draft, Self::ContractReady)
                | (Self::ContractReady, Self::Synthesizing)
                | (Self::Synthesizing, Self::Building)
                | (Self::Synthesizing, Self::Rejected)
                | (Self::Building, Self::Validating)
                | (Self::Building, Self::Rejected)
                | (Self::Validating, Self::Ready)
                | (Self::Validating, Self::Rejected)
                | (Self::Ready, Self::Deprecated)
                | (Self::Ready, Self::Revoked)
                | (Self::Deprecated, Self::Revoked)
        )
    }
}

impl FromStr for ToolVersionStatus {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "draft" => Ok(Self::Draft),
            "contract_ready" => Ok(Self::ContractReady),
            "synthesizing" => Ok(Self::Synthesizing),
            "building" => Ok(Self::Building),
            "validating" => Ok(Self::Validating),
            "ready" => Ok(Self::Ready),
            "rejected" => Ok(Self::Rejected),
            "deprecated" => Ok(Self::Deprecated),
            "revoked" => Ok(Self::Revoked),
            _ => Err(DomainError::InvalidVersionStatus(value.to_owned())),
        }
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DomainError {
    #[error("tool name must be 1-64 characters of lowercase ASCII letters, digits, '-' or '_'")]
    InvalidToolName,

    #[error("description must contain 1-4096 non-whitespace bytes")]
    InvalidDescription,

    #[error("unknown tool version status {0:?}")]
    InvalidVersionStatus(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_unix_style_tool_names() {
        assert!("slugify".parse::<ToolName>().is_ok());
        assert!("json-to-csv_2".parse::<ToolName>().is_ok());
    }

    #[test]
    fn rejects_ambiguous_or_unsafe_tool_names() {
        for name in ["", "Slugify", "../slugify", "a/b", "-slugify", "a b"] {
            assert!(name.parse::<ToolName>().is_err(), "accepted {name:?}");
        }
    }

    #[test]
    fn permits_only_declared_state_transitions() {
        assert!(ToolVersionStatus::Draft.can_transition_to(ToolVersionStatus::ContractReady));
        assert!(!ToolVersionStatus::Draft.can_transition_to(ToolVersionStatus::Ready));
        assert!(!ToolVersionStatus::Revoked.can_transition_to(ToolVersionStatus::Ready));
    }
}
