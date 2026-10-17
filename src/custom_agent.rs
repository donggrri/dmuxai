//! Generic headless-command agents declared in `dmuxai.toml` (`[[agents.custom]]`).
//! See docs/agents/custom-agents.md.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Upper bound for the built-in preset's `models_command` (SPEC-first-run FR-813).
pub const BUILTIN_MODELS_TIMEOUT: Duration = Duration::from_secs(10);

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
    pub equals: String,
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
    pub output: CustomAgentOutput,
    #[serde(default)]
    pub text_path: Option<String>,
    #[serde(default)]
    pub text_when: Option<TextWhen>,
    #[serde(default)]
    pub error_path: Option<String>,
    #[serde(default)]
    pub session_id_path: Option<String>,
    #[serde(default)]
    pub resume_arg: Vec<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub failure_stderr: Option<String>,
    #[serde(default)]
    pub temp_config_template: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub modes: CustomAgentModes,
    /// True only for the built-in opencode preset (FR-810). Not configurable.
    #[serde(skip)]
    pub builtin: bool,
}

/// The built-in `opencode` runner (SPEC-first-run FR-810..FR-812): the documented
/// opencode custom agent with the free-model policy. `program` is the executable
/// that was found. `default_model` stays unset, so the first allowed listed model
/// is used.
pub fn opencode_preset(program: &str) -> CustomAgentSpec {
    let words = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    CustomAgentSpec {
        name: "opencode".to_owned(),
        label: Some("opencode".to_owned()),
        color: Some("#ffa500".to_owned()),
        command: [
            program,
            "run",
            "--format",
            "json",
            "-m",
            "{model}",
            "--dir",
            "{workspace}",
            "{prompt}",
        ]
        .map(str::to_owned)
        .to_vec(),
        prompt_via: PromptVia::Arg,
        image_arg: words(&["-f", "{image}"]),
        default_model: None,
        models_command: words(&[program, "models"]),
        models_filter: Some("-free$".to_owned()),
        allowed_models: Some("^opencode/.*-free$".to_owned()),
        output: CustomAgentOutput::Jsonl,
        text_path: Some("/part/text".to_owned()),
        text_when: Some(TextWhen {
            path: "/type".to_owned(),
            equals: "text".to_owned(),
        }),
        error_path: Some("/error/message".to_owned()),
        session_id_path: Some("/sessionID".to_owned()),
        resume_arg: words(&["--session", "{session_id}"]),
        timeout_secs: Some(900),
        failure_stderr: Some("(?i)permission denied|rejected permission".to_owned()),
        temp_config_template: Some(
            r#"{"permission":{"edit":"{edit}","bash":"{edit}","webfetch":"allow"}}"#.to_owned(),
        ),
        env: BTreeMap::from([("OPENCODE_CONFIG".to_owned(), "{temp_config}".to_owned())]),
        builtin: true,
        ..Default::default()
    }
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

    /// Returns the timeout in seconds, defaulting to 900.
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs.unwrap_or(900)
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
    pub mcp_config: Option<String>,
    pub mode: &'static str,
    pub edit: &'static str,
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
            mcp_config: None,
            mode,
            edit,
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
            "mcp_config" => Some(ctx.mcp_config.as_deref()?),
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

    argv
}

/// Result of parsing a single JSONL line emitted by a custom agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedLine {
    pub text: Option<String>,
    pub error: Option<String>,
    pub session_id: Option<String>,
}

