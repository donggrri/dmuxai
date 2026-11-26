//! Board classification rules (FR-3104).

use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::{BoardError, Kind, Lane};
use crate::run_journal::mask_sensitive_tokens;

const REGEX_SIZE_LIMIT: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_regex: Option<String>,
    pub lane: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardRules {
    #[serde(default)]
    pub rules: Vec<RuleEntry>,
    pub default_lane: String,
}

pub fn default_rules() -> BoardRules {
    BoardRules {
        rules: vec![
            RuleEntry {
                kind: Some("debug".to_owned()),
                grade: None,
                title_regex: None,
                lane: "auto".to_owned(),
            },
            RuleEntry {
                kind: Some("feature".to_owned()),
                grade: None,
                title_regex: None,
                lane: "needs_decision".to_owned(),
            },
            RuleEntry {
                kind: Some("question".to_owned()),
                grade: None,
                title_regex: None,
                lane: "needs_decision".to_owned(),
            },
            RuleEntry {
                kind: Some("review".to_owned()),
                grade: None,
                title_regex: None,
                lane: "needs_decision".to_owned(),
            },
            RuleEntry {
                kind: Some("other".to_owned()),
                grade: None,
                title_regex: None,
                lane: "needs_decision".to_owned(),
            },
        ],
        default_lane: "needs_decision".to_owned(),
    }
}

impl BoardRules {
    pub fn validate(&self) -> Result<(), BoardError> {
        Lane::parse(&self.default_lane)
            .ok_or_else(|| BoardError::Usage("invalid default_lane".to_owned()))?;
        for rule in &self.rules {
            Lane::parse(&rule.lane)
                .ok_or_else(|| BoardError::Usage("invalid rule lane".to_owned()))?;
            if let Some(kind) = &rule.kind
                && Kind::parse(kind).is_none()
            {
                return Err(BoardError::Usage(format!("invalid rule kind '{kind}'")));
            }
            if let Some(re) = &rule.title_regex {
                if re.len() > REGEX_SIZE_LIMIT {
                    return Err(BoardError::Usage("title_regex too large".to_owned()));
                }
                Regex::new(re)
                    .map_err(|e| BoardError::Usage(format!("invalid title_regex: {e}")))?;
            }
        }
        Ok(())
    }

    pub fn initial_lane(
        &self,
        kind: Kind,
        grade: Option<super::Grade>,
        title: &str,
    ) -> Result<Lane, BoardError> {
        let kind_str = kind.as_str();
        let grade_str = grade.map(grade_label);
        for rule in &self.rules {
            if rule.kind.as_ref().is_some_and(|k| k != kind_str) {
                continue;
            }
            if rule
                .grade
                .as_deref()
                .is_some_and(|g| grade_str != Some(g))
            {
                continue;
            }
            if let Some(re) = &rule.title_regex {
                let regex = Regex::new(re)
                    .map_err(|e| BoardError::Internal(format!("invalid stored regex: {e}")))?;
                if !regex.is_match(title) {
                    continue;
                }
            }
            return Lane::parse(&rule.lane)
                .ok_or_else(|| BoardError::Internal("invalid rule lane in file".to_owned()));
        }
        Lane::parse(&self.default_lane)
            .ok_or_else(|| BoardError::Internal("invalid default_lane in file".to_owned()))
    }
}

pub struct RulesStore {
    path: PathBuf,
}

impl RulesStore {
    pub fn new(base: &Path) -> Self {
        Self {
            path: base.join("rules.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> BoardRules {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => match serde_json::from_str::<BoardRules>(&text) {
                Ok(rules) if rules.validate().is_ok() => rules,
                _ => {
                    eprintln!(
                        "warning: invalid board rules at {}; using defaults",
                        self.path.display()
                    );
                    default_rules()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => default_rules(),
            Err(error) => {
                eprintln!(
                    "warning: could not read board rules ({}); using defaults",
                    error
                );
                default_rules()
            }
        }
    }

    pub fn save(&self, rules: &BoardRules) -> Result<(), BoardError> {
        rules.validate()?;
        let json =
            serde_json::to_string_pretty(rules).map_err(|e| BoardError::Internal(e.to_string()))?;
        if let Some(parent) = self.path.parent() {
            crate::board::store::ensure_dir(parent)?;
        }
        crate::board::store::atomic_write(&self.path, json.as_bytes())?;
        Ok(())
    }
}

fn grade_label(grade: super::Grade) -> &'static str {
    match grade {
        super::Grade::A => "A",
        super::Grade::B => "B",
        super::Grade::C => "C",
        super::Grade::D => "D",
    }
}

pub fn compile_rules_from_set_text(text: &str) -> Result<BoardRules, BoardError> {
    let rules: BoardRules = serde_json::from_str(text)
        .or_else(|_| serde_json::from_str(&mask_sensitive_tokens(text)))
        .map_err(|e| BoardError::Usage(format!("invalid rules JSON: {e}")))?;
    rules.validate()?;
    Ok(rules)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::store::ensure_dir;
    use tempfile::tempdir;

    #[test]
    fn tc_3104_rules_pick_initial_lane_defaults_and_invalid_file() {
        let rules = default_rules();
        assert_eq!(
            rules.initial_lane(Kind::Debug, None, "fix").unwrap(),
            Lane::Auto
        );
        assert_eq!(
            rules.initial_lane(Kind::Feature, None, "add").unwrap(),
            Lane::NeedsDecision
        );

        let dir = tempdir().unwrap();
        let base = dir.path().join("board");
        ensure_dir(&base).unwrap();
        let store = RulesStore::new(&base);
        std::fs::write(store.path(), b"{not json").unwrap();
        let loaded = store.load();
        assert_eq!(
            loaded.initial_lane(Kind::Debug, None, "x").unwrap(),
            Lane::Auto
        );

        let custom = BoardRules {
            rules: vec![RuleEntry {
                kind: Some("feature".to_owned()),
                grade: None,
                title_regex: Some("^urgent".to_owned()),
                lane: "auto".to_owned(),
            }],
            default_lane: "needs_decision".to_owned(),
        };
        store.save(&custom).unwrap();
        let loaded = store.load();
        assert_eq!(
            loaded
                .initial_lane(Kind::Feature, None, "urgent fix")
                .unwrap(),
            Lane::Auto
        );
        assert_eq!(
            loaded
                .initial_lane(Kind::Feature, None, "normal fix")
                .unwrap(),
            Lane::NeedsDecision
        );
    }
}
