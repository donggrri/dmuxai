//! Generic headless-command agents declared in `dmuxai.toml` (`[[agents.custom]]`).
//! See docs/agents/custom-agents.md.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::runtime_env::RuntimeEnv;

/// Upper bound for the built-in preset's `models_command` (SPEC-first-run FR-813).
pub const BUILTIN_MODELS_TIMEOUT: Duration = Duration::from_secs(10);

/// Default per-turn timeout when `timeout_secs` is not set (FR-339).
pub const DEFAULT_TIMEOUT_SECS: u64 = 1800;

/// Default idle watchdog timeout when `idle_timeout_secs` is not set (FR-1796).
pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 300;

/// How the prompt is delivered to the runner process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PromptVia {
    #[default]
    Arg,
    Stdin,
}

/// Runner output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CustomAgentOutput {
    #[default]
    Text,
    Jsonl,
}

/// Additional arguments per execution mode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct CustomAgentModes {
    #[serde(default)]
    pub ask: Vec<String>,
    #[serde(default)]
    pub plan: Vec<String>,
    #[serde(default)]
    pub implement: Vec<String>,
}

/// Optional condition for matching JSONL text lines.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TextWhen {
    pub path: String,
    /// The value the pointer must have. A TOML boolean is accepted and
    /// compared as `"true"` / `"false"` (FR-2440).
    #[serde(deserialize_with = "deserialize_when_equals")]
    pub equals: String,
}

/// Accepts a string or a boolean for [`TextWhen::equals`]. A boolean becomes
/// `"true"` / `"false"`, so `equals = true` works in `dmuxai.toml` (FR-2440).
fn deserialize_when_equals<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum EqualsInput {
        Bool(bool),
        Text(String),
    }
    match EqualsInput::deserialize(deserializer)? {
        EqualsInput::Bool(value) => Ok(value.to_string()),
        EqualsInput::Text(value) => Ok(value),
    }
}

fn deserialize_error_path<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum ErrorPathInput {
        Single(String),
        Multiple(Vec<String>),
    }
    let value = Option::<ErrorPathInput>::deserialize(deserializer)?;
    Ok(value.map(|v| match v {
        ErrorPathInput::Single(s) => vec![s],
        ErrorPathInput::Multiple(v) => v,
    }))
}

/// Specification for a custom headless agent loaded from `[[agents.custom]]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
pub struct CustomAgentSpec {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub prompt_via: PromptVia,
    #[serde(default)]
    pub image_arg: Vec<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub models_command: Vec<String>,
    #[serde(default)]
    pub models_filter: Option<String>,
    #[serde(default)]
    pub allowed_models: Option<String>,
    #[serde(default)]
    pub output: Option<CustomAgentOutput>,
    #[serde(default)]
    pub text_path: Option<String>,
    #[serde(default)]
    pub text_when: Option<TextWhen>,
    /// JSONL lines matching this condition never contribute reply text
    /// (FR-2440). The opencode preset uses it to skip synthetic parts.
    #[serde(default)]
    pub text_skip_when: Option<TextWhen>,
    /// A JSONL line matching this condition marks a compaction: the reply text
    /// collected for the preceding message is dropped and a `context compacted`
    /// notice is shown instead (FR-2441).
    #[serde(default)]
    pub compaction_when: Option<TextWhen>,
    /// JSON pointer of the message id of a JSONL part. It groups the text parts
    /// of one message, so a compaction summary can be dropped as a whole
    /// (FR-2441).
    #[serde(default)]
    pub message_id_path: Option<String>,
    #[serde(default, deserialize_with = "deserialize_error_path")]
    pub error_path: Option<Vec<String>>,
    #[serde(default)]
    pub session_id_path: Option<String>,
    #[serde(default)]
    pub resume_arg: Vec<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Seconds without any stdout line before the turn fails with an Idle error; default 300; 0 disables.
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub failure_stderr: Option<String>,
    #[serde(default)]
    pub stall_keepalive: Option<String>,
    #[serde(default)]
    pub temp_config_template: Option<String>,
    #[serde(default)]
    pub system_prompt_arg: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub modes: CustomAgentModes,
    /// JSONL line that marks the end of the turn (SPEC-grok-runner FR-914).
    /// When it matches, DmuxAI stops reading, gives the process a short grace
    /// period to exit, then stops its process group.
    #[serde(default)]
    pub done_when: Option<TextWhen>,
    /// JSONL line after which the next reply text starts a new paragraph
    /// (SPEC-grok-runner FR-916), e.g. a tool call between two text segments.
    #[serde(default)]
    pub text_break_when: Option<TextWhen>,
    /// JSONL line that marks a tool invocation (FR-2430). When it matches,
    /// DmuxAI emits a tool activity item for `--progress`.
    #[serde(default)]
    pub tool_when: Option<TextWhen>,
    /// JSON pointer to the tool name in a matching tool line (FR-2430).
    #[serde(default)]
    pub tool_name_path: Option<String>,
    /// JSON pointer to the tool title or input summary (FR-2430).
    #[serde(default)]
    pub tool_title_path: Option<String>,
    /// True for a built-in preset (opencode, grok). Not configurable.
    #[serde(skip)]
    pub builtin: bool,
}

/// The built-in `opencode` runner (SPEC-first-run FR-810..FR-812, SPEC-custom-agent-provider
/// FR-1610): the documented opencode custom agent with the free-model policy, with no
/// `--dir` flag — the workspace comes only from the child's cwd. `program` is the executable
/// that was found. `default_model` stays unset, so the first allowed listed model
/// is used.
pub fn opencode_preset(program: &str) -> CustomAgentSpec {
    let words = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    CustomAgentSpec {
        name: "opencode".to_owned(),
        label: Some("opencode".to_owned()),
        color: Some("#ffa500".to_owned()),
        command: [
            program, "run", "--format", "json", "-m", "{model}", "{prompt}",
        ]
        .map(str::to_owned)
        .to_vec(),
        prompt_via: PromptVia::Arg,
        image_arg: words(&["-f", "{image}"]),
        default_model: None,
        models_command: words(&[program, "models"]),
        models_filter: Some("-free$".to_owned()),
        allowed_models: Some("^opencode/.*-free$".to_owned()),
        output: Some(CustomAgentOutput::Jsonl),
        text_path: Some("/part/text".to_owned()),
        text_when: Some(TextWhen {
            path: "/type".to_owned(),
            equals: "text".to_owned(),
        }),
        tool_when: Some(TextWhen {
            path: "/type".to_owned(),
            equals: "tool_use".to_owned(),
        }),
        tool_name_path: Some("/part/tool".to_owned()),
        tool_title_path: Some("/part/state/title".to_owned()),
        // FR-2440: a synthetic part is never reply text (the compaction
        // continuation prompt must not leak into the answer).
        text_skip_when: Some(TextWhen {
            path: "/part/synthetic".to_owned(),
            equals: "true".to_owned(),
        }),
        // FR-2441: the compaction continuation marks the end of the summary
        // message, whose text is dropped instead of reaching the reply.
        compaction_when: Some(TextWhen {
            path: "/part/metadata/compaction_continue".to_owned(),
            equals: "true".to_owned(),
        }),
        // FR-2441: the message id groups the text parts of one message.
        message_id_path: Some("/part/messageID".to_owned()),
        error_path: Some(vec![
            "/error/data/message".to_owned(),
            "/error/message".to_owned(),
        ]),
        session_id_path: Some("/sessionID".to_owned()),
        resume_arg: words(&["--session", "{session_id}"]),
        timeout_secs: Some(DEFAULT_TIMEOUT_SECS),
        idle_timeout_secs: Some(DEFAULT_IDLE_TIMEOUT_SECS),
        failure_stderr: Some("(?i)permission denied|rejected permission".to_owned()),
        temp_config_template: Some(
            r#"{"permission":{"edit":"{edit}","bash":"{edit}","webfetch":"allow"}}"#.to_owned(),
        ),
        env: BTreeMap::from([("OPENCODE_CONFIG".to_owned(), "{temp_config}".to_owned())]),
        builtin: true,
        ..Default::default()
    }
}

/// Preferred grok model when `grok models` lists it (SPEC-grok-runner FR-895).
pub const GROK_DEFAULT_MODEL: &str = "grok-4.7";

/// The built-in `grok` runner (SPEC-grok-runner FR-892..FR-898). `program` is
/// the executable that was found. `default_model` stays unset so a missing id
/// is never invented; selection prefers [`GROK_DEFAULT_MODEL`] when listed.
pub fn grok_preset(program: &str) -> CustomAgentSpec {
    let words = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    CustomAgentSpec {
        name: "grok".to_owned(),
        label: Some("grok".to_owned()),
        color: Some("#6366f1".to_owned()),
        command: [
            program,
            "-p",
            "{prompt}",
            "-m",
            "{model}",
            "--cwd",
            "{workspace}",
            "--output-format",
            "streaming-json",
            "--no-wait-for-background",
        ]
        .map(str::to_owned)
        .to_vec(),
        prompt_via: PromptVia::Arg,
        default_model: None,
        models_command: words(&[program, "models"]),
        allowed_models: Some(r"(?i)^grok-[a-z0-9][a-z0-9._-]*$".to_owned()),
        output: Some(CustomAgentOutput::Jsonl),
        text_path: Some("/data".to_owned()),
        text_when: Some(TextWhen {
            path: "/type".to_owned(),
            equals: "text".to_owned(),
        }),
        session_id_path: Some("/sessionId".to_owned()),
        resume_arg: words(&["-r", "{session_id}"]),
        timeout_secs: Some(DEFAULT_TIMEOUT_SECS),
        idle_timeout_secs: Some(DEFAULT_IDLE_TIMEOUT_SECS),
        // FR-913: a background task that finishes during the turn queues an
        // auto-wake prompt that grok 1.0.62 never runs in headless mode, so the
        // process idles forever after its reply. Auto-wake off avoids it.
        env: BTreeMap::from([("GROK_AUTO_WAKE".to_owned(), "0".to_owned())]),
        done_when: Some(TextWhen {
            path: "/type".to_owned(),
            equals: "end".to_owned(),
        }),
        text_break_when: Some(TextWhen {
            path: "/type".to_owned(),
            equals: "tool_call".to_owned(),
        }),
        modes: CustomAgentModes {
            ask: words(&["--permission-mode", "plan"]),
            plan: words(&["--permission-mode", "plan"]),
            implement: words(&["--always-approve"]),
        },
        builtin: true,
        ..Default::default()
    }
}

/// The built-in grok preset, as opposed to a user agent that happens to be named grok.
pub fn is_builtin_grok(spec: &CustomAgentSpec) -> bool {
    spec.builtin && spec.name.eq_ignore_ascii_case("grok")
}

/// Built-in `opencode` preset (SPEC-first-run FR-810, SPEC-acp-runners FR-1320).
pub fn is_builtin_opencode(spec: &CustomAgentSpec) -> bool {
    spec.builtin && spec.name.eq_ignore_ascii_case("opencode")
}

/// `(?i)^grok-[a-z0-9][a-z0-9._-]*$` on the trimmed id. Does not apply the `fast` rule.
pub fn is_grok_model_id(model_id: &str) -> bool {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        Regex::new(r"(?i)^grok-[a-z0-9][a-z0-9._-]*$").expect("grok model id pattern")
    });
    pattern.is_match(model_id.trim())
}

fn extract_grok_candidate_token(line: &str) -> Option<&str> {
    let mut token = line.trim();
    if token.is_empty() {
        return None;
    }
    if token.starts_with('*') || token.starts_with('-') {
        token = token[1..].trim_start();
    }
    if let Some(stripped) = token.strip_suffix("(default)") {
        token = stripped.trim_end();
    }
    if token.split_whitespace().nth(1).is_some() {
        return None;
    }
    Some(token)
}

/// Parse `grok models` stdout into allowed ids (FR-895). Order is kept.
/// `fast` ids and lines that are not a single grok token are dropped.
pub fn parse_grok_model_ids(stdout: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for line in stdout.lines() {
        let Some(token) = extract_grok_candidate_token(line) else {
            continue;
        };
        if !is_grok_model_id(token) || token.to_ascii_lowercase().contains("fast") {
            continue;
        }
        if !ids.iter().any(|id| id == token) {
            ids.push(token.to_owned());
        }
    }
    ids
}

