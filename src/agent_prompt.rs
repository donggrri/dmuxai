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

/// Built-in instructions for AGY/Gemini Flash agent turns.
pub const BUILTIN_AGY_PROMPT: &str = include_str!("prompts/agy-gemini-flash.md");

/// Built-in instructions for Cursor composer-2.5 agent turns.
pub const BUILTIN_COMPOSER_PROMPT: &str = include_str!("prompts/cursor-composer-2.5.md");

/// Built-in instructions for Cursor grok-4.7 agent turns.
pub const BUILTIN_CURSOR_GROK_PROMPT: &str = include_str!("prompts/cursor-grok-4.7.md");

/// Built-in instructions for Grok Build agent turns.
pub const BUILTIN_GROK_BUILD_PROMPT: &str = include_str!("prompts/grok-build.md");

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

/// Parsed prompt configuration for one `[runners.<id>.models."<model>"]` table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelPromptConfig {
    pub prompt: Option<PromptTextWithSource>,
    pub inject: Option<PromptInject>,
}

/// Parsed configuration for a single runner table (`[runners.default]` or `[runners.<id>]`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunnerPromptConfig {
    pub prompt: Option<PromptTextWithSource>,
    pub inject: Option<PromptInject>,
    pub roles: Option<Vec<PromptRole>>,
    pub model: Option<String>,
    /// Per-model prompt entries keyed by trimmed lowercase model id.
    pub models: Option<BTreeMap<String, ModelPromptConfig>>,
    /// Whether built-in prompts may be used for this runner (default true).
    pub builtin_prompt: Option<bool>,
}

/// Raw TOML table under `[runners.<id>.models."<model>"]`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
pub struct RunnerModelPromptToml {
    pub system_prompt: Option<String>,
    pub system_prompt_file: Option<String>,
    pub prompt_inject: Option<String>,
}

