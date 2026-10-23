//! Pure module for agent system prompts (SPEC PR-2.5 part 1).
//!
//! Provides types, hash calculation, delivery strategy detection, instruction block formatting,
//! injection decision logic, and pure prompt resolution from [`crate::settings::DmuxSettings`].

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::custom_agent::CustomAgentSpec;
use crate::settings::Settings;

/// Maximum allowed length of a system prompt in bytes (32 KiB = 32,768 bytes).
pub const MAX_PROMPT_BYTES: usize = 32_768;

/// Injection cadence for the prepended system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptInject {
    /// Injected on fresh sessions, or resumed sessions when prompt hash changed.
    #[default]
    Session,
    /// Injected on every prompt turn.
    Always,
}

impl PromptInject {
    /// Parses `prompt_inject` from a config string (`"session"` or `"always"`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "session" => Some(Self::Session),
            "always" => Some(Self::Always),
            _ => None,
        }
    }
}

/// Prompt role representing the context in which the agent is invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptRole {
    Chat,
    Worker,
    Judge,
    Triage,
    Planner,
    Replan,
    Summary,
    Interview,
}

impl PromptRole {
    /// Parses a role name from string.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "chat" => Some(Self::Chat),
            "worker" => Some(Self::Worker),
            "judge" => Some(Self::Judge),
            "triage" => Some(Self::Triage),
            "planner" => Some(Self::Planner),
            "replan" => Some(Self::Replan),
            "summary" => Some(Self::Summary),
            "interview" => Some(Self::Interview),
            _ => None,
        }
    }

    /// Whether this role is allowed in `prompt_roles` configuration.
    /// Only `chat` and `worker` are allowed.
    pub fn is_configurable_role(&self) -> bool {
        matches!(self, Self::Chat | Self::Worker)
    }
}

/// A resolved prompt ready to be delivered to an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPrompt {
    pub text: String,
    pub hash: String,
    pub source: String,
    pub inject: PromptInject,
}

/// Prompt text paired with its provenance label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTextWithSource {
    pub text: String,
    pub source: String,
}

/// Parsed configuration for a single runner table (`[runners.default]` or `[runners.<id>]`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunnerPromptConfig {
    pub prompt: Option<PromptTextWithSource>,
    pub inject: Option<PromptInject>,
    pub roles: Option<Vec<PromptRole>>,
    pub model: Option<String>,
}

/// Raw TOML table under `[runners.<id>]`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
pub struct RunnerPromptToml {
    pub system_prompt: Option<String>,
    pub system_prompt_file: Option<String>,
    pub prompt_inject: Option<String>,
    pub prompt_roles: Option<Vec<String>>,
    pub model: Option<String>,
}

/// Delivery mechanism for the system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Native,
    Prepend,
}

/// Determines the delivery mechanism for a runner.
///
/// Native for opencode and codex; Prepend for cursor, agy, grok and generic custom agents.
/// Keep a hook so it can return Native when a custom spec opts in (later part).
pub fn delivery(runner: &str, custom_spec: Option<&CustomAgentSpec>) -> Delivery {
    let lower = runner.trim().to_ascii_lowercase();
    match lower.as_str() {
        "opencode" | "codex" => Delivery::Native,
        "cursor" | "agy" | "grok" => Delivery::Prepend,
        _ => {
            if let Some(spec) = custom_spec {
                let contains =
                    |v: &str| v.contains("{system_prompt}") || v.contains("{system_prompt_file}");
                let opted = !spec.system_prompt_arg.is_empty()
                    || spec.command.iter().any(|v| contains(v))
                    || spec.env.values().any(|v| contains(v))
                    || spec
                        .temp_config_template
                        .as_ref()
                        .is_some_and(|t| contains(t));
                if opted {
                    Delivery::Native
                } else {
                    Delivery::Prepend
                }
            } else {
                Delivery::Prepend
            }
        }
    }
}

/// Convenience delivery lookup without a custom spec.
pub fn delivery_for_runner(runner: &str) -> Delivery {
    delivery(runner, None)
}

/// Formats text into the standard prepended block:
/// `<agent-instructions>\n{text}\n</agent-instructions>\n\n`
pub fn prepend_block(text: &str) -> String {
    format!("<agent-instructions>\n{text}\n</agent-instructions>\n\n")
}