/// Keep `current` when it is still listed. Otherwise prefer [`GROK_DEFAULT_MODEL`],
/// then the first allowed id. Never invent an id that was not listed.
pub fn select_grok_model(ids: &[String], current: Option<&str>) -> Option<String> {
    if let Some(current) = current.filter(|id| ids.iter().any(|listed| listed == id)) {
        return Some(current.to_owned());
    }
    if ids.iter().any(|id| id == GROK_DEFAULT_MODEL) {
        return Some(GROK_DEFAULT_MODEL.to_owned());
    }
    ids.first().cloned()
}

/// Authenticated iff the models command exited 0 and either stdout contains `logged in`
/// (ASCII case-insensitive) or stdout contains at least one parseable `grok-` model id
/// (including fast ids) (FR-896, FR-2170). Takes no path (NFR-890, A-3).
pub fn grok_models_authenticated(stdout: &str, exit_ok: bool) -> bool {
    if !exit_ok {
        return false;
    }
    if stdout.to_ascii_lowercase().contains("logged in") {
        return true;
    }
    stdout
        .lines()
        .any(|line| extract_grok_candidate_token(line).is_some_and(is_grok_model_id))
}

/// Shown when `grok models` is missing, fails, times out, or is not logged in (FR-896).
pub fn grok_login_message() -> String {
    "grok: not logged in. Run `grok login`.".to_owned()
}

/// Shown when an authenticated listing has no allowed id (FR-895).
pub fn grok_no_allowed_model_message() -> String {
    "grok: no allowed models (ids must match ^grok-; ids containing fast are rejected)".to_owned()
}

/// User-visible timeout message prefix and hint when a custom-agent turn exceeds `timeout_secs` (FR-333).
pub fn timeout_parts(spec: &CustomAgentSpec) -> (String, String) {
    let prefix = format!("{} timed out after {}s", spec.label(), spec.timeout_secs());
    let hint = if spec.builtin {
        format!(
            "copy the built-in \"{}\" preset into dmuxai.toml as [[agents.custom]] (run dmuxai init for a commented copy) and raise its timeout_secs (the turn was stopped; check the workspace for partial edits)",
            spec.name
        )
    } else {
        format!(
            "raise timeout_secs for [[agents.custom]] name = \"{}\" in dmuxai.toml (the turn was stopped; check the workspace for partial edits)",
            spec.name
        )
    };
    (prefix, hint)
}

/// User-visible message when a custom-agent turn exceeds `timeout_secs` (FR-333).
pub fn timeout_message(spec: &CustomAgentSpec) -> String {
    let (prefix, hint) = timeout_parts(spec);
    format!("{prefix}; {hint}")
}

impl CustomAgentSpec {
    /// Returns the display label, defaulting to the agent name.
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }

    /// Returns the UI theme color, defaulting to "white".
    pub fn color(&self) -> &str {
        self.color.as_deref().unwrap_or("white")
    }

    /// Returns the timeout in seconds, defaulting to [`DEFAULT_TIMEOUT_SECS`].
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS)
    }

    /// Returns the idle timeout in seconds, returning `None` if disabled (`0`),
    /// or `Some(300)` when unset ([`DEFAULT_IDLE_TIMEOUT_SECS`]).
    pub fn idle_timeout_secs(&self) -> Option<u64> {
        match self.idle_timeout_secs {
            None => Some(DEFAULT_IDLE_TIMEOUT_SECS),
            Some(0) => None,
            Some(secs) => Some(secs),
        }
    }

    /// Returns whether any of `text_path`, `text_when`, `text_skip_when`,
    /// `compaction_when`, `error_path`, `session_id_path`, `message_id_path`,
    /// `done_when`, or `text_break_when` is set.
    pub fn has_jsonl_path_fields(&self) -> bool {
        self.text_path.is_some()
            || self.text_when.is_some()
            || self.text_skip_when.is_some()
            || self.compaction_when.is_some()
            || self.error_path.is_some()
            || self.session_id_path.is_some()
            || self.message_id_path.is_some()
            || self.done_when.is_some()
            || self.text_break_when.is_some()
    }

    /// Whether any field references `{mcp_config}`, which was deprecated when
    /// MCP propagation was removed (FR-2082).
    pub fn uses_deprecated_mcp_config(&self) -> bool {
        self.command.iter().any(|arg| arg.contains("{mcp_config}"))
            || self
                .resume_arg
                .iter()
                .any(|arg| arg.contains("{mcp_config}"))
            || self
                .temp_config_template
                .as_deref()
                .is_some_and(|tmpl| tmpl.contains("{mcp_config}"))
            || self.env.values().any(|val| val.contains("{mcp_config}"))
    }

    /// Returns the effective output format, inferring Jsonl when JSONL path fields
    /// are set but output is not explicitly set to Jsonl.
    ///
    /// - If `output` is `Some(Jsonl)`, the output is `Jsonl`.
    /// - If `output` is `Some(Text)`, the output is `Text` (path fields are ignored
    ///   even when configured, because the user explicitly chose text mode).
    /// - If `output` is `None` (not set in config) and any JSONL path field is configured,
    ///   the output is inferred as `Jsonl`.
    /// - If `output` is `None` and no JSONL path field is configured, the output is `Text`
    ///   (backward‑compatible default).
    pub fn effective_output(&self) -> CustomAgentOutput {
        match self.output {
            Some(CustomAgentOutput::Jsonl) => CustomAgentOutput::Jsonl,
            Some(CustomAgentOutput::Text) => CustomAgentOutput::Text,
            None if self.has_jsonl_path_fields() => CustomAgentOutput::Jsonl,
            None => CustomAgentOutput::Text,
        }
    }

    /// Validates the specification according to documentation constraints.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("custom agent name cannot be empty".to_string());
        }

        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Err(format!(
                "custom agent name '{}' must contain only lowercase ASCII letters, digits, '_' and '-'",
                self.name
            ));
        }

        if self.command.is_empty() {
            return Err(format!(
                "custom agent '{}' command cannot be empty",
                self.name
            ));
        }

        if let Some(pattern) = &self.models_filter {
            Regex::new(pattern).map_err(|e| {
                format!(
                    "custom agent '{}' invalid models_filter regex: {e}",
                    self.name
                )
            })?;
        }

        if let Some(pattern) = &self.allowed_models {
            Regex::new(pattern).map_err(|e| {
                format!(
                    "custom agent '{}' invalid allowed_models regex: {e}",
                    self.name
                )
            })?;
        }

        if let Some(pattern) = &self.failure_stderr {
            Regex::new(pattern).map_err(|e| {
                format!(
                    "custom agent '{}' invalid failure_stderr regex: {e}",
                    self.name
                )
            })?;
        }

        if let Some(pattern) = &self.stall_keepalive {
            Regex::new(pattern).map_err(|e| {
                format!(
                    "custom agent '{}' invalid stall_keepalive regex: {e}",
                    self.name
                )
            })?;
        }

        Ok(())
    }
}

/// Context passed to template substitution and argument building.
#[derive(Debug, Clone)]
pub struct SubstitutionContext {
    pub prompt: String,
    pub model: Option<String>,
    pub workspace: String,
    pub images: Vec<String>,
    pub session_id: Option<String>,
    pub temp_config: Option<String>,
    pub mode: &'static str,
    pub edit: &'static str,
    pub system_prompt: Option<String>,
    pub system_prompt_file: Option<String>,
}

impl SubstitutionContext {
    pub fn new(
        prompt: impl Into<String>,
        workspace: impl Into<String>,
        mode: &'static str,
        edit: &'static str,
    ) -> Self {
        Self {
            prompt: prompt.into(),
            model: None,
            workspace: workspace.into(),
            images: Vec::new(),
            session_id: None,
            temp_config: None,
            mode,
            edit,
            system_prompt: None,
            system_prompt_file: None,
        }
    }
}

/// Internal helper for placeholder substitution.
fn substitute_internal(
    template: &str,
    ctx: &SubstitutionContext,
    image: Option<&str>,
    prompt_val: Option<&str>,
) -> Option<String> {
    // Single pass: values are inserted verbatim and never re-scanned, so a
    // prompt containing "{model}" (etc.) cannot trigger further substitution.
    let mut result = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        result.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find('}') else {
            result.push_str(after);
            return Some(result);
        };
        let key = &after[1..end];
        let value: Option<&str> = match key {
            "prompt" => Some(prompt_val?),
            "model" => Some(ctx.model.as_deref()?),
            "workspace" => Some(ctx.workspace.as_str()),
            "image" => Some(image?),
            "session_id" => Some(ctx.session_id.as_deref()?),
            "temp_config" => Some(ctx.temp_config.as_deref()?),
            "mcp_config" => return None,
            "system_prompt" => Some(ctx.system_prompt.as_deref().unwrap_or("")),
            "system_prompt_file" => Some(ctx.system_prompt_file.as_deref().unwrap_or("")),
            "mode" => Some(ctx.mode),
            "edit" => Some(ctx.edit),
            _ => None,
        };
        match value {
            Some(value) => {
                result.push_str(value);
                rest = &after[end + 1..];
            }
            None => {
                // Not a placeholder (e.g. a JSON brace): keep `{` and rescan after it.
                result.push('{');
                rest = &after[1..];
            }
        }
    }
    result.push_str(rest);
    Some(result)
}

/// Substitutes placeholders in `template` using `ctx`.
/// Returns `None` if any referenced placeholder does not have a value.
pub fn substitute(template: &str, ctx: &SubstitutionContext) -> Option<String> {
    let single_image = if ctx.images.len() == 1 {
        ctx.images.first().map(|s| s.as_str())
    } else {
        None
    };
    substitute_internal(template, ctx, single_image, Some(&ctx.prompt))
}

/// Appends a substituted template argument to `argv`, implementing the drop-unset-placeholder rule:
/// an argument whose placeholder has no value is dropped together with the flag right before it
/// when that flag starts with `-`.
fn push_substituted(
    argv: &mut Vec<String>,
    template: &str,
    ctx: &SubstitutionContext,
    image: Option<&str>,
    prompt_for_argv: Option<&str>,
) {
    if let Some(substituted) = substitute_internal(template, ctx, image, prompt_for_argv) {
        argv.push(substituted);
    } else if let Some(prev) = argv.last()
        && prev.starts_with('-')
    {
        argv.pop();
    }
}

/// Builds the final argv command list for execution.
pub fn build_argv(spec: &CustomAgentSpec, ctx: &SubstitutionContext) -> Vec<String> {
    let mut argv = Vec::new();

    // Determine the prompt value to use for {prompt} placeholder in argv.
    // For prompt_via stdin, no {prompt} is passed in argv (treated as unset).
    let prompt_for_argv = match spec.prompt_via {
        PromptVia::Stdin => None,
        PromptVia::Arg => {
            let mut p = ctx.prompt.clone();
            if spec.image_arg.is_empty() {
                for img in &ctx.images {
                    if !p.is_empty() && !p.ends_with(' ') {
                        p.push(' ');
                    }
                    p.push('@');
                    p.push_str(img);
                }
            }
            Some(p)
        }
    };

    // 1. Base command substitution
    for arg in &spec.command {
        push_substituted(&mut argv, arg, ctx, None, prompt_for_argv.as_deref());
    }

    // FR-1611: an opencode build can run in any directory (the child's cwd is
    // already the workspace), while older configurations still pass the legacy
    // pair `--dir {workspace}`. Drop exactly that pair; other programs and
    // other `--dir` values are left unchanged.
    drop_legacy_opencode_dir_pair(spec, ctx, &mut argv);

    // 2. Mode args appended
    let mode_args: &[String] = match ctx.mode {
        "ask" => &spec.modes.ask,
        "plan" => &spec.modes.plan,
        "implement" => &spec.modes.implement,
        _ => &[],
    };
    for arg in mode_args {
        push_substituted(&mut argv, arg, ctx, None, prompt_for_argv.as_deref());
    }

    // 3. image_arg per image (if image_arg configured)
    if !spec.image_arg.is_empty() {
        for img in &ctx.images {
            for arg in &spec.image_arg {
                push_substituted(&mut argv, arg, ctx, Some(img), prompt_for_argv.as_deref());
            }
        }
    }

    // 4. resume_arg when session_id is set
    if ctx.session_id.is_some() {
        for arg in &spec.resume_arg {
            push_substituted(&mut argv, arg, ctx, None, prompt_for_argv.as_deref());
        }
    }

    // 5. system_prompt_arg when a prompt is applied
    if ctx.system_prompt.is_some() {
        for arg in &spec.system_prompt_arg {
            push_substituted(&mut argv, arg, ctx, None, prompt_for_argv.as_deref());
        }
    }

    argv
}