/// Raw TOML table under `[runners.<id>]`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
pub struct RunnerPromptToml {
    pub system_prompt: Option<String>,
    pub system_prompt_file: Option<String>,
    pub prompt_inject: Option<String>,
    pub prompt_roles: Option<Vec<String>>,
    pub model: Option<String>,
    pub fallback: Option<Vec<String>>,
    pub stall_timeout_secs: Option<u64>,
    pub stall_api_retries: Option<u32>,
    pub models: Option<BTreeMap<String, RunnerModelPromptToml>>,
    pub builtin_prompt: Option<bool>,
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

/// Parses inline/file prompt text and `prompt_inject` for one table.
///
/// `table_path` is the TOML path without brackets (for warnings and the source
/// label), and `scope` is `user` or `workspace`.
fn parse_prompt_entry(
    table_path: &str,
    scope: &str,
    system_prompt: Option<&String>,
    system_prompt_file: Option<&String>,
    prompt_inject: Option<&String>,
    base_dir: &Path,
    warnings: &mut Vec<String>,
) -> (Option<PromptTextWithSource>, Option<PromptInject>) {
    let mut prompt = None;
    let mut inject = None;

    // 1. Prompt text or file
    if system_prompt.is_some() && system_prompt_file.is_some() {
        warnings.push(format!(
            "[{table_path}] both system_prompt and system_prompt_file are configured; prompt ignored"
        ));
    } else if let Some(inline) = system_prompt {
        if inline.len() > MAX_PROMPT_BYTES {
            warnings.push(format!(
                "[{table_path}] system_prompt exceeds 32 KiB (32768 bytes) limit ({} bytes); prompt ignored",
                inline.len()
            ));
        } else {
            prompt = Some(PromptTextWithSource {
                text: inline.clone(),
                source: format!("{scope}:[{table_path}]"),
            });
        }
    } else if let Some(file_rel) = system_prompt_file {
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
                        "[{table_path}] system_prompt_file '{file_rel}' exceeds 32 KiB (32768 bytes) limit ({} bytes); prompt ignored",
                        text.len()
                    ));
                } else {
                    prompt = Some(PromptTextWithSource {
                        text,
                        source: format!("{scope}:[{table_path}] ({file_rel})"),
                    });
                }
            }
            Err(err) => {
                warnings.push(format!(
                    "[{table_path}] failed to read system_prompt_file '{file_rel}': {err}; prompt ignored"
                ));
            }
        }
    }

    // 2. Prompt inject
    if let Some(raw_inject) = prompt_inject {
        match PromptInject::parse(raw_inject) {
            Some(parsed) => inject = Some(parsed),
            None => {
                warnings.push(format!(
                    "[{table_path}] unknown prompt_inject '{raw_inject}'; using default 'session'"
                ));
                inject = Some(PromptInject::Session);
            }
        }
    }

    (prompt, inject)
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
        let table_path = format!("runners.{id}");
        let (prompt, inject) = parse_prompt_entry(
            &table_path,
            scope,
            toml.system_prompt.as_ref(),
            toml.system_prompt_file.as_ref(),
            toml.prompt_inject.as_ref(),
            base_dir,
            warnings,
        );
        config.prompt = prompt;
        config.inject = inject;

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

        // 5. Built-in prompt switch (FR-2151).
        config.builtin_prompt = toml.builtin_prompt;

        // 6. Per-model prompt tables (FR-2150).
        if let Some(ref models) = toml.models {
            if id.trim().eq_ignore_ascii_case("default") {
                if !models.is_empty() {
                    warnings.push(
                        "[runners.default.models] is not allowed under [runners.default]; ignored"
                            .to_owned(),
                    );
                }
            } else {
                let mut parsed = BTreeMap::new();
                for (raw_model, raw) in models {
                    let model = raw_model.trim();
                    if model.is_empty() {
                        warnings.push(format!(
                            "[runners.{id}] models entry with an empty model id ignored"
                        ));
                        continue;
                    }
                    let model_path = format!("runners.{id}.models.\"{model}\"");
                    let (prompt, inject) = parse_prompt_entry(
                        &model_path,
                        scope,
                        raw.system_prompt.as_ref(),
                        raw.system_prompt_file.as_ref(),
                        raw.prompt_inject.as_ref(),
                        base_dir,
                        warnings,
                    );
                    if prompt.is_some() || inject.is_some() {
                        parsed.insert(
                            model.to_ascii_lowercase(),
                            ModelPromptConfig { prompt, inject },
                        );
                    }
                }
                config.models = Some(parsed);
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
            let RunnerPromptConfig {
                prompt,
                inject,
                roles,
                model,
                models,
                builtin_prompt,
            } = ws_config;
            let entry = merged.entry(id).or_default();
            if prompt.is_some() {
                entry.prompt = prompt;
            }
            if inject.is_some() {
                entry.inject = inject;
            }
            if roles.is_some() {
                entry.roles = roles;
            }
            if model.is_some() {
                entry.model = model;
            }
            if builtin_prompt.is_some() {
                entry.builtin_prompt = builtin_prompt;
            }
            if let Some(ws_models) = models {
                let models = entry.models.get_or_insert_with(BTreeMap::new);
                for (model_key, ws_model) in ws_models {
                    let model_entry = models.entry(model_key).or_default();
                    if ws_model.prompt.is_some() {
                        model_entry.prompt = ws_model.prompt;
                    }
                    if ws_model.inject.is_some() {
                        model_entry.inject = ws_model.inject;
                    }
                }
            }
        }
    }
    merged
}

/// Built-in prompt selected by model id (FR-2152, FR-2160).
fn builtin_prompt_for_model(
    runner: &str,
    model: Option<&str>,
) -> Option<(&'static str, &'static str)> {
    let model = model?.trim().to_ascii_lowercase();
    if model.contains("fast") {
        return None;
    }
    if runner.trim().eq_ignore_ascii_case("cursor")
        && matches!(
            model.as_str(),
            "grok-4.7" | "grok-4.7-low" | "grok-4.7-medium" | "grok-4.7-high" | "grok-4.7-xhigh"
        )
    {
        Some((BUILTIN_CURSOR_GROK_PROMPT, "builtin:model:cursor:grok-4.7"))
    } else if model.starts_with("gemini-") {
        Some((BUILTIN_AGY_PROMPT, "builtin:model:gemini"))
    } else if model == "composer-2.5" {
        Some((BUILTIN_COMPOSER_PROMPT, "builtin:model:composer-2.5"))
    } else {
        None
    }
}