/// Truth table for deciding whether to inject the system prompt block.
///
/// Always -> true;
/// Session -> fresh_session || last_hash != Some(hash).
pub fn should_inject(
    inject: PromptInject,
    fresh_session: bool,
    last_hash: Option<&str>,
    hash: &str,
) -> bool {
    match inject {
        PromptInject::Always => true,
        PromptInject::Session => fresh_session || last_hash != Some(hash),
    }
}

/// Stable short 16-hex sha256 hash of prompt text.
pub fn hash_prompt(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(16);
    for b in &digest[..8] {
        let _ = write!(out, "{b:02x}");
    }
    out
}

impl RunnerPromptConfig {
    /// Parses a single raw `[runners.<id>]` table, validating keys and emitting warnings.
    /// File reading for `system_prompt_file` happens here using `base_dir`.
    pub fn parse_table(
        id: &str,
        toml: &RunnerPromptToml,
        scope: &str,
        base_dir: &Path,
        warnings: &mut Vec<String>,
    ) -> Self {
        let mut config = Self::default();

        // 1. Prompt text or file
        if toml.system_prompt.is_some() && toml.system_prompt_file.is_some() {
            warnings.push(format!(
                "[runners.{id}] both system_prompt and system_prompt_file are configured; prompt ignored"
            ));
        } else if let Some(ref inline) = toml.system_prompt {
            if inline.len() > MAX_PROMPT_BYTES {
                warnings.push(format!(
                    "[runners.{id}] system_prompt exceeds 32 KiB (32768 bytes) limit ({} bytes); prompt ignored",
                    inline.len()
                ));
            } else {
                config.prompt = Some(PromptTextWithSource {
                    text: inline.clone(),
                    source: format!("{scope}:[runners.{id}]"),
                });
            }
        } else if let Some(ref file_rel) = toml.system_prompt_file {
            let file_path = Path::new(file_rel);
            let full_path = if file_path.is_absolute() {
                file_path.to_path_buf()
            } else {
                base_dir.join(file_path)
            };
            match std::fs::read_to_string(&full_path) {
                Ok(text) => {
                    if text.len() > MAX_PROMPT_BYTES {
                        warnings.push(format!(
                            "[runners.{id}] system_prompt_file '{file_rel}' exceeds 32 KiB (32768 bytes) limit ({} bytes); prompt ignored",
                            text.len()
                        ));
                    } else {
                        config.prompt = Some(PromptTextWithSource {
                            text,
                            source: format!("{scope}:[runners.{id}] ({file_rel})"),
                        });
                    }
                }
                Err(err) => {
                    warnings.push(format!(
                        "[runners.{id}] failed to read system_prompt_file '{file_rel}': {err}; prompt ignored"
                    ));
                }
            }
        }

        // 2. Prompt inject
        if let Some(ref raw_inject) = toml.prompt_inject {
            match PromptInject::parse(raw_inject) {
                Some(inject) => config.inject = Some(inject),
                None => {
                    warnings.push(format!(
                        "[runners.{id}] unknown prompt_inject '{raw_inject}'; using default 'session'"
                    ));
                    config.inject = Some(PromptInject::Session);
                }
            }
        }

        // 3. Prompt roles
        if let Some(ref raw_roles) = toml.prompt_roles {
            let mut roles = Vec::new();
            for r in raw_roles {
                let trimmed = r.trim().to_ascii_lowercase();
                match trimmed.as_str() {
                    "chat" => {
                        if !roles.contains(&PromptRole::Chat) {
                            roles.push(PromptRole::Chat);
                        }
                    }
                    "worker" => {
                        if !roles.contains(&PromptRole::Worker) {
                            roles.push(PromptRole::Worker);
                        }
                    }
                    "judge" | "triage" | "planner" | "replan" | "summary" | "interview" => {
                        warnings.push(format!(
                            "[runners.{id}] role '{r}' in prompt_roles cannot receive system prompts; role dropped"
                        ));
                    }
                    _ => {
                        warnings.push(format!(
                            "[runners.{id}] unknown role '{r}' in prompt_roles ignored"
                        ));
                    }
                }
            }
            config.roles = Some(roles);
        }

        // 4. Per-runner model profile (FR-1731). Validation of the model id is
        // deferred to settings load, where custom agents are already known.
        if let Some(ref raw_model) = toml.model {
            let trimmed = raw_model.trim();
            if trimmed.is_empty() {
                // ignore empty
            } else if id.trim().eq_ignore_ascii_case("default") {
                warnings.push(
                    "[runners.default] model is not allowed under [runners.default]; ignored"
                        .to_owned(),
                );
            } else {
                config.model = Some(trimmed.to_owned());
            }
        }

        config
    }
}