/// Parses a line of JSONL output according to the agent's pointer configuration.
/// Invalid JSON lines are ignored (returning all fields `None`).
pub fn parse_jsonl_line(spec: &CustomAgentSpec, line: &str) -> ParsedLine {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
        return ParsedLine::default();
    };

    let text = if let Some(text_path) = &spec.text_path {
        let condition_met = match &spec.text_when {
            None => true,
            Some(when) => match val.pointer(&when.path) {
                Some(serde_json::Value::String(s)) => s == &when.equals,
                Some(v) => *v == when.equals,
                None => false,
            },
        };

        if condition_met {
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

    let error = spec.error_path.as_ref().and_then(|path| {
        val.pointer(path).and_then(|v| match v {
            serde_json::Value::String(s) => (!s.is_empty()).then(|| s.clone()),
            serde_json::Value::Null => None,
            other => Some(other.to_string()),
        })
    });

    let session_id = spec.session_id_path.as_ref().and_then(|path| {
        val.pointer(path).and_then(|v| match v {
            serde_json::Value::String(s) => (!s.is_empty()).then(|| s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            serde_json::Value::Null => None,
            other => Some(other.to_string()),
        })
    });

    ParsedLine {
        text,
        error,
        session_id,
    }
}

/// Determines whether a turn failed and returns an error reason if so.
pub fn failure_reason(
    spec: &CustomAgentSpec,
    exit_ok: bool,
    reply_text: &str,
    stderr: &str,
    jsonl_error: Option<&str>,
) -> Option<String> {
    if let Some(err) = jsonl_error.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(err.to_string());
    }

    if let Some(pattern) = &spec.failure_stderr
        && let Ok(re) = Regex::new(pattern)
        && re.is_match(stderr)
    {
        let trimmed = stderr.trim();
        return Some(if trimmed.is_empty() {
            "matched failure_stderr pattern".to_string()
        } else {
            trimmed.to_string()
        });
    }

    if !exit_ok {
        let trimmed = stderr.trim();
        return Some(if trimmed.is_empty() {
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
        });
    }

    if reply_text.trim().is_empty() {
        return Some("empty reply text".to_string());
    }

    None
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

/// Runs `models_command` and returns the filtered model ids (no policy applied).
async fn run_models_command(
    spec: &CustomAgentSpec,
    cwd: &Path,
    timeout: Option<Duration>,
) -> Result<Vec<String>, String> {
    let Some((program, args)) = spec.models_command.split_first() else {
        return Ok(Vec::new());
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .kill_on_drop(true);
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
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = match stderr.trim() {
            "" => format!(
                "exited with {} and printed nothing on stderr",
                output.status
            ),
            text => text.to_owned(),
        };
        return Err(format!("{} models failed: {detail}", spec.label()));
    }
    Ok(filter_models(
        spec,
        &String::from_utf8_lossy(&output.stdout),
    ))
}

/// Lists the model ids for `spec`, honouring the model policy (FR-812).
///
/// For the built-in preset (`spec.builtin`) a failure, a timeout, or a listing
/// with no `allowed_models` match is an error that names the policy and how to
/// override it. Other agents keep the plain listing.
pub async fn list_model_ids(
    spec: &CustomAgentSpec,
    cwd: &Path,
    timeout: Option<Duration>,
) -> Result<Vec<String>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    const OPENCODE_TOML: &str = r##"
name = "opencode"
label = "opencode"
color = "#ffa500"
command = ["opencode", "run", "--format", "json", "-m", "{model}", "--dir", "{workspace}", "{prompt}"]
prompt_via = "arg"
image_arg = ["-f", "{image}"]
default_model = "opencode/glm-5.3-flash-free"
models_command = ["opencode", "models"]
models_filter = "-free$"
allowed_models = "^opencode/.*-free$"
output = "jsonl"
text_path = "/part/text"
text_when = { path = "/type", equals = "text" }
error_path = "/error/message"
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
        assert_eq!(spec.output, CustomAgentOutput::Jsonl);
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
        assert_eq!(spec.output, CustomAgentOutput::Text);
        assert_eq!(spec.timeout_secs(), 900);
        assert!(spec.image_arg.is_empty());
        assert!(spec.env.is_empty());
        assert!(spec.validate().is_ok());
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
            mcp_config: Some("/tmp/mcp.json".to_string()),
            mode: "implement",
            edit: "allow",
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
            substitute("MCP={mcp_config}", &ctx).as_deref(),
            Some("MCP=/tmp/mcp.json")
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
            mcp_config: None,
            mode: "ask",
            edit: "deny",
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
            mcp_config: None,
            mode: "ask",
            edit: "deny",
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
                "--dir",
                "/workspace",
                "--model x; rm -rf /",
            ]
        );
        assert_eq!(argv[8], injection_prompt);
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
            mcp_config: None,
            mode: "ask",
            edit: "deny",
        };

        let argv = build_argv(&spec, &ctx);
        // "-m" and "{model}" must both be dropped
        assert_eq!(
            argv,
            vec![
                "opencode",
                "run",
                "--format",
                "json",
                "--dir",
                "/workspace",
                "Hello",
            ]
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
            mcp_config: None,
            mode: "ask",
            edit: "deny",
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
            mcp_config: None,
            mode: "implement",
            edit: "allow",
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
            mcp_config: None,
            mode: "ask",
            edit: "deny",
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

        let err = list_model_ids(&with_script("printf 'opencode/paid\\n'"), &cwd, timeout)
            .await
            .unwrap_err();
        assert_policy(&err);

        let err = list_model_ids(&with_script("echo boom >&2; exit 1"), &cwd, timeout)
            .await
            .unwrap_err();
        assert_policy(&err);
        assert!(err.contains("boom"), "{err}");

        let err = list_model_ids(&with_script("sleep 5"), &cwd, timeout)
            .await
            .unwrap_err();
        assert_policy(&err);
        assert!(err.contains("timed out"), "{err}");

        let ids = list_model_ids(
            &with_script("printf 'opencode/a-free\\nopencode/b\\nother/c-free\\n'"),
            &cwd,
            timeout,
        )
        .await;
        assert_eq!(ids, Ok(vec!["opencode/a-free".to_owned()]));
    }
}
