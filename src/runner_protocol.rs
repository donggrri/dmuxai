//! Runner integration protocol (SPEC-acp-runners FR-1305).

use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunnerProtocol {
    #[default]
    Acp,
    Legacy,
}

impl RunnerProtocol {
    pub fn parse_config(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "acp" => Some(Self::Acp),
            "legacy" => Some(Self::Legacy),
            _ => None,
        }
    }
}

impl FromStr for RunnerProtocol {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_config(s).ok_or(())
    }
}