/// Merges runner prompt configurations: workspace overrides user per key, per table.
pub fn merge_runner_prompts(
    user_prompts: Option<BTreeMap<String, RunnerPromptConfig>>,
    ws_prompts: Option<BTreeMap<String, RunnerPromptConfig>>,
) -> BTreeMap<String, RunnerPromptConfig> {
    let mut merged = user_prompts.unwrap_or_default();
    if let Some(workspace) = ws_prompts {
        for (id, ws_config) in workspace {
            let entry = merged.entry(id).or_default();
            if ws_config.prompt.is_some() {
                entry.prompt = ws_config.prompt;
            }
            if ws_config.inject.is_some() {
                entry.inject = ws_config.inject;
            }
            if ws_config.roles.is_some() {
                entry.roles = ws_config.roles;
            }
            if ws_config.model.is_some() {
                entry.model = ws_config.model;
            }
        }
    }
    merged
}

/// Pure resolver for a runner's system prompt given settings, runner ID, and invocation role.
///
/// Returns None when:
/// - No prompt configured (neither for the runner nor in `default`)
/// - Prompt text is empty or whitespace
/// - Role is not in `prompt_roles` (roles Judge, Triage, Planner, Replan, Summary, Interview always return None)
pub fn resolve(settings: &Settings, runner: &str, role: PromptRole) -> Option<ResolvedPrompt> {
    // 1. Disallowed roles never receive system prompts
    match role {
        PromptRole::Chat | PromptRole::Worker => {}
        PromptRole::Judge
        | PromptRole::Triage
        | PromptRole::Planner
        | PromptRole::Replan
        | PromptRole::Summary
        | PromptRole::Interview => return None,
    }

    let runner_id = runner.trim().to_ascii_lowercase();
    let specific_config = settings.runner_prompts.get(&runner_id);
    let default_config = settings.runner_prompts.get("default");

    // 2. Resolve prompt text and source: specific overrides default
    let prompt = specific_config
        .and_then(|c| c.prompt.as_ref())
        .or_else(|| default_config.and_then(|c| c.prompt.as_ref()))?;

    // None when empty or whitespace
    if prompt.text.trim().is_empty() {
        return None;
    }

    // 3. Resolve roles: specific overrides default, defaults to [Chat, Worker]
    let roles = specific_config
        .and_then(|c| c.roles.as_ref())
        .or_else(|| default_config.and_then(|c| c.roles.as_ref()));

    let role_allowed = match roles {
        Some(list) => list.contains(&role),
        None => matches!(role, PromptRole::Chat | PromptRole::Worker),
    };

    if !role_allowed {
        return None;
    }

    // 4. Resolve inject: specific overrides default, defaults to Session
    let inject = specific_config
        .and_then(|c| c.inject)
        .or_else(|| default_config.and_then(|c| c.inject))
        .unwrap_or(PromptInject::Session);

    let hash = hash_prompt(&prompt.text);

    Some(ResolvedPrompt {
        text: prompt.text.clone(),
        hash,
        source: prompt.source.clone(),
        inject,
    })
}