/// FR-1611: removes the adjacent `--dir {workspace}` pair that some opencode
/// builds reject. Applies only when the first argv element's basename, with a
/// trailing `.exe` stripped (ASCII case-insensitive), is `opencode`, and only
/// when the spec's base `command` itself contains the adjacent pair. Other
/// programs and `--dir` with any other value are left untouched.
fn drop_legacy_opencode_dir_pair(
    spec: &CustomAgentSpec,
    ctx: &SubstitutionContext,
    argv: &mut Vec<String>,
) {
    if !argv
        .first()
        .is_some_and(|program| program_is_opencode(program))
    {
        return;
    }
    if !spec
        .command
        .windows(2)
        .any(|pair| pair == ["--dir", "{workspace}"])
    {
        return;
    }
    if let Some(index) = argv
        .windows(2)
        .position(|pair| pair == ["--dir", ctx.workspace.as_str()])
    {
        let tail = argv.split_off(index + 2);
        argv.truncate(index);
        argv.extend(tail);
    }
}

/// True when `program`'s basename is `opencode` (or `opencode.exe`,
/// ASCII case-insensitive). Directory components are ignored.
fn program_is_opencode(program: &str) -> bool {
    let basename = Path::new(program)
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| program.to_ascii_lowercase());
    basename.strip_suffix(".exe").unwrap_or(&basename) == "opencode"
}

/// Builds a headless `tokio::process::Command` for a custom agent turn with
/// `.current_dir(workspace)` and substituted environment variables (FR-875).
pub fn build_custom_command(
    spec: &CustomAgentSpec,
    ctx: &SubstitutionContext,
    workspace: &Path,
) -> Result<tokio::process::Command, String> {
    let argv = build_argv(spec, ctx);
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| format!("{}: empty command", spec.label()))?;
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(workspace)
        .stdin(if spec.prompt_via == PromptVia::Stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    crate::process::apply_child_env_policy_tokio(&mut command);

    command.env_remove("DMUXAI_MCP_CONFIG");

    for (key, value) in &spec.env {
        if let Some(value) = substitute(value, ctx) {
            command.env(key, value);
        }
    }
    #[cfg(unix)]
    command.process_group(0);

    Ok(command)
}

/// Result of parsing a single JSONL line emitted by a custom agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedLine {
    pub text: Option<String>,
    pub error: Option<String>,
    pub session_id: Option<String>,
    /// The line matches `done_when` (FR-914).
    pub done: bool,
    /// The line matches `text_break_when` (FR-916).
    pub text_break: bool,
    /// The line matches `compaction_when` (FR-2441).
    pub compacted: bool,
    /// The message id of the line's part, when `message_id_path` is set
    /// (FR-2441).
    pub message_id: Option<String>,
}

/// True when `val` has `when.equals` at `when.path` (string or JSON value).
///
/// A JSON boolean is compared as `"true"` / `"false"`, so a condition can be
/// written as `equals = true` in `dmuxai.toml` (FR-2440).
fn json_matches(val: &serde_json::Value, when: &TextWhen) -> bool {
    match val.pointer(&when.path) {
        Some(serde_json::Value::String(s)) => s == &when.equals,
        Some(serde_json::Value::Bool(b)) => b.to_string() == when.equals,
        Some(v) => *v == when.equals,
        None => false,
    }
}

/// Returns the JSON pointer to the error container for `pointer`, if any: the
/// nearest ancestor of `pointer` (including `pointer` itself) whose final path
/// segment is literally "error". The compact-JSON fallback may only fire for an
/// object located at this pointer, so a success payload such as `/result` (whose
/// own `error` field is null or absent) is never reported as an error. Returns
/// an empty string when no such segment exists.
fn error_container_pointer(pointer: &str) -> &str {
    if !pointer.starts_with('/') {
        return "";
    }
    let mut seg_start = 1usize;
    let mut container_end: Option<usize> = None;
    for slash in pointer[1..].match_indices('/').map(|(o, _)| 1 + o) {
        if &pointer[seg_start..slash] == "error" {
            container_end = Some(slash);
        }
        seg_start = slash + 1;
    }
    if seg_start <= pointer.len() && &pointer[seg_start..] == "error" {
        container_end = Some(pointer.len());
    }
    match container_end {
        Some(end) => &pointer[..end],
        None => "",
    }
}

/// Parses a line of JSONL output according to the agent's pointer configuration.
/// Invalid JSON lines are ignored (returning all fields `None`).
pub fn parse_jsonl_line(spec: &CustomAgentSpec, line: &str) -> ParsedLine {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
        return ParsedLine::default();
    };

    // FR-2440: a line that matches `text_skip_when` never contributes reply
    // text (the opencode preset skips synthetic parts).
    let skipped = spec
        .text_skip_when
        .as_ref()
        .is_some_and(|when| json_matches(&val, when));

    let text = if let Some(text_path) = &spec.text_path {
        let condition_met = match &spec.text_when {
            None => true,
            Some(when) => json_matches(&val, when),
        };

        if condition_met && !skipped {
            val.pointer(text_path).and_then(|v| match v {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Null => None,
                other => Some(other.to_string()),
            })
        } else {
            None
        }
    } else {
        None
    };

    let error = spec.error_path.as_ref().and_then(|paths| {
        let mut result: Option<String> = None;
        let mut yielded_string = false;
        for path in paths {
            if let Some(v) = val.pointer(path) {
                match v {
                    serde_json::Value::String(s) => {
                        if !s.is_empty() {
                            result = Some(s.clone());
                            yielded_string = true;
                            break;
                        }
                    }
                    serde_json::Value::Null => {}
                    other => {
                        result = Some(other.to_string());
                        break;
                    }
                }
            }
        }
        if result.is_none()
            && !yielded_string
            && let Some(first) = paths.first()
        {
            // The compact-JSON fallback only applies to an object that is itself
            // the error container (a pointer segment literally named "error");
            // never to an arbitrary success object that merely contains a
            // null/missing `error` field.
            let top = error_container_pointer(first);
            if !top.is_empty()
                && let Some(serde_json::Value::Object(_)) = val.pointer(top)
            {
                result = val.pointer(top).map(|v| v.to_string());
            }
        }
        result
    });

    let session_id = spec.session_id_path.as_ref().and_then(|path| {
        val.pointer(path).and_then(|v| match v {
            serde_json::Value::String(s) => (!s.is_empty()).then(|| s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            serde_json::Value::Null => None,
            other => Some(other.to_string()),
        })
    });

    // FR-2441: the message id groups the text parts of one message.
    let message_id = spec.message_id_path.as_ref().and_then(|path| {
        val.pointer(path).and_then(|v| match v {
            serde_json::Value::String(s) => (!s.is_empty()).then(|| s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
    });

    ParsedLine {
        text,
        error,
        session_id,
        done: spec
            .done_when
            .as_ref()
            .is_some_and(|when| json_matches(&val, when)),
        text_break: spec
            .text_break_when
            .as_ref()
            .is_some_and(|when| json_matches(&val, when)),
        compacted: spec
            .compaction_when
            .as_ref()
            .is_some_and(|when| json_matches(&val, when)),
        message_id,
    }
}

/// Appends `text` to `reply`, starting a new paragraph when a break is pending
/// and the reply does not already end in whitespace (FR-916). Returns the text
/// actually appended, so the streamed delta matches the stored reply.
pub fn append_reply_text(reply: &mut String, text: &str, break_pending: &mut bool) -> String {
    let mut chunk = String::new();
    if std::mem::take(break_pending)
        && !reply.is_empty()
        && !reply.ends_with(char::is_whitespace)
        && !text.starts_with(char::is_whitespace)
    {
        chunk.push_str("\n\n");
    }
    chunk.push_str(text);
    reply.push_str(&chunk);
    chunk
}

/// The user-visible notice shown when a compaction summary is dropped
/// (FR-2441).
pub const COMPACTION_NOTICE: &str = "context compacted";

/// One text segment of the message that is still open.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReplySegment {
    text: String,
    /// The segment starts a new paragraph (FR-916).
    break_before: bool,
}

/// Collects the reply text of a JSONL turn.
///
/// The text parts are grouped per message (`message_id_path`), so that the
/// compaction summary of a runner such as opencode can be dropped as a whole
/// when its compaction continuation arrives (FR-2441). A runner without
/// `message_id_path` keeps the old behaviour: every text part is appended
/// immediately (FR-2442).
#[derive(Debug, Default)]
pub struct ReplyCollector {
    /// True when the text parts are grouped per message (FR-2441).
    grouped: bool,
    /// The message id of the message that is still open.
    open_message_id: Option<String>,
    /// The text segments of the open message.
    open: Vec<ReplySegment>,
    /// The released reply text.
    reply: String,
    /// A paragraph break is pending (FR-916).
    break_pending: bool,
}

impl ReplyCollector {
    /// Creates a collector for the given specification.
    pub fn new(spec: &CustomAgentSpec) -> Self {
        Self {
            grouped: spec.message_id_path.is_some(),
            ..Default::default()
        }
    }

    /// True when the text parts are grouped per message (FR-2441).
    fn grouped(&self) -> bool {
        self.grouped
    }

    /// Feeds one parsed line and returns the text that is ready to be
    /// streamed, if any.
    ///
    /// A line that matches `compaction_when` drops the text of the open
    /// message (the compaction summary) and never contributes its own text
    /// (FR-2441).
    pub fn push(&mut self, parsed: &ParsedLine) -> Option<String> {
        if parsed.compacted {
            // FR-2441: the compaction summary (the text of the open message)
            // is dropped, and the continuation prompt is never reply text.
            self.open.clear();
            self.open_message_id = None;
            return None;
        }

        let mut released = String::new();
        let mut had_text = false;

        if self.grouped()
            && let Some(message_id) = parsed.message_id.as_deref()
            && self.open_message_id.as_deref() != Some(message_id)
        {
            // The previous message ended: its text can be released.
            released = self.release();
            self.open_message_id = Some(message_id.to_owned());
        }

        if let Some(text) = parsed.text.as_deref() {
            had_text = true;
            let break_before = std::mem::take(&mut self.break_pending);
            self.open.push(ReplySegment {
                text: text.to_owned(),
                break_before,
            });
            if !self.grouped() {
                released = self.release();
            }
        }

        if parsed.text_break {
            self.break_pending = true;
        }

        if self.grouped() {
            (!released.is_empty()).then_some(released)
        } else if had_text {
            Some(released)
        } else {
            None
        }
    }

    /// Appends raw output text (text mode) to the reply.
    pub fn push_raw(&mut self, text: &str) {
        self.reply.push_str(text);
    }

    /// Releases the text of the open message at the end of the turn.
    pub fn finish(&mut self) -> Option<String> {
        let released = self.release();
        (!released.is_empty()).then_some(released)
    }

    /// The reply text collected so far.
    pub fn reply(&self) -> &str {
        &self.reply
    }

    /// Appends the segments of the open message to the reply.
    fn release(&mut self) -> String {
        let mut released = String::new();
        for segment in std::mem::take(&mut self.open) {
            if segment.break_before {
                self.break_pending = true;
            }
            released.push_str(&append_reply_text(
                &mut self.reply,
                &segment.text,
                &mut self.break_pending,
            ));
        }
        released
    }
}

/// The failure signal of a finished turn, in the order the checks run.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FailureSignal {
    JsonlError(String),
    StderrPattern(String),
    NonZeroExit(String),
    EmptyReply,
}

impl FailureSignal {
    /// The user-facing reason for this failure signal.
    fn reason(&self) -> String {
        match self {
            Self::JsonlError(message)
            | Self::StderrPattern(message)
            | Self::NonZeroExit(message) => message.clone(),
            Self::EmptyReply => "empty reply text".to_string(),
        }
    }
}

/// Detects the first failure signal of a finished turn, in the order the
/// checks run: JSONL error, `failure_stderr` match, non-zero exit, empty
/// reply.
fn failure_signal(
    spec: &CustomAgentSpec,
    exit_ok: bool,
    reply_text: &str,
    stderr: &str,
    jsonl_error: Option<&str>,
) -> Option<FailureSignal> {
    if let Some(err) = jsonl_error.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(FailureSignal::JsonlError(err.to_string()));
    }

    if let Some(pattern) = &spec.failure_stderr
        && let Ok(re) = Regex::new(pattern)
        && re.is_match(stderr)
    {
        let trimmed = stderr.trim();
        return Some(FailureSignal::StderrPattern(if trimmed.is_empty() {
            "matched failure_stderr pattern".to_string()
        } else {
            trimmed.to_string()
        }));
    }

    if !exit_ok {
        let trimmed = stderr.trim();
        return Some(FailureSignal::NonZeroExit(if trimmed.is_empty() {
            let program = spec
                .command
                .first()
                .map(|program| {
                    Path::new(program)
                        .file_name()
                        .map_or(program.clone(), |name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_default();
            format!(
                "process exited with non-zero status and printed nothing on stderr; run `{program}` directly (same model and prompt) to see why"
            )
        } else {
            trimmed.to_string()
        }));
    }

    if reply_text.trim().is_empty() {
        return Some(FailureSignal::EmptyReply);
    }

    None
}

/// Determines whether a turn failed and returns an error reason if so.
pub fn failure_reason(
    spec: &CustomAgentSpec,
    exit_ok: bool,
    reply_text: &str,
    stderr: &str,
    jsonl_error: Option<&str>,
) -> Option<String> {
    failure_signal(spec, exit_ok, reply_text, stderr, jsonl_error).map(|signal| signal.reason())
}

/// The outcome of a finished custom-agent turn (FR-2125).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// The turn produced a reply and no error indicator.
    Success,
    /// The turn exited successfully without a final message, but the workspace
    /// changed during an edit-allowed turn (FR-2125).
    SuccessWithoutReply,
    /// The turn failed with the given reason.
    Failed(String),
}

/// Classifies a finished turn.
///
/// The failure checks run in the same order as [`failure_reason`]: a JSONL
/// error wins, then a `failure_stderr` match, then a non-zero exit, then an
/// empty reply. The one difference is the empty reply: when the turn exited
/// successfully, reported no error, and the workspace changed while it ran
/// (edit-allowed turn), the turn is a success without a final message
/// (FR-2125). An empty reply without a workspace change stays a failure.
pub fn classify_turn_outcome(
    spec: &CustomAgentSpec,
    exit_ok: bool,
    reply_text: &str,
    stderr: &str,
    jsonl_error: Option<&str>,
    workspace_changed: bool,
) -> TurnOutcome {
    match failure_signal(spec, exit_ok, reply_text, stderr, jsonl_error) {
        None => TurnOutcome::Success,
        Some(FailureSignal::EmptyReply) if workspace_changed => TurnOutcome::SuccessWithoutReply,
        Some(signal) => TurnOutcome::Failed(signal.reason()),
    }
}

/// Process-wide registry of configured custom agents, installed once at startup.
static REGISTRY: std::sync::OnceLock<Vec<CustomAgentSpec>> = std::sync::OnceLock::new();

/// Installs the custom agents (first call wins). Invalid specs and duplicates
/// of built-in runner names are skipped and reported in the returned warnings.
pub fn install_registry(specs: Vec<CustomAgentSpec>) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut accepted: Vec<CustomAgentSpec> = Vec::new();
    for spec in specs {
        if let Err(error) = spec.validate() {
            warnings.push(error);
            continue;
        }
        if spec.uses_deprecated_mcp_config() {
            warnings.push(format!(
                "agents.custom \"{}\": {{mcp_config}} is deprecated and ignored; DmuxAI no longer passes MCP servers to agents (configure MCP in the agent itself)",
                spec.name
            ));
        }
        if matches!(spec.name.as_str(), "cursor" | "agy" | "codex")
            || accepted.iter().any(|other| other.name == spec.name)
        {
            warnings.push(format!("custom agent name '{}' is already used", spec.name));
            continue;
        }
        if accepted.len() >= usize::from(u8::MAX) {
            warnings.push("too many custom agents".to_owned());
            break;
        }
        accepted.push(spec);
    }
    let _ = REGISTRY.set(accepted);
    warnings
}

/// All installed custom agents (empty before `install_registry`).
pub fn registry() -> &'static [CustomAgentSpec] {
    REGISTRY.get().map(Vec::as_slice).unwrap_or(&[])
}