/// Built-in prompt selected by runner id (FR-2152, FR-2161).
fn builtin_prompt_for_runner(runner: &str) -> Option<(&'static str, &'static str)> {
    match runner.trim().to_ascii_lowercase().as_str() {
        "agy" => Some((BUILTIN_AGY_PROMPT, "builtin:runner:agy")),
        "cursor" => Some((BUILTIN_COMPOSER_PROMPT, "builtin:runner:cursor")),
        "grok" => Some((BUILTIN_GROK_BUILD_PROMPT, "builtin:runner:grok")),
        _ => None,
    }
}

/// Pure resolver for a runner's system prompt given settings, runner ID, and invocation role.
///
/// This preserves the config-only behaviour used by existing callers: no
/// per-model lookup and no built-in prompts.
///
/// Returns None when:
/// - No prompt configured (neither for the runner nor in `default`)
/// - Prompt text is empty or whitespace
/// - Role is not in `prompt_roles` (roles Judge, Triage, Planner, Replan, Summary, Interview always return None)
pub fn resolve(settings: &Settings, runner: &str, role: PromptRole) -> Option<ResolvedPrompt> {
    resolve_for(settings, runner, None, role, false)
}

/// Resolves the system prompt for a turn (FR-2150..FR-2155).
///
/// Precedence, first non-empty wins:
/// 1. `[runners.<runner>.models."<model>"]`
/// 2. `[runners.<runner>]`
/// 3. built-in by model (Gemini → AGY, composer-2.5 → Cursor)
/// 4. built-in by runner (agy / cursor)
/// 5. `[runners.default]`
///
/// Built-ins are used only for write (`TurnPolicy::Agent`) turns and only when
/// the effective `builtin_prompt` setting is true.
pub fn resolve_for(
    settings: &Settings,
    runner: &str,
    model: Option<&str>,
    role: PromptRole,
    write_turn: bool,
) -> Option<ResolvedPrompt> {
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

    // 2. Resolve roles: specific overrides default, defaults to [Chat, Worker].
    // The rule is the same at every prompt level.
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

    let model_key = model
        .map(|m| m.trim().to_ascii_lowercase())
        .filter(|m| !m.is_empty());
    let model_config = model_key.as_ref().and_then(|key| {
        specific_config
            .and_then(|c| c.models.as_ref())
            .and_then(|models| models.get(key))
    });

    let effective_builtin = specific_config
        .and_then(|c| c.builtin_prompt)
        .or_else(|| default_config.and_then(|c| c.builtin_prompt))
        .unwrap_or(true);

    let builtin_model = if write_turn && effective_builtin {
        builtin_prompt_for_model(&runner_id, model_key.as_deref())
    } else {
        None
    };
    let builtin_runner = if write_turn && effective_builtin {
        builtin_prompt_for_runner(&runner_id)
    } else {
        None
    };

    // 3. First prompt in resolution order. An explicitly configured but
    // empty/whitespace prompt at the model or runner level means "no prompt"
    // and stops resolution (it must not fall through to built-ins or default).
    let config_model_prompt = model_config.and_then(|c| c.prompt.as_ref());
    let config_runner_prompt = specific_config.and_then(|c| c.prompt.as_ref());
    let config_default_prompt = default_config.and_then(|c| c.prompt.as_ref());

    let (text, source, from_builtin) = if let Some(prompt) = config_model_prompt {
        if prompt.text.trim().is_empty() {
            return None;
        }
        (prompt.text.clone(), prompt.source.clone(), false)
    } else if let Some(prompt) = config_runner_prompt {
        if prompt.text.trim().is_empty() {
            return None;
        }
        (prompt.text.clone(), prompt.source.clone(), false)
    } else if let Some((text, source)) = builtin_model {
        (text.to_owned(), source.to_owned(), true)
    } else if let Some((text, source)) = builtin_runner {
        (text.to_owned(), source.to_owned(), true)
    } else {
        let prompt = config_default_prompt?;
        if prompt.text.trim().is_empty() {
            return None;
        }
        (prompt.text.clone(), prompt.source.clone(), false)
    };

    // 4. Resolve inject: model entry, runner table, default, then Session.
    // Built-ins skip the per-model entry and use the runner/default value.
    let inject = if from_builtin {
        specific_config
            .and_then(|c| c.inject)
            .or_else(|| default_config.and_then(|c| c.inject))
            .unwrap_or(PromptInject::Session)
    } else {
        model_config
            .and_then(|c| c.inject)
            .or_else(|| specific_config.and_then(|c| c.inject))
            .or_else(|| default_config.and_then(|c| c.inject))
            .unwrap_or(PromptInject::Session)
    };

    let hash = hash_prompt(&text);

    Some(ResolvedPrompt {
        text,
        hash,
        source,
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
            idle_timeout_secs: None,
            failure_stderr: None,
            stall_keepalive: None,
            temp_config_template: None,
            system_prompt_arg: vec![],
            env: BTreeMap::new(),
            modes: Default::default(),
            done_when: None,
            text_break_when: None,
            tool_when: None,
            tool_name_path: None,
            tool_title_path: None,
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
                models: None,
                builtin_prompt: None,
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
                models: None,
                builtin_prompt: None,
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
    // TC-2152 / FR-2152: resolution order across model, runner, built-in, default.
    #[test]
    fn tc_2152_resolve_order_model_runner_builtin_default() {
        let model_prompt = PromptTextWithSource {
            text: "model prompt".to_owned(),
            source: "workspace:[runners.cursor.models.\"composer-2.5\"]".to_owned(),
        };
        let runner_prompt = PromptTextWithSource {
            text: "runner prompt".to_owned(),
            source: "workspace:[runners.cursor]".to_owned(),
        };
        let default_prompt = PromptTextWithSource {
            text: "default prompt".to_owned(),
            source: "workspace:[runners.default]".to_owned(),
        };

        let mut settings = Settings::default();
        settings.runner_prompts.insert(
            "cursor".to_owned(),
            RunnerPromptConfig {
                prompt: Some(runner_prompt.clone()),
                inject: None,
                roles: None,
                model: None,
                models: Some(BTreeMap::from([(
                    "composer-2.5".to_owned(),
                    ModelPromptConfig {
                        prompt: Some(model_prompt.clone()),
                        inject: None,
                    },
                )])),
                builtin_prompt: None,
            },
        );
        settings.runner_prompts.insert(
            "default".to_owned(),
            RunnerPromptConfig {
                prompt: Some(default_prompt.clone()),
                inject: None,
                roles: None,
                model: None,
                models: None,
                builtin_prompt: None,
            },
        );

        // Model entry beats the runner table.
        let resolved = resolve_for(
            &settings,
            "cursor",
            Some("Composer-2.5"),
            PromptRole::Chat,
            true,
        )
        .expect("model prompt");
        assert_eq!(resolved.text, "model prompt");
        assert_eq!(
            resolved.source,
            "workspace:[runners.cursor.models.\"composer-2.5\"]"
        );

        // Runner table beats the built-in and default.
        let resolved = resolve_for(&settings, "cursor", Some("other"), PromptRole::Chat, true)
            .expect("runner prompt");
        assert_eq!(resolved.text, "runner prompt");

        // Built-in beats default.
        let mut builtin_settings = Settings::default();
        builtin_settings.runner_prompts.insert(
            "default".to_owned(),
            RunnerPromptConfig {
                prompt: Some(default_prompt),
                inject: None,
                roles: None,
                model: None,
                models: None,
                builtin_prompt: None,
            },
        );
        let resolved = resolve_for(&builtin_settings, "cursor", None, PromptRole::Chat, true)
            .expect("built-in cursor prompt");
        assert_eq!(resolved.text, BUILTIN_COMPOSER_PROMPT);
        assert_eq!(resolved.source, "builtin:runner:cursor");

        // Built-in by model beats built-in by runner (cursor + Gemini model).
        let resolved = resolve_for(
            &builtin_settings,
            "cursor",
            Some("gemini-3.8-flash-low"),
            PromptRole::Chat,
            true,
        )
        .expect("built-in model prompt");
        assert_eq!(resolved.text, BUILTIN_AGY_PROMPT);
        assert_eq!(resolved.source, "builtin:model:gemini");
    }

    // TC-2153 / FR-2153: built-ins only on write turns and can be disabled.
    #[test]
    fn tc_2153_builtins_require_write_turn_and_builtin_prompt() {
        let settings = Settings::default();

        assert!(resolve_for(&settings, "cursor", None, PromptRole::Chat, true).is_some());
        assert!(resolve_for(&settings, "cursor", None, PromptRole::Chat, false).is_none());

        // Config prompts still apply on read-only turns.
        let mut config_settings = Settings::default();
        config_settings.runner_prompts.insert(
            "cursor".to_owned(),
            RunnerPromptConfig {
                prompt: Some(PromptTextWithSource {
                    text: "read-only config prompt".to_owned(),
                    source: "workspace:[runners.cursor]".to_owned(),
                }),
                inject: None,
                roles: None,
                model: None,
                models: None,
                builtin_prompt: None,
            },
        );
        let resolved = resolve_for(&config_settings, "cursor", None, PromptRole::Chat, false)
            .expect("config prompt on read-only turn");
        assert_eq!(resolved.text, "read-only config prompt");

        // Runner builtin_prompt = false disables built-ins.
        let mut runner_off = Settings::default();
        runner_off.runner_prompts.insert(
            "cursor".to_owned(),
            RunnerPromptConfig {
                builtin_prompt: Some(false),
                ..Default::default()
            },
        );
        assert!(resolve_for(&runner_off, "cursor", None, PromptRole::Chat, true).is_none());

        // Default builtin_prompt = false disables built-ins for unset runners.
        let mut default_off = Settings::default();
        default_off.runner_prompts.insert(
            "default".to_owned(),
            RunnerPromptConfig {
                builtin_prompt: Some(false),
                ..Default::default()
            },
        );
        assert!(resolve_for(&default_off, "cursor", None, PromptRole::Chat, true).is_none());

        // A runner value beats the default value.
        let mut runner_on = default_off.clone();
        runner_on.runner_prompts.insert(
            "cursor".to_owned(),
            RunnerPromptConfig {
                builtin_prompt: Some(true),
                ..Default::default()
            },
        );
        assert!(resolve_for(&runner_on, "cursor", None, PromptRole::Chat, true).is_some());
        assert!(resolve_for(&runner_on, "agy", None, PromptRole::Chat, true).is_none());
    }

    // TC-2154 / FR-2152: built-ins by model and by runner, including custom agents.
    #[test]
    fn tc_2154_builtin_by_model_and_runner() {
        let settings = Settings::default();

        let agy_flash = resolve_for(
            &settings,
            "agy-flash",
            Some("gemini-3.8-flash-low"),
            PromptRole::Worker,
            true,
        )
        .expect("agy-flash built-in");
        assert_eq!(agy_flash.text, BUILTIN_AGY_PROMPT);
        assert_eq!(agy_flash.source, "builtin:model:gemini");

        let cursor = resolve_for(&settings, "cursor", None, PromptRole::Chat, true)
            .expect("cursor built-in");
        assert_eq!(cursor.text, BUILTIN_COMPOSER_PROMPT);
        assert_eq!(cursor.source, "builtin:runner:cursor");

        assert!(resolve_for(&settings, "opencode", None, PromptRole::Chat, true).is_none());
        assert!(resolve_for(&settings, "codex", None, PromptRole::Chat, true).is_none());
    }

    // TC-2155 / FR-2154: inject precedence, per-model hashes, and sources.
    #[test]
    fn tc_2155_model_inject_hash_and_source() {
        let mut settings = Settings::default();
        settings.runner_prompts.insert(
            "cursor".to_owned(),
            RunnerPromptConfig {
                prompt: None,
                inject: Some(PromptInject::Session),
                roles: None,
                model: None,
                models: Some(BTreeMap::from([(
                    "composer-2.5".to_owned(),
                    ModelPromptConfig {
                        prompt: Some(PromptTextWithSource {
                            text: "model specific".to_owned(),
                            source: "user:[runners.cursor.models.\"composer-2.5\"]".to_owned(),
                        }),
                        inject: Some(PromptInject::Always),
                    },
                )])),
                builtin_prompt: None,
            },
        );

        let resolved = resolve_for(
            &settings,
            "cursor",
            Some("  COMPOSER-2.5  "),
            PromptRole::Chat,
            true,
        )
        .expect("model prompt");
        assert_eq!(resolved.inject, PromptInject::Always);
        assert_eq!(resolved.hash, hash_prompt("model specific"));
        assert_eq!(
            resolved.source,
            "user:[runners.cursor.models.\"composer-2.5\"]"
        );

        let composer = resolve_for(&Settings::default(), "cursor", None, PromptRole::Chat, true)
            .expect("composer built-in");
        let gemini = resolve_for(
            &Settings::default(),
            "x",
            Some("gemini-3.8-flash-low"),
            PromptRole::Chat,
            true,
        )
        .expect("agy built-in");
        assert_ne!(composer.hash, gemini.hash);
    }

    // TC-2156 / FR-2150 / FR-2162: built-in prompt files are small and keep their key lines.
    #[test]
    fn tc_2156_builtin_prompt_files_are_bounded_and_keep_key_lines() {
        assert!(BUILTIN_AGY_PROMPT.len() < 4 * 1024);
        assert!(BUILTIN_COMPOSER_PROMPT.len() < 4 * 1024);
        assert!(BUILTIN_CURSOR_GROK_PROMPT.len() < 4 * 1024);
        assert!(BUILTIN_GROK_BUILD_PROMPT.len() < 4 * 1024);
        assert!(BUILTIN_AGY_PROMPT.contains("no manage_task"));
        assert!(BUILTIN_AGY_PROMPT.contains("never ask for approval"));
        assert!(BUILTIN_COMPOSER_PROMPT.contains("Do not narrate progress"));
        assert!(BUILTIN_CURSOR_GROK_PROMPT.contains("ALWAYS"));
        assert!(BUILTIN_CURSOR_GROK_PROMPT.contains("## Stop conditions"));
        assert!(BUILTIN_GROK_BUILD_PROMPT.contains("ALWAYS"));
        assert!(BUILTIN_GROK_BUILD_PROMPT.contains("## Stop conditions"));
    }

    // TC-2160 / FR-2160: Cursor runner with grok-4.7 variants resolves to cursor-grok prompt.
    #[test]
    fn tc_2160_cursor_grok_builtin_model_prompt() {
        let settings = Settings::default();

        let cursor_grok = resolve_for(
            &settings,
            "cursor",
            Some("grok-4.7"),
            PromptRole::Chat,
            true,
        )
        .expect("cursor grok-4.7 built-in");
        assert_eq!(cursor_grok.text, BUILTIN_CURSOR_GROK_PROMPT);
        assert_eq!(cursor_grok.source, "builtin:model:cursor:grok-4.7");

        let cursor_grok_high = resolve_for(
            &settings,
            "cursor",
            Some("  GROK-4.7-HIGH  "),
            PromptRole::Worker,
            true,
        )
        .expect("cursor grok-4.7-high built-in");
        assert_eq!(cursor_grok_high.text, BUILTIN_CURSOR_GROK_PROMPT);
        assert_eq!(cursor_grok_high.source, "builtin:model:cursor:grok-4.7");

        // fast ids do not match
        assert_ne!(
            resolve_for(
                &settings,
                "cursor",
                Some("grok-4.7-fast"),
                PromptRole::Chat,
                true
            )
            .map(|r| r.source),
            Some("builtin:model:cursor:grok-4.7".to_string())
        );
    }

    // TC-2161 / FR-2160, FR-2161: grok runner resolution and runner-scoped cursor grok.
    #[test]
    fn tc_2161_grok_runner_and_runner_isolation() {
        let settings = Settings::default();

        // Grok runner with grok-4.7 gets grok-build prompt, NOT cursor-grok
        let grok_with_model =
            resolve_for(&settings, "grok", Some("grok-4.7"), PromptRole::Chat, true)
                .expect("grok-build prompt");
        assert_eq!(grok_with_model.text, BUILTIN_GROK_BUILD_PROMPT);
        assert_eq!(grok_with_model.source, "builtin:runner:grok");

        // Grok runner with no model gets grok-build prompt
        let grok_no_model = resolve_for(&settings, "grok", None, PromptRole::Chat, true)
            .expect("grok-build prompt");
        assert_eq!(grok_no_model.text, BUILTIN_GROK_BUILD_PROMPT);
        assert_eq!(grok_no_model.source, "builtin:runner:grok");

        // Other runners (e.g. opencode, codex) with grok-4.7 get NO cursor-grok prompt
        assert!(
            resolve_for(
                &settings,
                "opencode",
                Some("grok-4.7"),
                PromptRole::Chat,
                true
            )
            .is_none()
        );
        assert!(
            resolve_for(&settings, "codex", Some("grok-4.7"), PromptRole::Chat, true).is_none()
        );

        // Cursor with other model (e.g. claude-sonnet-5-5-high) gets composer prompt via runner built-in
        let cursor_other = resolve_for(
            &settings,
            "cursor",
            Some("claude-sonnet-5-5-high"),
            PromptRole::Chat,
            true,
        )
        .expect("cursor fallback to runner built-in");
        assert_eq!(cursor_other.text, BUILTIN_COMPOSER_PROMPT);
        assert_eq!(cursor_other.source, "builtin:runner:cursor");
    }

    // TC-2162 / FR-2160, FR-2161: read-only turns and builtin_prompt=false get none.
    #[test]
    fn tc_2162_grok_prompts_respect_readonly_and_builtin_flag() {
        let settings = Settings::default();

        // Read-only turns get none
        assert!(
            resolve_for(
                &settings,
                "cursor",
                Some("grok-4.7"),
                PromptRole::Chat,
                false
            )
            .is_none()
        );
        assert!(resolve_for(&settings, "grok", None, PromptRole::Chat, false).is_none());
        assert!(
            resolve_for(&settings, "grok", Some("grok-4.7"), PromptRole::Chat, false).is_none()
        );

        // builtin_prompt = false disables them
        let mut disabled_settings = Settings::default();
        disabled_settings.runner_prompts.insert(
            "cursor".to_owned(),
            RunnerPromptConfig {
                prompt: None,
                inject: None,
                roles: None,
                model: None,
                models: None,
                builtin_prompt: Some(false),
            },
        );
        disabled_settings.runner_prompts.insert(
            "grok".to_owned(),
            RunnerPromptConfig {
                prompt: None,
                inject: None,
                roles: None,
                model: None,
                models: None,
                builtin_prompt: Some(false),
            },
        );

        assert!(
            resolve_for(
                &disabled_settings,
                "cursor",
                Some("grok-4.7"),
                PromptRole::Chat,
                true
            )
            .is_none()
        );
        assert!(resolve_for(&disabled_settings, "grok", None, PromptRole::Chat, true).is_none());
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
            idle_timeout_secs: None,
            failure_stderr: None,
            stall_keepalive: None,
            temp_config_template: None,
            system_prompt_arg: vec!["--system".to_owned(), "{system_prompt}".to_owned()],
            env: BTreeMap::new(),
            modes: Default::default(),
            done_when: None,
            text_break_when: None,
            tool_when: None,
            tool_name_path: None,
            tool_title_path: None,
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