/// Edits `[runners.<runner>]` in a dmuxai.toml text, keeping the rest of the file.
/// `system_prompt`: None => leave unchanged; Some(None) => remove; Some(Some(t)) => set.
/// `prompt_inject`: same convention.
pub fn set_runner_prompt_in_toml(
    text: &str,
    runner: &str,
    system_prompt: Option<Option<&str>>,
    prompt_inject: Option<Option<&str>>,
) -> Result<String, String> {
    let mut doc: toml::Value = if text.trim().is_empty() {
        toml::Value::Table(Default::default())
    } else {
        toml::from_str(text).map_err(|err| err.to_string())?
    };
    let root = doc
        .as_table_mut()
        .ok_or("dmuxai.toml root is not a table")?;
    let runners = root
        .entry("runners")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or("[runners] is not a table")?;
    let entry = runners
        .entry(runner.to_owned())
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or("runner entry is not a table")?;

    if let Some(sp) = system_prompt {
        match sp {
            Some(value) => {
                entry.insert(
                    "system_prompt".to_owned(),
                    toml::Value::String(value.to_owned()),
                );
            }
            None => {
                entry.remove("system_prompt");
            }
        }
    }
    if let Some(pi) = prompt_inject {
        match pi {
            Some(value) => {
                entry.insert(
                    "prompt_inject".to_owned(),
                    toml::Value::String(value.to_owned()),
                );
            }
            None => {
                entry.remove("prompt_inject");
            }
        }
    }
    toml::to_string(&doc).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // TC-1714 (FR-1704): delivery per runner
    #[test]
    fn agent_prompt_delivery_per_runner() {
        assert_eq!(delivery("opencode", None), Delivery::Native);
        assert_eq!(delivery("OpenCode", None), Delivery::Native);
        assert_eq!(delivery("codex", None), Delivery::Native);
        assert_eq!(delivery("Codex", None), Delivery::Native);
        assert_eq!(delivery("cursor", None), Delivery::Prepend);
        assert_eq!(delivery("Cursor", None), Delivery::Prepend);
        assert_eq!(delivery("agy", None), Delivery::Prepend);
        assert_eq!(delivery("grok", None), Delivery::Prepend);
        assert_eq!(delivery("generic-agent", None), Delivery::Prepend);
        assert_eq!(delivery_for_runner("codex"), Delivery::Native);
        assert_eq!(delivery_for_runner("cursor"), Delivery::Prepend);

        let custom_spec = CustomAgentSpec {
            name: "custom".to_owned(),
            label: None,
            color: None,
            command: vec!["custom-binary".to_owned()],
            prompt_via: Default::default(),
            image_arg: vec![],
            default_model: None,
            models_command: vec![],
            models_filter: None,
            allowed_models: None,
            output: None,
            text_path: None,
            text_when: None,
            error_path: None,
            session_id_path: None,
            resume_arg: vec![],
            timeout_secs: None,
            failure_stderr: None,
            temp_config_template: None,
            system_prompt_arg: vec![],
            env: BTreeMap::new(),
            modes: Default::default(),
            done_when: None,
            text_break_when: None,
            builtin: false,
        };
        assert_eq!(delivery("custom", Some(&custom_spec)), Delivery::Prepend);
    }

    // TC-1714 (FR-1704): prepend block format
    #[test]
    fn agent_prompt_prepend_block_format() {
        let text = "Follow these instructions carefully.";
        let block = prepend_block(text);
        assert_eq!(
            block,
            "<agent-instructions>\nFollow these instructions carefully.\n</agent-instructions>\n\n"
        );
    }

    // TC-1713 (FR-1701): should_inject truth table
    #[test]
    fn agent_prompt_should_inject_truth_table() {
        let hash = "abc1234567890def";
        let other = "fed0987654321cba";

        // Always: always returns true regardless of session freshness or last hash
        assert!(should_inject(PromptInject::Always, true, None, hash));
        assert!(should_inject(PromptInject::Always, true, Some(hash), hash));
        assert!(should_inject(PromptInject::Always, false, Some(hash), hash));
        assert!(should_inject(
            PromptInject::Always,
            false,
            Some(other),
            hash
        ));
        assert!(should_inject(PromptInject::Always, false, None, hash));

        // Session: true if fresh_session, or if last_hash != Some(hash)
        assert!(should_inject(PromptInject::Session, true, None, hash));
        assert!(should_inject(PromptInject::Session, true, Some(hash), hash));
        assert!(should_inject(
            PromptInject::Session,
            true,
            Some(other),
            hash
        ));
        assert!(should_inject(PromptInject::Session, false, None, hash));
        assert!(should_inject(
            PromptInject::Session,
            false,
            Some(other),
            hash
        ));
        // Session: false only when resumed with matching hash
        assert!(!should_inject(
            PromptInject::Session,
            false,
            Some(hash),
            hash
        ));
    }

    // TC-1713 (FR-1701): hash stable
    #[test]
    fn agent_prompt_hash_stable_and_changes_with_text() {
        let h1 = hash_prompt("Instructions version 1");
        let h1_repeat = hash_prompt("Instructions version 1");
        let h2 = hash_prompt("Instructions version 2");

        assert_eq!(h1, h1_repeat, "hash must be deterministic and stable");
        assert_ne!(h1, h2, "hash must change when prompt text changes");
        assert_eq!(h1.len(), 16, "hash must be exactly 16 hex characters");
        assert!(
            h1.chars().all(|c| c.is_ascii_hexdigit()),
            "hash must consist of hex digits"
        );
    }

    // TC-1712 (FR-1702): resolve none unconfigured
    #[test]
    fn agent_prompt_resolve_none_for_unconfigured_runner() {
        let settings = Settings::default();
        assert!(resolve(&settings, "cursor", PromptRole::Chat).is_none());
        assert!(resolve(&settings, "codex", PromptRole::Worker).is_none());
    }

    // TC-1712 (FR-1702): resolve empty returns none
    #[test]
    fn agent_prompt_resolve_empty_or_whitespace_returns_none() {
        let mut settings = Settings::default();
        settings.runner_prompts.insert(
            "cursor".to_owned(),
            RunnerPromptConfig {
                prompt: Some(PromptTextWithSource {
                    text: "   \n\t  ".to_owned(),
                    source: "workspace:[runners.cursor]".to_owned(),
                }),
                inject: None,
                roles: None,
                model: None,
            },
        );
        assert!(resolve(&settings, "cursor", PromptRole::Chat).is_none());
    }

    // TC-1712 (FR-1702): excluded roles none
    #[test]
    fn agent_prompt_excluded_roles_always_return_none() {
        let mut settings = Settings::default();
        settings.runner_prompts.insert(
            "default".to_owned(),
            RunnerPromptConfig {
                prompt: Some(PromptTextWithSource {
                    text: "Valid prompt".to_owned(),
                    source: "workspace:[runners.default]".to_owned(),
                }),
                inject: None,
                roles: None,
                model: None,
            },
        );

        // Chat and Worker succeed
        assert!(resolve(&settings, "cursor", PromptRole::Chat).is_some());
        assert!(resolve(&settings, "cursor", PromptRole::Worker).is_some());

        // All excluded roles return None unconditionally
        assert!(resolve(&settings, "cursor", PromptRole::Judge).is_none());
        assert!(resolve(&settings, "cursor", PromptRole::Triage).is_none());
        assert!(resolve(&settings, "cursor", PromptRole::Planner).is_none());
        assert!(resolve(&settings, "cursor", PromptRole::Replan).is_none());
        assert!(resolve(&settings, "cursor", PromptRole::Summary).is_none());
        assert!(resolve(&settings, "cursor", PromptRole::Interview).is_none());
    }
    // TC-1714 (FR-1704): native opted-in custom
    #[test]
    fn agent_prompt_delivery_native_for_opted_in_custom() {
        use crate::custom_agent::CustomAgentSpec;
        // system_prompt_arg opts in
        let spec = CustomAgentSpec {
            name: "custom".to_owned(),
            label: None,
            color: None,
            command: vec!["custom-binary".to_owned()],
            prompt_via: Default::default(),
            image_arg: vec![],
            default_model: None,
            models_command: vec![],
            models_filter: None,
            allowed_models: None,
            output: None,
            text_path: None,
            text_when: None,
            error_path: None,
            session_id_path: None,
            resume_arg: vec![],
            timeout_secs: None,
            failure_stderr: None,
            temp_config_template: None,
            system_prompt_arg: vec!["--system".to_owned(), "{system_prompt}".to_owned()],
            env: BTreeMap::new(),
            modes: Default::default(),
            done_when: None,
            text_break_when: None,
            builtin: false,
        };
        assert_eq!(delivery("custom", Some(&spec)), Delivery::Native);

        // placeholder in command opts in
        let spec2 = CustomAgentSpec {
            system_prompt_arg: vec![],
            command: vec![
                "custom-binary".to_owned(),
                "{system_prompt_file}".to_owned(),
            ],
            ..spec.clone()
        };
        // fix name? clone ok
        assert_eq!(delivery("custom", Some(&spec2)), Delivery::Native);

        // plain remains prepend
        let plain = CustomAgentSpec {
            name: "plain".to_owned(),
            command: vec!["plain".to_owned()],
            ..Default::default()
        };
        assert_eq!(delivery("plain", Some(&plain)), Delivery::Prepend);
    }
}