pub fn spec_at(index: u8) -> Option<&'static CustomAgentSpec> {
    registry().get(usize::from(index))
}

pub fn index_of(name: &str) -> Option<u8> {
    registry()
        .iter()
        .position(|spec| spec.name.eq_ignore_ascii_case(name.trim()))
        .and_then(|index| u8::try_from(index).ok())
}

/// Model policy for a custom runner: its `allowed_models` regex when set.
pub fn validate_custom_model(spec: &CustomAgentSpec, model_id: &str) -> Result<(), String> {
    match spec.allowed_models.as_deref() {
        None => Ok(()),
        Some(pattern) => match Regex::new(pattern) {
            Ok(re) if re.is_match(model_id.trim()) => Ok(()),
            Ok(_) => Err(format!(
                "model {model_id} is not allowed for {} (allowed_models = {pattern})",
                spec.name
            )),
            Err(error) => Err(error.to_string()),
        },
    }
}

/// Filters `models_command` stdout into model ids using `models_filter`.
pub fn filter_models(spec: &CustomAgentSpec, stdout: &str) -> Vec<String> {
    let filter = spec
        .models_filter
        .as_deref()
        .and_then(|pattern| Regex::new(pattern).ok());
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| filter.as_ref().is_none_or(|re| re.is_match(line)))
        .map(str::to_owned)
        .collect()
}

/// The message shown when the built-in preset has no usable model (FR-812).
pub fn policy_message(spec: &CustomAgentSpec, reason: Option<&str>) -> String {
    let pattern = spec.allowed_models.as_deref().unwrap_or_default();
    let detail = reason.map(|r| format!("; {r}")).unwrap_or_default();
    format!(
        "{label}: no allowed models (policy allowed_models = {pattern}{detail}). Run `{check}` to check, or override with an [[agents.custom]] named {name} (see `dmuxai init`).",
        label = spec.label(),
        check = spec.models_command.join(" "),
        name = spec.name,
    )
}

struct ModelsOutput {
    stdout: String,
    stderr: String,
    status: std::process::ExitStatus,
}

/// Runs `models_command`. Spawn and timeout failures are errors. A non-zero
/// exit is still [`Ok`] so the grok auth check can read stdout and status.
async fn run_models_output(
    spec: &CustomAgentSpec,
    cwd: &Path,
    timeout: Option<Duration>,
) -> Result<ModelsOutput, String> {
    let Some((program, args)) = spec.models_command.split_first() else {
        return Err(format!("{} models: command is empty", spec.label()));
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    crate::process::apply_child_env_policy_tokio(&mut command);
    // Test fixtures set GROK_FAKE_* on the preset. Production grok env is empty.
    // Opencode keeps its models command free of OPENCODE_CONFIG substitution.
    if is_builtin_grok(spec) {
        let ctx = SubstitutionContext::new(String::new(), String::new(), "ask", "deny");
        for (key, value) in &spec.env {
            if let Some(value) = substitute(value, &ctx) {
                command.env(key, value);
            }
        }
    }
    let run = command.output();
    let output = match timeout {
        Some(limit) => match tokio::time::timeout(limit, run).await {
            Ok(output) => output,
            Err(_) => {
                return Err(format!(
                    "{} models timed out after {}s",
                    spec.label(),
                    limit.as_secs()
                ));
            }
        },
        None => run.await,
    }
    .map_err(|error| format!("{} models: {error}", spec.label()))?;
    Ok(ModelsOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        status: output.status,
    })
}

/// Runs `models_command` and returns the filtered model ids (no policy applied).
async fn run_models_command(
    spec: &CustomAgentSpec,
    cwd: &Path,
    timeout: Option<Duration>,
) -> Result<Vec<String>, String> {
    if spec.models_command.is_empty() {
        return Ok(Vec::new());
    }
    let output = run_models_output(spec, cwd, timeout).await?;
    if !output.status.success() {
        let detail = match output.stderr.trim() {
            "" => format!(
                "exited with {} and printed nothing on stderr",
                output.status
            ),
            text => text.to_owned(),
        };
        return Err(format!("{} models failed: {detail}", spec.label()));
    }
    Ok(filter_models(spec, &output.stdout))
}

/// `grok models`: any probe failure is the login message. An authenticated
/// listing with no allowed id is the fast/`^grok-` message (FR-895, FR-896).
async fn list_grok_model_ids(
    spec: &CustomAgentSpec,
    cwd: &Path,
    timeout: Option<Duration>,
) -> Result<Vec<String>, String> {
    let timeout = timeout.or(Some(BUILTIN_MODELS_TIMEOUT));
    let output = match run_models_output(spec, cwd, timeout).await {
        Ok(output) => output,
        Err(_) => return Err(grok_login_message()),
    };
    if !grok_models_authenticated(&output.stdout, output.status.success()) {
        return Err(grok_login_message());
    }
    let ids = parse_grok_model_ids(&output.stdout);
    if ids.is_empty() {
        Err(grok_no_allowed_model_message())
    } else {
        Ok(ids)
    }
}

/// Lists the model ids for `spec`, honouring the model policy (FR-812).
///
/// For the built-in opencode preset a failure, a timeout, or a listing with no
/// `allowed_models` match is an error that names the policy and how to override
/// it. The built-in grok preset uses the login and `^grok-` messages instead.
/// Other agents keep the plain listing.
pub async fn list_model_ids_with_env(
    spec: &CustomAgentSpec,
    cwd: &Path,
    timeout: Option<Duration>,
    env: &RuntimeEnv,
) -> Result<Vec<String>, String> {
    if spec.builtin
        && spec.name.eq_ignore_ascii_case("opencode")
        && let Some(message) = crate::settings::opencode_bin_explicit_missing_message_from(env)
    {
        return Err(message);
    }
    if is_builtin_grok(spec) {
        return list_grok_model_ids(spec, cwd, timeout).await;
    }
    let listed = run_models_command(spec, cwd, timeout).await;
    if !spec.builtin {
        return listed;
    }
    match listed {
        Err(reason) => Err(policy_message(spec, Some(&reason))),
        Ok(ids) => {
            let allowed: Vec<String> = ids
                .into_iter()
                .filter(|id| validate_custom_model(spec, id).is_ok())
                .collect();
            if allowed.is_empty() {
                Err(policy_message(spec, None))
            } else {
                Ok(allowed)
            }
        }
    }
}

pub async fn list_model_ids(
    spec: &CustomAgentSpec,
    cwd: &Path,
    timeout: Option<Duration>,
) -> Result<Vec<String>, String> {
    list_model_ids_with_env(spec, cwd, timeout, &RuntimeEnv::from_process()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENCODE_TOML: &str = r##"
name = "opencode"
label = "opencode"
color = "#ffa500"
command = ["opencode", "run", "--format", "json", "-m", "{model}", "{prompt}"]
prompt_via = "arg"
image_arg = ["-f", "{image}"]
default_model = "opencode/glm-5.3-flash-free"
models_command = ["opencode", "models"]
models_filter = "-free$"
allowed_models = "^opencode/.*-free$"
output = "jsonl"
text_path = "/part/text"
text_when = { path = "/type", equals = "text" }
error_path = ["/error/data/message", "/error/message"]
session_id_path = "/sessionID"
resume_arg = ["--session", "{session_id}"]
timeout_secs = 900
failure_stderr = "(?i)permission denied|rejected permission"
temp_config_template = '{"permission":{"edit":"{edit}","bash":"{edit}","webfetch":"allow"}}'
env = { OPENCODE_CONFIG = "{temp_config}" }

[modes]
ask = []
plan = []
implement = []
"##;

    #[test]
    fn test_deserialize_full_spec() {
        let spec: CustomAgentSpec = toml::from_str(OPENCODE_TOML).unwrap();
        assert_eq!(spec.name, "opencode");
        assert_eq!(spec.label(), "opencode");
        assert_eq!(spec.color(), "#ffa500");
        assert_eq!(spec.prompt_via, PromptVia::Arg);
        assert_eq!(spec.effective_output(), CustomAgentOutput::Jsonl);
        assert_eq!(spec.timeout_secs(), 900);
        assert_eq!(
            spec.default_model.as_deref(),
            Some("opencode/glm-5.3-flash-free")
        );
        assert_eq!(spec.models_filter.as_deref(), Some("-free$"));
        assert_eq!(
            spec.text_when,
            Some(TextWhen {
                path: "/type".to_string(),
                equals: "text".to_string(),
            })
        );
        assert_eq!(
            spec.env.get("OPENCODE_CONFIG").map(String::as_str),
            Some("{temp_config}")
        );
        assert!(spec.validate().is_ok());
    }

    // TC-338 (FR-333)
    #[test]
    fn timeout_message_mentions_config_for_user_agent() {
        let spec = CustomAgentSpec {
            name: "my-agent".to_owned(),
            label: Some("my-label".to_owned()),
            command: vec!["tool".to_owned()],
            builtin: false,
            ..Default::default()
        };
        let msg = timeout_message(&spec);
        assert!(msg.starts_with("my-label timed out after 1800s"));
        assert!(msg.contains("timeout_secs"));
        assert!(msg.contains("name = \"my-agent\""));
        assert!(msg.contains("raise timeout_secs"));
        assert!(msg.contains("partial edits"));
    }

    // TC-338 (FR-333)
    #[test]
    fn timeout_message_mentions_override_for_builtin_preset() {
        let spec = opencode_preset("opencode");
        let msg = timeout_message(&spec);
        assert!(msg.starts_with("opencode timed out after 1800s"));
        assert!(msg.contains("timeout_secs"));
        assert!(msg.contains("built-in \"opencode\""));
        assert!(msg.contains("dmuxai init"));
        assert!(msg.contains("partial edits"));
    }

    // TC-339 (FR-339)
    #[test]
    fn builtin_presets_use_default_timeout() {
        assert_eq!(
            opencode_preset("opencode").timeout_secs(),
            DEFAULT_TIMEOUT_SECS
        );
        assert_eq!(grok_preset("grok").timeout_secs(), DEFAULT_TIMEOUT_SECS);
    }

    // TC-1796 (FR-1796)
    #[test]
    fn tc_1796_idle_timeout_secs_defaults() {
        let unset: CustomAgentSpec = toml::from_str("name = \"a\"\ncommand = [\"a\"]").unwrap();
        assert_eq!(unset.idle_timeout_secs, None);
        assert_eq!(unset.idle_timeout_secs(), Some(300));

        let zero: CustomAgentSpec =
            toml::from_str("name = \"a\"\ncommand = [\"a\"]\nidle_timeout_secs = 0").unwrap();
        assert_eq!(zero.idle_timeout_secs, Some(0));
        assert_eq!(zero.idle_timeout_secs(), None);

        let explicit: CustomAgentSpec =
            toml::from_str("name = \"a\"\ncommand = [\"a\"]\nidle_timeout_secs = 45").unwrap();
        assert_eq!(explicit.idle_timeout_secs, Some(45));
        assert_eq!(explicit.idle_timeout_secs(), Some(45));
    }

    #[test]
    fn test_deserialize_minimal_spec() {
        let toml_str = r#"
name = "pi"
command = ["pi", "run"]
"#;
        let spec: CustomAgentSpec = toml::from_str(toml_str).unwrap();
        assert_eq!(spec.name, "pi");
        assert_eq!(spec.label(), "pi");
        assert_eq!(spec.color(), "white");
        assert_eq!(spec.prompt_via, PromptVia::Arg);
        assert_eq!(spec.effective_output(), CustomAgentOutput::Text);
        assert_eq!(spec.timeout_secs(), DEFAULT_TIMEOUT_SECS);
        assert!(spec.image_arg.is_empty());
        assert!(spec.env.is_empty());
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn tc_345_effective_output_infers_jsonl_when_text_path_set() {
        let toml_str = r#"
name = "test-agent"
command = ["sh", "-c", "{prompt}"]
text_path = "/part/text"
"#;
        let spec: CustomAgentSpec = toml::from_str(toml_str).unwrap();
        assert_eq!(
            spec.effective_output(),
            CustomAgentOutput::Jsonl,
            "inference: no output set but text_path configured => Jsonl"
        );
    }

    #[test]
    fn tc_346_effective_output_explicit_text_ignores_path_fields() {
        let toml_str = r#"
name = "test-agent"
command = ["sh", "-c", "{prompt}"]
output = "text"
text_path = "/part/text"
"#;
        let spec: CustomAgentSpec = toml::from_str(toml_str).unwrap();
        assert_eq!(
            spec.effective_output(),
            CustomAgentOutput::Text,
            "explicit output = text with text_path => Text (path fields ignored)"
        );
    }

    #[test]
    fn tc_347_effective_output_defaults_to_text_without_paths() {
        let toml_str = r#"
name = "test-agent"
command = ["sh", "-c", "{prompt}"]
"#;
        let spec: CustomAgentSpec = toml::from_str(toml_str).unwrap();
        assert_eq!(
            spec.effective_output(),
            CustomAgentOutput::Text,
            "no output, no path fields => Text (backward compat)"
        );
    }

    #[test]
    fn explicit_jsonl_output_preserved() {
        let toml_str = r#"
name = "test-agent"
command = ["sh", "-c", "{prompt}"]
output = "jsonl"
text_path = "/part/text"
"#;
        let spec: CustomAgentSpec = toml::from_str(toml_str).unwrap();
        assert_eq!(
            spec.effective_output(),
            CustomAgentOutput::Jsonl,
            "output = jsonl with text_path => Jsonl"
        );
    }

    #[test]
    fn test_validate_constraints() {
        let mut spec = CustomAgentSpec {
            name: "invalid name".to_string(),
            command: vec!["tool".to_string()],
            ..Default::default()
        };
        assert!(spec.validate().is_err());

        spec.name = "Upper".to_string();
        assert!(spec.validate().is_err());

        spec.name = "".to_string();
        assert!(spec.validate().is_err());

        spec.name = "valid_name-1".to_string();
        spec.command.clear();
        assert!(spec.validate().is_err());

        spec.command = vec!["tool".to_string()];
        spec.models_filter = Some("[unclosed".to_string());
        assert!(spec.validate().is_err());

        spec.models_filter = None;
        spec.allowed_models = Some("(?i+".to_string());
        assert!(spec.validate().is_err());

        spec.allowed_models = None;
        spec.failure_stderr = Some("(?invalid".to_string());
        assert!(spec.validate().is_err());

        spec.failure_stderr = Some("(?i)error".to_string());
        assert!(spec.validate().is_ok());

        spec.stall_keepalive = Some("(?invalid".to_string());
        assert!(spec.validate().is_err());

        spec.stall_keepalive = Some("keepalive:.*".to_string());
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_substitute_basic() {
        let ctx = SubstitutionContext {
            prompt: "Write a function".to_string(),
            model: Some("glm-4".to_string()),
            workspace: "/home/user/project".to_string(),
            images: vec!["/tmp/shot.png".to_string()],
            session_id: Some("sess-99".to_string()),
            temp_config: Some("/tmp/config.json".to_string()),
            mode: "implement",
            edit: "allow",
            system_prompt: None,
            system_prompt_file: None,
        };

        assert_eq!(
            substitute("{prompt}", &ctx).as_deref(),
            Some("Write a function")
        );
        assert_eq!(substitute("-m {model}", &ctx).as_deref(), Some("-m glm-4"));
        assert_eq!(
            substitute("{workspace}/file", &ctx).as_deref(),
            Some("/home/user/project/file")
        );
        assert_eq!(
            substitute("--image {image}", &ctx).as_deref(),
            Some("--image /tmp/shot.png")
        );
        assert_eq!(
            substitute("id={session_id}", &ctx).as_deref(),
            Some("id=sess-99")
        );
        assert_eq!(
            substitute("CONF={temp_config}", &ctx).as_deref(),
            Some("CONF=/tmp/config.json")
        );
        assert_eq!(
            substitute("mode={mode},edit={edit}", &ctx).as_deref(),
            Some("mode=implement,edit=allow")
        );
    }

    #[test]
    fn test_substitute_unset_placeholder_returns_none() {
        let ctx = SubstitutionContext {
            prompt: "Hi".to_string(),
            model: None,
            workspace: "/dir".to_string(),
            images: Vec::new(),
            session_id: None,
            temp_config: None,
            mode: "ask",
            edit: "deny",
            system_prompt: None,
            system_prompt_file: None,
        };

        assert_eq!(substitute("-m {model}", &ctx), None);
        assert_eq!(substitute("{session_id}", &ctx), None);
        assert_eq!(substitute("{temp_config}", &ctx), None);
        assert_eq!(substitute("{mcp_config}", &ctx), None);
        assert_eq!(substitute("{image}", &ctx), None);
        assert_eq!(
            substitute("static text", &ctx).as_deref(),
            Some("static text")
        );
    }

    #[test]
    fn test_build_argv_prompt_injection_prevented() {
        let spec: CustomAgentSpec = toml::from_str(OPENCODE_TOML).unwrap();
        let injection_prompt = "--model x; rm -rf /";
        let ctx = SubstitutionContext {
            prompt: injection_prompt.to_string(),
            model: Some("good-model".to_string()),
            workspace: "/workspace".to_string(),
            images: Vec::new(),
            session_id: None,
            temp_config: None,
            mode: "ask",
            edit: "deny",
            system_prompt: None,
            system_prompt_file: None,
        };

        let argv = build_argv(&spec, &ctx);
        // Prompt is passed as a single argv element without shell splitting
        assert_eq!(
            argv,
            vec![
                "opencode",
                "run",
                "--format",
                "json",
                "-m",
                "good-model",
                "--model x; rm -rf /",
            ]
        );
        assert_eq!(argv[6], injection_prompt);
    }

    #[test]
    fn test_build_argv_drop_unset_placeholder_rule() {
        let spec: CustomAgentSpec = toml::from_str(OPENCODE_TOML).unwrap();
        let ctx = SubstitutionContext {
            prompt: "Hello".to_string(),
            model: None, // unset model
            workspace: "/workspace".to_string(),
            images: Vec::new(),
            session_id: None,
            temp_config: None,
            mode: "ask",
            edit: "deny",
            system_prompt: None,
            system_prompt_file: None,
        };

        let argv = build_argv(&spec, &ctx);
        // "-m" and "{model}" must both be dropped
        assert_eq!(argv, vec!["opencode", "run", "--format", "json", "Hello"]);
    }

    // TC-1610 / FR-1610: the built-in opencode preset carries no `--dir`; the
    // workspace reaches the child only through the built command's cwd, which
    // every opencode build honours. The documented examples stay in sync.
    #[test]
    fn tc_1610_opencode_preset_has_no_dir_flag() {
        let spec = opencode_preset("opencode");
        assert!(
            spec.command.iter().all(|arg| arg != "--dir"),
            "{:?}",
            spec.command
        );
        assert_eq!(
            spec.command,
            vec![
                "opencode", "run", "--format", "json", "-m", "{model}", "{prompt}",
            ]
        );

        let mut ctx = SubstitutionContext::new("explain this", "/ws", "ask", "deny");
        ctx.model = Some("opencode/glm-5.3-flash-free".to_owned());
        assert_eq!(
            build_argv(&spec, &ctx),
            vec![
                "opencode",
                "run",
                "--format",
                "json",
                "-m",
                "opencode/glm-5.3-flash-free",
                "explain this",
            ]
        );

        // The workspace is passed as the child's cwd (FR-875), not a flag.
        let workspace = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(workspace.path()).unwrap();
        let ctx = SubstitutionContext::new(
            "explain this",
            workspace.to_string_lossy().into_owned(),
            "ask",
            "deny",
        );
        let command = build_custom_command(&spec, &ctx, &workspace).unwrap();
        assert_eq!(
            command.as_std().get_current_dir(),
            Some(workspace.as_path())
        );

        // The documented examples match the preset shape (no `--dir`).
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for file in [
            "README.md",
            "docs/agents/presets.toml",
            "docs/agents/custom-agents.md",
        ] {
            let text = std::fs::read_to_string(root.join(file)).unwrap();
            assert!(
                !text.contains(r#""--dir", "{workspace}""#),
                "{file} must pass the workspace as cwd, not --dir"
            );
        }
    }

    // TC-1611 / FR-1611: `build_argv` drops the legacy `--dir {workspace}` pair
    // for opencode programs only; other programs and other `--dir` values are
    // left unchanged.
    #[test]
    fn tc_1611_opencode_dir_pair_dropped() {
        let ctx = SubstitutionContext::new("hello there", "/ws", "ask", "deny");
        let spec_with_pair = |program: &str| -> CustomAgentSpec {
            toml::from_str(&format!(
                r#"
name = "legacy"
command = ["{program}", "run", "--format", "json", "--dir", "{{workspace}}", "{{prompt}}"]
"#
            ))
            .unwrap()
        };
        let spec_with_other_dir = |program: &str| -> CustomAgentSpec {
            toml::from_str(&format!(
                r#"
name = "other"
command = ["{program}", "run", "--dir", "/other", "{{prompt}}"]
"#
            ))
            .unwrap()
        };

        // opencode: the pair is dropped after substitution.
        assert_eq!(
            build_argv(&spec_with_pair("opencode"), &ctx),
            vec!["opencode", "run", "--format", "json", "hello there"]
        );
        // `opencode.exe` (ASCII case-insensitive basename) drops it too.
        assert_eq!(
            build_argv(&spec_with_pair("OPENCODE.EXE"), &ctx),
            vec!["OPENCODE.EXE", "run", "--format", "json", "hello there"]
        );
        // The pair is dropped from a mid-command position.
        let mid: CustomAgentSpec = toml::from_str(
            r#"
name = "mid"
command = ["opencode", "--dir", "{workspace}", "run", "{prompt}"]
"#,
        )
        .unwrap();
        assert_eq!(
            build_argv(&mid, &ctx),
            vec!["opencode", "run", "hello there"]
        );

        // A different program keeps the pair, even one whose name contains opencode.
        assert_eq!(
            build_argv(&spec_with_pair("myagent"), &ctx),
            vec![
                "myagent",
                "run",
                "--format",
                "json",
                "--dir",
                "/ws",
                "hello there"
            ]
        );
        assert_eq!(
            build_argv(&spec_with_pair("./tools/my-opencode"), &ctx),
            vec![
                "./tools/my-opencode",
                "run",
                "--format",
                "json",
                "--dir",
                "/ws",
                "hello there"
            ]
        );
        // `--dir` with any value other than {workspace} stays.
        assert_eq!(
            build_argv(&spec_with_other_dir("opencode"), &ctx),
            vec!["opencode", "run", "--dir", "/other", "hello there"]
        );
    }

    #[test]
    fn test_build_argv_prompt_via_stdin_drops_prompt() {
        let toml_str = r#"
name = "piper"
command = ["piper", "run", "--prompt", "{prompt}", "--flag"]
prompt_via = "stdin"
"#;
        let spec: CustomAgentSpec = toml::from_str(toml_str).unwrap();
        let ctx = SubstitutionContext {
            prompt: "secret prompt".to_string(),
            model: None,
            workspace: "/ws".to_string(),
            images: Vec::new(),
            session_id: None,
            temp_config: None,
            mode: "ask",
            edit: "deny",
            system_prompt: None,
            system_prompt_file: None,
        };

        let argv = build_argv(&spec, &ctx);
        // "--prompt" and "{prompt}" dropped, leaving "piper", "run", "--flag"
        assert_eq!(argv, vec!["piper", "run", "--flag"]);
    }

    #[test]
    fn test_build_argv_images_and_resume() {
        let toml_str = r#"
name = "runner"
command = ["runner", "{prompt}"]
image_arg = ["-f", "{image}"]
resume_arg = ["--resume", "{session_id}"]

[modes]
ask = ["--mode", "ask"]
plan = ["--mode", "plan"]
implement = ["--mode", "implement"]
"#;
        let spec: CustomAgentSpec = toml::from_str(toml_str).unwrap();
        let ctx = SubstitutionContext {
            prompt: "Inspect".to_string(),
            model: None,
            workspace: "/ws".to_string(),
            images: vec!["img1.png".to_string(), "img2.png".to_string()],
            session_id: Some("sess-123".to_string()),
            temp_config: None,
            mode: "implement",
            edit: "allow",
            system_prompt: None,
            system_prompt_file: None,
        };

        let argv = build_argv(&spec, &ctx);
        assert_eq!(
            argv,
            vec![
                "runner",
                "Inspect",
                "--mode",
                "implement",
                "-f",
                "img1.png",
                "-f",
                "img2.png",
                "--resume",
                "sess-123",
            ]
        );
    }

    #[test]
    fn test_build_argv_empty_image_arg_appends_to_prompt() {
        let toml_str = r#"
name = "pi"
command = ["pi", "{prompt}"]
image_arg = []
"#;
        let spec: CustomAgentSpec = toml::from_str(toml_str).unwrap();
        let ctx = SubstitutionContext {
            prompt: "Look at this".to_string(),
            model: None,
            workspace: "/ws".to_string(),
            images: vec!["a.png".to_string(), "b.png".to_string()],
            session_id: None,
            temp_config: None,
            mode: "ask",
            edit: "deny",
            system_prompt: None,
            system_prompt_file: None,
        };

        let argv = build_argv(&spec, &ctx);
        assert_eq!(argv, vec!["pi", "Look at this @a.png @b.png"]);
    }

    #[test]
    fn test_parse_jsonl_opencode_sample() {
        let spec: CustomAgentSpec = toml::from_str(OPENCODE_TOML).unwrap();

        let session_line = r#"{"type":"session","sessionID":"ses_abc123"}"#;
        let parsed_session = parse_jsonl_line(&spec, session_line);
        assert_eq!(parsed_session.text, None);
        assert_eq!(parsed_session.error, None);
        assert_eq!(parsed_session.session_id.as_deref(), Some("ses_abc123"));

        let step_line = r#"{"type":"step_start"}"#;
        let parsed_step = parse_jsonl_line(&spec, step_line);
        assert_eq!(parsed_step, ParsedLine::default());

        let text_line =
            r#"{"type":"text","part":{"text":"Rust is fast."},"sessionID":"ses_abc123"}"#;
        let parsed_text = parse_jsonl_line(&spec, text_line);
        assert_eq!(parsed_text.text.as_deref(), Some("Rust is fast."));
        assert_eq!(parsed_text.error, None);
        assert_eq!(parsed_text.session_id.as_deref(), Some("ses_abc123"));

        let error_line = r#"{"type":"error","error":{"message":"Quota depleted"}}"#;
        let parsed_error = parse_jsonl_line(&spec, error_line);
        assert_eq!(parsed_error.text, None);
        assert_eq!(parsed_error.error.as_deref(), Some("Quota depleted"));

        let invalid_line = "not a valid json object";
        assert_eq!(parse_jsonl_line(&spec, invalid_line), ParsedLine::default());
    }

    #[test]
    fn test_failure_reason() {
        let spec: CustomAgentSpec = toml::from_str(OPENCODE_TOML).unwrap();

        // 1. JSONL error takes priority
        let r1 = failure_reason(&spec, true, "some text", "", Some("Rate limited"));
        assert_eq!(r1.as_deref(), Some("Rate limited"));

        // 2. failure_stderr regex match
        let r2 = failure_reason(
            &spec,
            true,
            "some text",
            "Error: Permission Denied to execute bash",
            None,
        );
        assert_eq!(
            r2.as_deref(),
            Some("Error: Permission Denied to execute bash")
        );

        // 3. Non-zero exit with stderr
        let r3 = failure_reason(&spec, false, "", "Process failed miserably\n", None);
        assert_eq!(r3.as_deref(), Some("Process failed miserably"));

        // 4. Non-zero exit without stderr
        let r4 = failure_reason(&spec, false, "", "", None);
        assert!(
            r4.as_deref().is_some_and(|reason| reason
                .starts_with("process exited with non-zero status")
                && reason.contains("directly")),
            "{r4:?}"
        );

        // 5. Zero exit with empty reply text
        let r5 = failure_reason(&spec, true, "   \n  ", "", None);
        assert_eq!(r5.as_deref(), Some("empty reply text"));

        // 6. Success
        let r6 = failure_reason(&spec, true, "Everything succeeded!", "", None);
        assert_eq!(r6, None);
    }

    // TC-2125 / FR-2125: empty reply is only a success when the workspace
    // changed during an edit-allowed turn; every other failure signal wins.
    #[test]
    fn tc_2125_classify_turn_outcome_table() {
        let spec: CustomAgentSpec = toml::from_str(OPENCODE_TOML).unwrap();

        // Empty reply + workspace change -> success without a final message.
        assert_eq!(
            classify_turn_outcome(&spec, true, "   \n  ", "", None, true),
            TurnOutcome::SuccessWithoutReply
        );

        // Empty reply without a workspace change -> failure.
        assert_eq!(
            classify_turn_outcome(&spec, true, "   \n  ", "", None, false),
            TurnOutcome::Failed("empty reply text".to_string())
        );

        // A JSONL error wins over the workspace change.
        assert_eq!(
            classify_turn_outcome(&spec, true, "", "", Some("model not found"), true),
            TurnOutcome::Failed("model not found".to_string())
        );

        // A non-zero exit wins over the workspace change.
        let exit_failure = classify_turn_outcome(&spec, false, "", "boom\n", None, true);
        assert_eq!(exit_failure, TurnOutcome::Failed("boom".to_string()));

        // A failure_stderr match wins over the workspace change.
        let stderr_failure = classify_turn_outcome(
            &spec,
            true,
            "",
            "Error: Permission Denied to execute bash",
            None,
            true,
        );
        assert_eq!(
            stderr_failure,
            TurnOutcome::Failed("Error: Permission Denied to execute bash".to_string())
        );

        // A reply with text is a plain success.
        assert_eq!(
            classify_turn_outcome(&spec, true, "done", "", None, false),
            TurnOutcome::Success
        );
    }

    #[test]
    fn substituted_values_are_not_rescanned() {
        let spec: CustomAgentSpec =
            toml::from_str("name = \"x\"\ncommand = [\"x\", \"-m\", \"{model}\", \"{prompt}\"]")
                .unwrap();
        let mut ctx = SubstitutionContext::new(
            "use {model} and {workspace} {unknown}",
            "/ws",
            "ask",
            "deny",
        );
        ctx.model = Some("m1".to_owned());
        assert_eq!(
            build_argv(&spec, &ctx),
            vec!["x", "-m", "m1", "use {model} and {workspace} {unknown}"]
        );
    }

    #[test]
    fn models_are_filtered_and_policy_enforced() {
        let spec: CustomAgentSpec = toml::from_str(
            "name = \"oc\"\ncommand = [\"oc\"]\nmodels_filter = \"-free$\"\nallowed_models = \"^opencode/.*-free$\"",
        )
        .unwrap();
        assert_eq!(
            filter_models(&spec, "opencode/a-free\nopencode/b\n\nopencode/c-free\n"),
            vec!["opencode/a-free", "opencode/c-free"]
        );
        assert!(validate_custom_model(&spec, "opencode/a-free").is_ok());
        assert!(validate_custom_model(&spec, "opencode/b").is_err());
    }

    // TC-812 / FR-812: when the built-in preset cannot list an allowed model
    // (no match, non-zero exit, or timeout) the error names the policy and the
    // way out; a valid listing keeps only the allowed ids.
    #[cfg(unix)]
    #[tokio::test]
    async fn preset_without_allowed_models_explains_policy() {
        let env = RuntimeEnv::default();
        let cwd = std::env::temp_dir();
        let timeout = Some(Duration::from_millis(300));
        let with_script = |script: &str| {
            let mut spec = opencode_preset("opencode");
            spec.models_command = vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()];
            spec
        };
        let assert_policy = |error: &str| {
            assert!(error.starts_with("opencode: no allowed models"), "{error}");
            assert!(
                error.contains("allowed_models = ^opencode/.*-free$"),
                "{error}"
            );
            assert!(error.contains("dmuxai init"), "{error}");
        };

        let err = list_model_ids_with_env(
            &with_script("printf 'opencode/paid\\n'"),
            &cwd,
            timeout,
            &env,
        )
        .await
        .unwrap_err();
        assert_policy(&err);

        let err =
            list_model_ids_with_env(&with_script("echo boom >&2; exit 1"), &cwd, timeout, &env)
                .await
                .unwrap_err();
        assert_policy(&err);
        assert!(err.contains("boom"), "{err}");

        let err = list_model_ids_with_env(&with_script("sleep 5"), &cwd, timeout, &env)
            .await
            .unwrap_err();
        assert_policy(&err);
        assert!(err.contains("timed out"), "{err}");

        let ids = list_model_ids_with_env(
            &with_script("printf 'opencode/a-free\\nopencode/b\\nother/c-free\\n'"),
            &cwd,
            timeout,
            &env,
        )
        .await;
        assert_eq!(ids, Ok(vec!["opencode/a-free".to_owned()]));
    }

    // TC-1663 / FR-1663
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn tc_1663_custom_child_clears_inherited_mcp_env() {
        let _guard = crate::state_path::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(tmp.path()).unwrap();
        let spec = CustomAgentSpec {
            name: "env-probe".to_owned(),
            command: vec![
                "sh".to_owned(),
                "-c".to_owned(),
                "echo env=${DMUXAI_MCP_CONFIG:-}".to_owned(),
            ],
            ..Default::default()
        };
        let ctx = SubstitutionContext::new(
            "probe",
            workspace.to_string_lossy().into_owned(),
            "ask",
            "deny",
        );
        unsafe {
            std::env::set_var("DMUXAI_MCP_CONFIG", "/inherited/leak");
        }
        let mut cmd = build_custom_command(&spec, &ctx, &workspace).unwrap();
        let out = cmd.output().await.unwrap();
        unsafe {
            std::env::remove_var("DMUXAI_MCP_CONFIG");
        }
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "env=");
    }

    #[test]
    fn custom_child_applies_policy_and_respects_order_guarantees() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        let mut spec = CustomAgentSpec {
            name: "policy-test".to_owned(),
            command: vec!["echo".to_owned()],
            ..Default::default()
        };
        spec.env
            .insert("DMUXAI_CONFIG".to_owned(), "agent-override.toml".to_owned());
        spec.env
            .insert("CUSTOM_VAR".to_owned(), "custom-val".to_owned());

        let ctx = SubstitutionContext::new(
            "test",
            workspace.to_string_lossy().into_owned(),
            "ask",
            "deny",
        );

        let cmd = build_custom_command(&spec, &ctx, workspace).unwrap();
        let env_map: std::collections::HashMap<_, _> = cmd
            .as_std()
            .get_envs()
            .map(|(k, v)| (k.to_str().unwrap(), v.map(|v| v.to_str().unwrap())))
            .collect();

        // DMUXAI_WEB_TOKEN was stripped by policy
        assert_eq!(env_map.get("DMUXAI_WEB_TOKEN"), Some(&None));
        // DMUXAI_MCP_CONFIG was stripped unconditionally
        assert_eq!(env_map.get("DMUXAI_MCP_CONFIG"), Some(&None));
        // spec.env was applied last, so its override won
        assert_eq!(
            env_map.get("DMUXAI_CONFIG"),
            Some(&Some("agent-override.toml"))
        );
        assert_eq!(env_map.get("CUSTOM_VAR"), Some(&Some("custom-val")));
    }

    // TC-2082 / FR-2082: custom agent {mcp_config} placeholder dropped with one warning.
    #[test]
    fn tc_2082_custom_mcp_config_placeholder_dropped_with_warning() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("dmuxai.toml"),
            r#"
[[agents.custom]]
name = "mcp-agent"
command = ["x", "--mcp", "{mcp_config}", "{prompt}"]
"#,
        )
        .unwrap();

        let mut env = crate::runtime_env::RuntimeEnv::from_process();
        env.trust_workspace_config = true;
        let settings = crate::settings::DmuxSettings::load_with_env(tmp.path(), &env);
        let warnings: Vec<&String> = settings
            .config_warnings
            .iter()
            .filter(|w| w.contains("{mcp_config} is deprecated and ignored"))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "warnings: {:?}",
            settings.config_warnings
        );
        assert_eq!(
            warnings[0],
            "agents.custom \"mcp-agent\": {mcp_config} is deprecated and ignored; DmuxAI no longer passes MCP servers to agents (configure MCP in the agent itself)"
        );

        let spec = settings
            .custom_agents
            .iter()
            .find(|s| s.name == "mcp-agent")
            .unwrap();
        let ctx = SubstitutionContext::new("my prompt", "/ws", "ask", "deny");
        let argv = build_argv(spec, &ctx);
        assert_eq!(argv, vec!["x", "my prompt"]);
        assert!(!argv.contains(&"--mcp".to_string()));
        assert!(!argv.iter().any(|arg| arg.contains("{mcp_config}")));
    }

    // TC-877 (FR-875)
    #[cfg(unix)]
    #[tokio::test]
    async fn runner_cwd_is_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(tmp.path()).unwrap();
        let test_cwd = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
        assert_ne!(workspace, test_cwd);

        let spec = CustomAgentSpec {
            name: "probe".to_owned(),
            command: vec![
                "sh".to_owned(),
                "-c".to_owned(),
                "pwd; printf '%s\\n' \"$1\"".to_owned(),
                "--".to_owned(),
                "{workspace}".to_owned(),
            ],
            models_command: vec!["sh".to_owned(), "-c".to_owned(), "pwd".to_owned()],
            ..Default::default()
        };
        let ctx = SubstitutionContext::new(
            "check cwd",
            workspace.to_string_lossy().into_owned(),
            "ask",
            "deny",
        );
        let mut cmd = build_custom_command(&spec, &ctx, &workspace).unwrap();
        let out = cmd.output().await.unwrap();
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout);
        let lines: Vec<&str> = stdout.lines().map(str::trim).collect();
        let ws_str = workspace.to_string_lossy();
        assert_eq!(lines, vec![ws_str.as_ref(), ws_str.as_ref()]);

        let model_lines = list_model_ids(&spec, &workspace, Some(Duration::from_secs(5)))
            .await
            .unwrap();
        assert_eq!(model_lines, vec![ws_str.into_owned()]);
    }

    // TC-892 / FR-892, FR-894, NFR-890
    #[test]
    fn grok_preset_argv_is_headless_and_not_a_shell() {
        let spec = grok_preset("grok");
        let prompt = "hello; $(rm)";
        let base = [
            "grok",
            "-p",
            prompt,
            "-m",
            "grok-4.7",
            "--cwd",
            "/ws",
            "--output-format",
            "streaming-json",
            "--no-wait-for-background",
        ];
        for (mode, edit, extra) in [
            ("ask", "deny", &["--permission-mode", "plan"][..]),
            ("plan", "deny", &["--permission-mode", "plan"][..]),
            ("implement", "allow", &["--always-approve"][..]),
        ] {
            let mut ctx = SubstitutionContext::new(prompt, "/ws", mode, edit);
            ctx.model = Some("grok-4.7".to_owned());
            let argv = build_argv(&spec, &ctx);
            let mut expected: Vec<String> = base.iter().map(|s| (*s).to_owned()).collect();
            expected.extend(extra.iter().map(|s| (*s).to_owned()));
            assert_eq!(argv, expected, "{mode}");
            assert_eq!(argv.iter().filter(|arg| arg.as_str() == prompt).count(), 1);
            assert!(
                argv.iter()
                    .all(|arg| arg != "sh" && arg != "cmd" && arg != "cmd.exe")
            );
            if mode == "implement" {
                assert!(argv.iter().any(|arg| arg == "--always-approve"));
                assert!(!argv.iter().any(|arg| arg == "--permission-mode"));
            } else {
                assert!(!argv.iter().any(|arg| arg == "--always-approve"));
                assert!(
                    argv.windows(2)
                        .any(|pair| pair == ["--permission-mode", "plan"])
                );
            }
        }
        let mut resumed = SubstitutionContext::new(prompt, "/ws", "implement", "allow");
        resumed.model = Some("grok-4.7".to_owned());
        resumed.session_id = Some("11111111-1111-1111-1111-111111111111".to_owned());
        let argv = build_argv(&spec, &resumed);
        assert_eq!(
            argv[argv.len() - 2..],
            ["-r".to_owned(), resumed.session_id.clone().unwrap()]
        );
    }

    // TC-893 / FR-893, FR-894
    #[test]
    fn grok_streaming_json_extracts_text_and_session() {
        let spec = grok_preset("grok");
        let lines = [
            r#"{"type":"thought","data":"thinking"}"#,
            r#"{"type":"text","data":"OK"}"#,
            "{not json",
            r#"{"type":"text","data":"!"}"#,
            r#"{"type":"usage","usage":{"input":1}}"#,
            r#"{"type":"end","stopReason":"end_turn","sessionId":"11111111-1111-1111-1111-111111111111"}"#,
        ];
        let mut reply = String::new();
        let mut session = None;
        for line in lines {
            let parsed = parse_jsonl_line(&spec, line);
            if let Some(text) = parsed.text {
                reply.push_str(&text);
            }
            if parsed.session_id.is_some() {
                session = parsed.session_id;
            }
            assert!(parsed.error.is_none());
        }
        assert_eq!(reply, "OK!");
        assert_eq!(
            session.as_deref(),
            Some("11111111-1111-1111-1111-111111111111")
        );
        assert!(!reply.contains("thinking"));
        assert!(!reply.contains("usage"));
    }

    // TC-895 / FR-895
    #[test]
    fn grok_models_parse_default_filter_and_fast() {
        let listed = parse_grok_model_ids(
            "You are logged in with grok.com.\n* grok-4.7 (default)\n- grok-4.6\n- grok-4.7-build-fast\n- grok-4.7\n",
        );
        assert_eq!(listed, vec!["grok-4.7".to_owned(), "grok-4.6".to_owned()]);
        assert_eq!(
            select_grok_model(&listed, None).as_deref(),
            Some(GROK_DEFAULT_MODEL)
        );

        let default_on_other = parse_grok_model_ids("- grok-4.6 (default)\n* grok-4.7\n");
        assert_eq!(
            default_on_other,
            vec!["grok-4.6".to_owned(), "grok-4.7".to_owned()]
        );
        assert_eq!(
            select_grok_model(&default_on_other, None).as_deref(),
            Some(GROK_DEFAULT_MODEL)
        );

        let no_preferred = parse_grok_model_ids("- grok-4.6\n- grok-4.5\n");
        assert_eq!(
            select_grok_model(&no_preferred, None).as_deref(),
            Some("grok-4.6")
        );
        assert_eq!(
            select_grok_model(&no_preferred, Some("grok-4.5")).as_deref(),
            Some("grok-4.5")
        );

        let flash = parse_grok_model_ids("- grok-4.7-flash\n");
        assert_eq!(flash, vec!["grok-4.7-flash".to_owned()]);

        let fast_only =
            parse_grok_model_ids("You are logged in with grok.com.\n- grok-4.7-build-fast\n");
        assert!(fast_only.is_empty());
        let message = grok_no_allowed_model_message();
        assert!(message.contains("^grok-"), "{message}");
        assert!(message.contains("fast"), "{message}");
    }

    // TC-896 / FR-896, NFR-890
    #[test]
    fn grok_auth_is_models_stdout_only() {
        assert!(grok_models_authenticated(
            "You are logged in with grok.com.\n",
            true
        ));
        assert!(grok_models_authenticated("prefix LOGGED IN suffix", true));
        assert!(!grok_models_authenticated(
            "You are logged in with grok.com.\n",
            false
        ));
        assert!(!grok_models_authenticated(
            "Available models:\n* grok-4.7 (default)\n",
            false
        ));
        let login = grok_login_message();
        assert!(login.contains("grok login"), "{login}");
        assert_ne!(login, grok_no_allowed_model_message());
    }

    // TC-2170 / FR-2170: v1.0.62 listing with exit 0 is authenticated
    #[test]
    fn tc_2170_grok_v1_0_62_models_listing_authenticated() {
        let stdout = "\
Available models:
  * grok-4.7 (default)
  - grok-4.7-build-fast
  - grok-4.6
  - grok-4.5
";
        assert!(grok_models_authenticated(stdout, true));
    }

    // TC-2171 / FR-2170: "logged in" old format is authenticated
    #[test]
    fn tc_2171_grok_logged_in_old_format_authenticated() {
        assert!(grok_models_authenticated(
            "You are logged in with grok.com.\n",
            true
        ));
        assert!(grok_models_authenticated(
            "You are logged in with grok.com.\nAvailable models:\n  * grok-4.7 (default)\n",
            true
        ));
    }

    // TC-2172 / FR-2170: exit nonzero, empty list, or list without grok- ids is not authenticated
    #[test]
    fn tc_2172_grok_models_unauthenticated_failures() {
        let v1_0_62_stdout = "\
Available models:
  * grok-4.7 (default)
  - grok-4.7-build-fast
  - grok-4.6
  - grok-4.5
";
        // Exit nonzero
        assert!(!grok_models_authenticated(v1_0_62_stdout, false));
        assert!(!grok_models_authenticated(
            "You are logged in with grok.com.\n",
            false
        ));

        // Empty list / output
        assert!(!grok_models_authenticated("", true));
        assert!(!grok_models_authenticated("   \n\n  ", true));

        // List without grok- ids
        let no_grok_ids = "\
Available models:
  * other-model (default)
  - some-other-model
";
        assert!(!grok_models_authenticated(no_grok_ids, true));
    }

    // TC-899 / FR-898
    #[test]
    fn grok_preset_color_is_distinct() {
        let color = grok_preset("grok").color.expect("preset color");
        assert_eq!(color, "#6366f1");
        let parsed = crate::theme::parse_color(&color).expect("hex color");
        for other in [
            "cyan", "green", "magenta", "white", "gray", "#ffa500", "#d4af37", "#40e0d0",
            "#ff8800", "#4aa8ff", "#0b57d0",
        ] {
            let other_color = crate::theme::parse_color(other).expect(other);
            assert_ne!(parsed, other_color, "{other}");
            assert_ne!(color.as_str(), other);
        }
    }

    // TC-913 / FR-913: the grok preset turns auto-wake off and does not wait
    // for background work, and marks the end and tool-call lines.
    #[test]
    fn grok_preset_avoids_stranded_auto_wake() {
        let spec = grok_preset("grok");
        assert_eq!(
            spec.env.get("GROK_AUTO_WAKE").map(String::as_str),
            Some("0")
        );
        assert!(
            spec.command
                .iter()
                .any(|arg| arg == "--no-wait-for-background")
        );
        let when = |path: &str, equals: &str| TextWhen {
            path: path.to_owned(),
            equals: equals.to_owned(),
        };
        assert_eq!(spec.done_when, Some(when("/type", "end")));
        assert_eq!(spec.text_break_when, Some(when("/type", "tool_call")));
        let line = |text: &str| parse_jsonl_line(&spec, text);
        assert!(line(r#"{"type":"end","sessionId":"s"}"#).done);
        assert!(!line(r#"{"type":"text","data":"x"}"#).done);
        assert!(line(r#"{"type":"tool_call","toolCallId":"t"}"#).text_break);
        assert!(!line(r#"{"type":"tool_call_update"}"#).text_break);
    }

    // TC-1696 / FR-1683: preset drift guard for opencode error_path.
    // Preset drift guard: docs/agents/presets.toml opencode error_path == built-in.
    #[test]
    fn presets_opencode_error_path_matches_builtin() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(root.join("docs/agents/presets.toml")).unwrap();
        let value: toml::Value = toml::from_str(&text).unwrap();
        let customs = value
            .get("agents")
            .and_then(|v| v.get("custom"))
            .and_then(|v| v.as_array())
            .unwrap();
        let oc = customs
            .iter()
            .find(|v| v.get("name").and_then(|n| n.as_str()) == Some("opencode"))
            .unwrap();
        let preset: CustomAgentSpec = CustomAgentSpec::deserialize(oc.clone()).unwrap();
        let built = opencode_preset("opencode");
        assert_eq!(preset.error_path, built.error_path);
    }

    // TC-1697 / FR-1683: list pointer surfaces provider message.
    #[test]
    fn opencode_error_jsonl_surfaces_provider_message() {
        let spec = opencode_preset("opencode");
        let line = r#"{"type":"error","sessionID":"ses_x","error":{"name":"UnknownError","data":{"message":"Unexpected server error. Check server logs for details."}}}"#;
        let parsed = parse_jsonl_line(&spec, line);
        assert_eq!(
            parsed.error.as_deref(),
            Some("Unexpected server error. Check server logs for details.")
        );
    }

    // TC-1698 / FR-1683: single pointer still works.
    #[test]
    fn single_string_error_path_still_works() {
        let toml_str = r#"
name = "x"
command = ["x"]
error_path = "/error/message"
"#;
        let spec: CustomAgentSpec = toml::from_str(toml_str).unwrap();
        let line = r#"{"error":{"message":"boom"}}"#;
        let parsed = parse_jsonl_line(&spec, line);
        assert_eq!(parsed.error.as_deref(), Some("boom"));
    }

    // TC-1699 / FR-1683: compact JSON fallback.
    #[test]
    fn error_object_without_message_surfaces_compact_json() {
        let spec = opencode_preset("opencode");
        let line = r#"{"type":"error","sessionID":"ses_x","error":{"name":"UnknownError","data":{"foo":"bar"}}}"#;
        let parsed = parse_jsonl_line(&spec, line);
        assert_eq!(
            parsed.error.as_deref(),
            Some(r#"{"data":{"foo":"bar"},"name":"UnknownError"}"#)
        );
    }

    // TC-1722 / FR-1721: agy success /result object is not an error.
    #[test]
    fn tc_1722_agy_success_result_object_is_not_an_error() {
        let toml_str = r#"
name = "agy"
command = ["agy"]
output = "jsonl"
text_path = "/result/response"
text_when = { path = "/event", equals = "result" }
error_path = "/result/error"
"#;
        let spec: CustomAgentSpec = toml::from_str(toml_str).unwrap();
        let line = r#"{"event":"result","conversation_id":"c1","result":{"status":"SUCCESS","response":"done","error":null}}"#;
        let parsed = parse_jsonl_line(&spec, line);
        assert_eq!(parsed.error, None);
        assert_eq!(parsed.text.as_deref(), Some("done"));
    }

    // TC-1723 / FR-1721: agy success with a missing error key is not an error.
    #[test]
    fn tc_1723_agy_missing_error_key_is_not_an_error() {
        let toml_str = r#"
name = "agy"
command = ["agy"]
output = "jsonl"
text_path = "/result/response"
text_when = { path = "/event", equals = "result" }
error_path = "/result/error"
"#;
        let spec: CustomAgentSpec = toml::from_str(toml_str).unwrap();
        let line = r#"{"event":"result","conversation_id":"c1","result":{"status":"SUCCESS","response":"done"}}"#;
        let parsed = parse_jsonl_line(&spec, line);
        assert_eq!(parsed.error, None);
        assert_eq!(parsed.text.as_deref(), Some("done"));
    }

    // TC-1723 / FR-1721: empty error container still surfaces compact JSON.
    #[test]
    fn tc_1723_empty_error_container_still_surfaces() {
        let spec = opencode_preset("opencode");
        let line = r#"{"type":"error","error":{}}"#;
        let parsed = parse_jsonl_line(&spec, line);
        assert_eq!(parsed.error.as_deref(), Some("{}"));
    }

    // TC-1724 / FR-1721: agy error result surfaces the error string.
    #[test]
    fn tc_1724_agy_error_result_surfaces_error_string() {
        let toml_str = r#"
name = "agy"
command = ["agy"]
output = "jsonl"
text_path = "/result/response"
text_when = { path = "/event", equals = "result" }
error_path = "/result/error"
"#;
        let spec: CustomAgentSpec = toml::from_str(toml_str).unwrap();
        let line = r#"{"event":"result","conversation_id":"c1","result":{"status":"ERROR","error":"quota exhausted"}}"#;
        let parsed = parse_jsonl_line(&spec, line);
        assert_eq!(parsed.error.as_deref(), Some("quota exhausted"));
    }

    // TC-917 / FR-916: a pending break adds one blank line only between two
    // words, never at the start or after existing whitespace.
    #[test]
    fn append_reply_text_breaks_paragraphs_once() {
        let mut reply = String::new();
        let mut pending = true;
        assert_eq!(
            append_reply_text(&mut reply, "Checking", &mut pending),
            "Checking"
        );
        pending = true;
        assert_eq!(
            append_reply_text(&mut reply, "Done.", &mut pending),
            "\n\nDone."
        );
        assert!(!pending);
        assert_eq!(
            append_reply_text(&mut reply, " More", &mut pending),
            " More"
        );
        pending = true;
        reply.push('\n');
        assert_eq!(append_reply_text(&mut reply, "Next", &mut pending), "Next");
        pending = true;
        assert_eq!(
            append_reply_text(&mut reply, " tail", &mut pending),
            " tail"
        );
        assert_eq!(reply, "Checking\n\nDone. More\nNext tail");
    }

    // TC-2441 / FR-2440: a synthetic part is never reply text.
    #[test]
    fn tc_2441_synthetic_part_is_skipped() {
        let spec = opencode_preset("opencode");
        let synthetic = r#"{"type":"text","part":{"id":"prt_1","messageID":"msg_1","synthetic":true,"text":"Continue if you have next steps, or stop and ask for clarification if you are unsure how to proceed."}}"#;
        let parsed = parse_jsonl_line(&spec, synthetic);
        assert_eq!(parsed.text, None, "a synthetic part must not be reply text");

        let plain = r#"{"type":"text","part":{"id":"prt_2","messageID":"msg_1","text":"The real answer."}}"#;
        let parsed = parse_jsonl_line(&spec, plain);
        assert_eq!(parsed.text.as_deref(), Some("The real answer."));
    }

    // TC-2443 / FR-2440: the new JSONL options parse from `dmuxai.toml`.
    #[test]
    fn tc_2443_compaction_options_parse() {
        let toml_str = r#"
name = "opencode-like"
command = ["opencode", "run", "--format", "json"]
output = "jsonl"
text_path = "/part/text"
text_when = { path = "/type", equals = "text" }
text_skip_when = { path = "/part/synthetic", equals = true }
compaction_when = { path = "/part/metadata/compaction_continue", equals = true }
message_id_path = "/part/messageID"
"#;
        let spec: CustomAgentSpec = toml::from_str(toml_str).unwrap();
        assert_eq!(
            spec.text_skip_when,
            Some(TextWhen {
                path: "/part/synthetic".to_owned(),
                equals: "true".to_owned(),
            })
        );
        assert_eq!(
            spec.compaction_when,
            Some(TextWhen {
                path: "/part/metadata/compaction_continue".to_owned(),
                equals: "true".to_owned(),
            })
        );
        assert_eq!(spec.message_id_path.as_deref(), Some("/part/messageID"));
        assert!(spec.has_jsonl_path_fields());
        assert!(spec.validate().is_ok());

        // The built-in opencode preset ships the same options.
        let preset = opencode_preset("opencode");
        assert_eq!(preset.text_skip_when, spec.text_skip_when);
        assert_eq!(preset.compaction_when, spec.compaction_when);
        assert_eq!(preset.message_id_path, spec.message_id_path);
    }

    // TC-2440 / FR-2441: the compaction continuation drops the summary text of
    // the open message and keeps the text of the earlier messages.
    #[test]
    fn tc_2440_compaction_drops_summary_and_keeps_earlier_text() {
        let spec = opencode_preset("opencode");
        let mut collector = ReplyCollector::new(&spec);

        let narration = r#"{"type":"text","part":{"id":"prt_1","messageID":"msg_1","text":"Narration.\n"}}"#;
        let summary = r###"{"type":"text","part":{"id":"prt_2","messageID":"msg_2","text":"## Objective\n- summary\n"}}"###;
        let continuation = r#"{"type":"text","part":{"id":"prt_3","messageID":"msg_3","synthetic":true,"metadata":{"compaction_continue":true},"text":"Continue if you have next steps."}}"#;
        let answer = r#"{"type":"text","part":{"id":"prt_4","messageID":"msg_4","text":"The real answer.\n"}}"#;

        let mut streamed = String::new();
        for line in [narration, summary, continuation, answer] {
            let parsed = parse_jsonl_line(&spec, line);
            if let Some(chunk) = collector.push(&parsed) {
                streamed.push_str(&chunk);
            }
        }
        if let Some(chunk) = collector.finish() {
            streamed.push_str(&chunk);
        }

        assert_eq!(collector.reply(), "Narration.\nThe real answer.\n");
        assert_eq!(streamed, "Narration.\nThe real answer.\n");
        assert!(!collector.reply().contains("summary"));
        assert!(!collector.reply().contains("Continue if you have next steps"));
    }

    // TC-2444 / FR-2442: a runner without the new options keeps every text
    // part, synthetic ones included.
    #[test]
    fn tc_2444_runner_without_options_keeps_every_text_part() {
        let toml_str = r#"
name = "plain"
command = ["plain"]
output = "jsonl"
text_path = "/part/text"
text_when = { path = "/type", equals = "text" }
"#;
        let spec: CustomAgentSpec = toml::from_str(toml_str).unwrap();
        let mut collector = ReplyCollector::new(&spec);

        let continuation = r#"{"type":"text","part":{"id":"prt_1","messageID":"msg_1","synthetic":true,"text":"Continue if you have next steps."}}"#;
        let parsed = parse_jsonl_line(&spec, continuation);
        assert_eq!(parsed.text.as_deref(), Some("Continue if you have next steps."));
        assert_eq!(collector.push(&parsed).as_deref(), Some("Continue if you have next steps."));
        assert_eq!(collector.reply(), "Continue if you have next steps.");
    }
}
