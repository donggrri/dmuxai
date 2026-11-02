use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::activity::UiSettings;
use crate::custom_agent::CustomAgentOutput;
use crate::notify::NotifySettings;
use crate::runner_protocol::RunnerProtocol;
use crate::runtime_env::RuntimeEnv;
use crate::theme::{Theme, ThemeToml};
use crate::workspace_trust::{
    TrustDecision, TrustStore, WorkspaceTrustStatus, content_hash, decide,
};

#[doc(hidden)]
pub use crate::state_path::test_env_lock;

/// Web dashboard theme settings loaded from `[web]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebThemeSettings {
    /// One of `light`, `dark` or `system` (default `system`).
    pub theme: String,
    /// Normalized lowercase `#rrggbb` accent, if configured.
    pub accent: Option<String>,
}

impl Default for WebThemeSettings {
    fn default() -> Self {
        Self {
            theme: "system".to_owned(),
            accent: None,
        }
    }
}

/// User-config `[workspace]` settings (FR-870, FR-872). Read only from the
/// user config, never from a workspace's `dmuxai.toml`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceSettings {
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub browse_roots: Vec<String>,
}

/// Workspace-local and environment overrides for DmuxAI behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmuxSettings {
    pub cursor_prewarm: bool,
    pub cursor_prewarm_idle_secs: u64,
    /// `[cursor] protocol` (default legacy stream-json; FR-1410).
    pub cursor_protocol: RunnerProtocol,
    /// `[agents] grok_protocol` (default ACP).
    pub grok_protocol: RunnerProtocol,
    pub routing_default_agent: Option<String>,
    pub routing_cursor_models: Vec<String>,
    pub graph_max_parallel: usize,
    pub graph_max_retries: u32,
    /// `[graph] max_replans` (FR-958), clamped to 0..=10; 0 disables replanning.
    pub graph_max_replans: u32,
    /// `[graph] replan_budget_chars` (FR-958), clamped to 2 000..=200 000.
    pub graph_replan_budget_chars: u32,
    /// `[graph] max_nodes` (FR-958), clamped to 1..=16.
    pub graph_max_nodes: u32,
    /// `[graph] tasks_file` (FR-967). When true, writes
    /// `<workspace>/.dmuxai/tasks/TASKS-<run id>.md` on approval and task status changes.
    pub graph_tasks_file: bool,
    pub graph_allowed_runners: Option<Vec<String>>,
    pub theme: Theme,
    pub theme_force_color: Option<bool>,
    pub reduced_motion: bool,
    /// `[[agents.custom]]` headless agents (installed into the registry at startup).
    pub custom_agents: Vec<crate::custom_agent::CustomAgentSpec>,
    /// `[agents] builtin_opencode` (default true). `false` hides the built-in
    /// opencode preset (SPEC-first-run FR-811).
    pub builtin_opencode: bool,
    /// `[agents] builtin_grok` (default true). `false` hides the built-in
    /// grok preset (SPEC-grok-runner FR-891).
    pub builtin_grok: bool,
    /// `[[mcp.servers]]` merged from user and repo configs by name (repo wins).
    pub mcp_servers: Vec<crate::mcp::McpServerSpec>,
    /// `[todo] jira_base_url` (user or repo, repo wins) used to derive links
    /// for to-dos that carry a Jira issue key (FR-564).
    pub todo_jira_base_url: Option<String>,
    /// Resolved `[jira]` settings (SPEC-jira-todos FR-2400); `None` = not configured.
    pub jira: Option<JiraSettings>,
    /// Activity rendering settings (tool calls and model thinking).
    pub ui: UiSettings,
    /// Web dashboard theme settings from `[web]`.
    pub web_theme: WebThemeSettings,
    /// Event delivery settings from `[notify]` (FR-770..FR-772).
    pub notify: NotifySettings,
    /// User-config `[workspace]` settings (FR-870, FR-872).
    pub workspace: WorkspaceSettings,
    /// Stage → skill mapping (SPEC-stage-skills).
    pub stage_skills: crate::stage_skills::StageSkillSettings,
    /// Problems found while reading dmuxai.toml, shown at startup.
    pub config_warnings: Vec<String>,
    /// Resolved `[memory]` (SPEC-obsidian-memory FR-720); `None` = disabled.
    pub memory: Option<crate::memory::MemoryConfig>,
    /// Raw `[memory]` keys merged per key (workspace wins), resolved in `load`.
    memory_raw: MemoryToml,
    /// Raw `[jira]` keys merged per key (workspace wins), resolved in `load`.
    jira_raw: JiraToml,
    /// Per-runner system prompt configs keyed by normalized runner ID (or "default") (PR-2.5).
    pub runner_prompts: BTreeMap<String, crate::agent_prompt::RunnerPromptConfig>,
    /// Per-runner model profile from `[runners.<id>] model` (FR-1731), keys
    /// lowercased runner ids. `[runners.default] model` is ignored with a warning.
    pub runner_models: BTreeMap<String, String>,
    /// Runner fallback chains and stall policy from `[runners.<id>]`
    /// `fallback`/`stall_*` (SPEC-runner-fallback, FR-2101).
    pub runner_fallback: crate::runner_fallback::FallbackSettings,
    /// Trust status of workspace configuration.
    pub workspace_trust: WorkspaceTrustStatus,
}

/// Convenience alias for [`DmuxSettings`].
pub type Settings = DmuxSettings;

/// Resolved `[jira]` settings (SPEC-jira-todos FR-2400).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraSettings {
    /// Absolute Jira Cloud base URL, trailing slash trimmed.
    pub base_url: String,
    pub email: String,
    pub jql: String,
    pub max_results: u32,
    pub token_env: String,
}

const DEFAULT_JIRA_JQL: &str =
    "assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC";
const DEFAULT_JIRA_MAX_RESULTS: u32 = 50;
const DEFAULT_JIRA_TOKEN_ENV: &str = "DMUXAI_JIRA_TOKEN";

#[derive(Debug, Default, Deserialize)]
struct DmuxToml {
    cursor: Option<CursorToml>,
    routing: Option<RoutingToml>,
    graph: Option<GraphToml>,
    theme: Option<ThemeToml>,
    agents: Option<AgentsToml>,
    ui: Option<UiSettings>,
    web: Option<WebToml>,
    notify: Option<NotifySettings>,
    memory: Option<MemoryToml>,
    mcp: Option<McpToml>,
    todo: Option<TodoToml>,
    jira: Option<JiraToml>,
    workspace: Option<WorkspaceSettings>,
    /// `[stages.<stage>]` skill mapping (SPEC-stage-skills FR-1500).
    stages: Option<std::collections::BTreeMap<String, crate::stage_skills::StageToml>>,
    /// `[skills]` sources (SPEC-stage-skills FR-1501).
    skills: Option<crate::stage_skills::SkillsToml>,
    /// Legacy execution modes table (removed); presence triggers a warning.
    modes: Option<LegacyModesToml>,
    /// `[runners.<id>]` prompt configuration (PR-2.5).
    runners: Option<std::collections::BTreeMap<String, crate::agent_prompt::RunnerPromptToml>>,
}

#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
struct LegacyModesToml {
    ask: Option<String>,
    plan: Option<String>,
    implement: Option<String>,
}

const MODES_SECTION_DEPRECATED: &str =
    "[modes] was removed; execution modes are gone — every turn runs in agent mode";

#[derive(Debug, Default, Deserialize)]
struct TodoToml {
    jira_base_url: Option<String>,
}

/// `[jira]` table (SPEC-jira-todos FR-2400).
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
struct JiraToml {
    base_url: Option<String>,
    email: Option<String>,
    jql: Option<String>,
    max_results: Option<i64>,
    token_env: Option<String>,
}

/// `[memory]` (FR-720).
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
struct MemoryToml {
    vault: Option<String>,
    dir: Option<String>,
    project: Option<String>,
    inject: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct WebToml {
    theme: Option<String>,
    accent: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct AgentsToml {
    /// `None` when the table has no `[[agents.custom]]`, so a `[agents]` table
    /// that only sets `builtin_opencode` or `builtin_grok` keeps the other
    /// config's agents.
    custom: Option<Vec<crate::custom_agent::CustomAgentSpec>>,
    builtin_opencode: Option<bool>,
    builtin_grok: Option<bool>,
    grok_protocol: Option<String>,
    opencode_protocol: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct McpToml {
    #[serde(default)]
    servers: Vec<crate::mcp::McpServerSpec>,
}

#[derive(Debug, Default, Deserialize)]
struct GraphToml {
    enabled: Option<bool>,
    max_parallel: Option<usize>,
    max_retries: Option<u32>,
    max_replans: Option<u32>,
    replan_budget_chars: Option<u32>,
    max_nodes: Option<u32>,
    tasks_file: Option<bool>,
    allowed_runners: Option<Vec<String>>,
}

/// Warning emitted once per settings load when `[graph] enabled` is present (FR-931).
const GRAPH_ENABLED_DEPRECATED: &str =
    "[graph] enabled is deprecated: graph engineering is always on; remove the key";

/// Defaults for the FR-958 replan caps.
const DEFAULT_GRAPH_MAX_REPLANS: u32 = 3;
const DEFAULT_GRAPH_REPLAN_BUDGET_CHARS: u32 = 24_000;
const DEFAULT_GRAPH_MAX_NODES: u32 = 12;

/// Validates a `[graph]` replan cap (FR-958): a value inside `range` is used;
/// an out-of-range value falls back to `default` with a config warning.
fn clamp_replan_limit(
    value: u32,
    range: std::ops::RangeInclusive<u32>,
    default: u32,
    key: &str,
) -> Result<u32, String> {
    if range.contains(&value) {
        return Ok(value);
    }
    Err(format!(
        "[graph] {key} = {value} is out of range {}..={}; using {default}",
        range.start(),
        range.end()
    ))
}

#[derive(Debug, Default, Deserialize)]
struct CursorToml {
    prewarm: Option<bool>,
    prewarm_idle_secs: Option<u64>,
    protocol: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RoutingToml {
    default_agent: Option<String>,
    cursor_models: Option<Vec<String>>,
}

impl DmuxSettings {
    pub fn load(workspace: &Path) -> Self {
        Self::load_with_env(workspace, &RuntimeEnv::from_process())
    }

    pub fn load_with_env(workspace: &Path, env: &RuntimeEnv) -> Self {
        let user_config_path = default_user_config_path_from(env);
        let exe = std::env::current_exe().ok();
        Self::load_with_runners_and_env(
            workspace,
            user_config_path.as_deref(),
            exe.as_deref(),
            find_opencode_bin_from(env),
            find_grok_bin_from(env),
            env,
        )
    }

    pub fn load_with_user_config(workspace: &Path, user_config_path: Option<&Path>) -> Self {
        Self::load_with_user_config_and_env(
            workspace,
            user_config_path,
            &RuntimeEnv::from_process(),
        )
    }

    pub fn load_with_user_config_and_env(
        workspace: &Path,
        user_config_path: Option<&Path>,
        env: &RuntimeEnv,
    ) -> Self {
        let exe = std::env::current_exe().ok();
        Self::load_with_runners_and_env(
            workspace,
            user_config_path,
            exe.as_deref(),
            find_opencode_bin_from(env),
            find_grok_bin_from(env),
            env,
        )
    }

    /// Like [`Self::load_with_user_config`] with the DmuxAI executable injected
    /// for the built-in `dmuxai-memory` entry (FR-733), so tests are deterministic.
    /// The opencode and grok executables are looked up in the environment.
    #[allow(dead_code)]
    pub(crate) fn load_with_exe(
        workspace: &Path,
        user_config_path: Option<&Path>,
        exe: Option<&Path>,
    ) -> Self {
        Self::load_with_runners_and_env(
            workspace,
            user_config_path,
            exe,
            find_opencode_bin(),
            find_grok_bin(),
            &RuntimeEnv::from_process(),
        )
    }

    /// Loads settings with the opencode executable injected (`None` = not
    /// installed) and no grok executable, so preset decisions stay deterministic
    /// in tests that predate the grok runner (FR-810).
    ///
    /// Production loading goes through [`Self::load_with_exe`]. This entry point
    /// is for tests, so the non-test build allows it to look unused.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn load_with_opencode(
        workspace: &Path,
        user_config_path: Option<&Path>,
        exe: Option<&Path>,
        opencode_bin: Option<PathBuf>,
    ) -> Self {
        Self::load_with_opencode_and_env(
            workspace,
            user_config_path,
            exe,
            opencode_bin,
            &RuntimeEnv::from_process(),
        )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn load_with_opencode_and_env(
        workspace: &Path,
        user_config_path: Option<&Path>,
        exe: Option<&Path>,
        opencode_bin: Option<PathBuf>,
        env: &RuntimeEnv,
    ) -> Self {
        Self::load_with_runners_and_env(workspace, user_config_path, exe, opencode_bin, None, env)
    }

    /// Loads settings with both runner executables injected (`None` = not
    /// installed), so preset registration is deterministic in tests (FR-810, FR-890).
    #[allow(dead_code)]
    pub(crate) fn load_with_runners(
        workspace: &Path,
        user_config_path: Option<&Path>,
        exe: Option<&Path>,
        opencode_bin: Option<PathBuf>,
        grok_bin: Option<PathBuf>,
    ) -> Self {
        Self::load_with_runners_and_env(
            workspace,
            user_config_path,
            exe,
            opencode_bin,
            grok_bin,
            &RuntimeEnv::from_process(),
        )
    }

    pub(crate) fn load_with_runners_and_env(
        workspace: &Path,
        user_config_path: Option<&Path>,
        _exe: Option<&Path>,
        opencode_bin: Option<PathBuf>,
        grok_bin: Option<PathBuf>,
        env: &RuntimeEnv,
    ) -> Self {
        let mut settings = Self::default();

        let mut configs: Vec<DmuxToml> = Vec::new();
        let mut user_runners = None;
        let mut user_fallback_tables: Option<
            BTreeMap<String, crate::runner_fallback::RunnerFallbackToml>,
        > = None;
        if let Some(user_path) = user_config_path
            && let Ok(contents) = std::fs::read_to_string(user_path)
        {
            match toml::from_str::<DmuxToml>(&contents) {
                Ok(mut parsed) => {
                    if let Some(ws) = parsed.workspace.take() {
                        settings.workspace = ws;
                    }
                    if let Some(runners) = parsed.runners.take() {
                        let base_dir = match user_path.parent() {
                            Some(p) if !p.as_os_str().is_empty() => p,
                            _ => Path::new("."),
                        };
                        let mut parsed_map = BTreeMap::new();
                        let mut fallback_map = BTreeMap::new();
                        for (id, raw) in runners {
                            let norm_id = id.trim().to_ascii_lowercase();
                            let cfg = crate::agent_prompt::RunnerPromptConfig::parse_table(
                                &norm_id,
                                &raw,
                                "user",
                                base_dir,
                                &mut settings.config_warnings,
                            );
                            parsed_map.insert(norm_id.clone(), cfg);
                            fallback_map.insert(
                                norm_id,
                                crate::runner_fallback::RunnerFallbackToml {
                                    fallback: raw.fallback.clone(),
                                    stall_timeout_secs: raw.stall_timeout_secs,
                                    stall_api_retries: raw.stall_api_retries,
                                },
                            );
                        }
                        user_runners = Some(parsed_map);
                        user_fallback_tables = Some(fallback_map);
                    }
                    configs.push(parsed);
                }
                Err(error) => settings.config_warnings.push(format!(
                    "user config {} ignored: {error}",
                    user_path.display()
                )),
            }
        }

        let config_path = workspace.join("dmuxai.toml");
        let mut workspace_runners = None;
        let mut workspace_fallback_tables: Option<
            BTreeMap<String, crate::runner_fallback::RunnerFallbackToml>,
        > = None;
        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            match toml::from_str::<DmuxToml>(&contents) {
                Ok(mut parsed) => {
                    let sha256 = content_hash(contents.as_bytes());
                    let restricted = restricted_workspace_keys_with_user(
                        &parsed,
                        workspace,
                        user_config_path,
                        env,
                    );
                    if restricted.is_empty() {
                        settings.workspace_trust = WorkspaceTrustStatus::NotNeeded;
                    } else {
                        let store = TrustStore::load(env);
                        if let Some(warning) = store.corrupt_warning() {
                            settings.config_warnings.push(warning.to_owned());
                        }
                        match decide(env, workspace, &sha256) {
                            TrustDecision::Trusted(source) => {
                                settings.workspace_trust = WorkspaceTrustStatus::Trusted(source);
                            }
                            TrustDecision::Untrusted => {
                                strip_restricted_workspace_keys_with_user(
                                    &mut parsed,
                                    workspace,
                                    user_config_path,
                                    env,
                                );
                                if let Some(skills) = &mut parsed.skills {
                                    for source in skills.sources.values_mut() {
                                        source.from_workspace = true;
                                    }
                                }
                                settings.stage_skills.confine_to =
                                    std::fs::canonicalize(workspace).ok().or_else(|| {
                                        if workspace.is_absolute() {
                                            Some(workspace.to_path_buf())
                                        } else {
                                            std::env::current_dir().ok().map(|d| d.join(workspace))
                                        }
                                    });
                                settings.config_warnings.push(format!(
                                    "workspace dmuxai.toml is not trusted; ignored: {}. Review it, then run `dmuxai trust` (or set DMUXAI_TRUST_WORKSPACE_CONFIG=1 for one run)",
                                    restricted.join(", ")
                                ));
                                settings.workspace_trust = WorkspaceTrustStatus::Untrusted {
                                    sha256,
                                    ignored: restricted.clone(),
                                };
                            }
                        }
                    }

                    if let Some(runners) = parsed.runners.take() {
                        let mut parsed_map = BTreeMap::new();
                        let mut fallback_map = BTreeMap::new();
                        for (id, raw) in runners {
                            let norm_id = id.trim().to_ascii_lowercase();
                            let cfg = crate::agent_prompt::RunnerPromptConfig::parse_table(
                                &norm_id,
                                &raw,
                                "workspace",
                                workspace,
                                &mut settings.config_warnings,
                            );
                            parsed_map.insert(norm_id.clone(), cfg);
                            fallback_map.insert(
                                norm_id,
                                crate::runner_fallback::RunnerFallbackToml {
                                    fallback: raw.fallback.clone(),
                                    stall_timeout_secs: raw.stall_timeout_secs,
                                    stall_api_retries: raw.stall_api_retries,
                                },
                            );
                        }
                        workspace_runners = Some(parsed_map);
                        workspace_fallback_tables = Some(fallback_map);
                    }
                    configs.push(parsed);
                }
                Err(error) => {
                    settings
                        .config_warnings
                        .push(format!("dmuxai.toml ignored: {error}"));
                }
            }
        }

        settings.runner_prompts =
            crate::agent_prompt::merge_runner_prompts(user_runners, workspace_runners);
        settings.runner_models = BTreeMap::new();

        // FR-810/FR-811 and FR-890/FR-891: a built-in preset is registered only
        // when its flag is not false, no config names that agent, and the
        // executable exists. This is decided before `allowed_runners` is
        // validated, so `allowed_runners = ["opencode"]` or `["grok"]` is accepted.
        // The last config's flag wins. A custom name in either file blocks the
        // preset even when the winning custom list no longer contains it.
        let agents = || configs.iter().filter_map(|parsed| parsed.agents.as_ref());
        settings.builtin_opencode = agents()
            .filter_map(|agents| agents.builtin_opencode)
            .next_back()
            .unwrap_or(true);
        settings.builtin_grok = agents()
            .filter_map(|agents| agents.builtin_grok)
            .next_back()
            .unwrap_or(true);
        let names_opencode = agents()
            .flat_map(|agents| agents.custom.iter().flatten())
            .any(|spec| spec.name.eq_ignore_ascii_case("opencode"));
        let names_grok = agents()
            .flat_map(|agents| agents.custom.iter().flatten())
            .any(|spec| spec.name.eq_ignore_ascii_case("grok"));
        let explicit_opencode = env
            .opencode_bin
            .clone()
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let opencode_preset_bin = if settings.builtin_opencode && !names_opencode {
            opencode_bin.or(explicit_opencode)
        } else {
            None
        };
        let grok_preset_bin = grok_bin.filter(|_| settings.builtin_grok && !names_grok);

        // Custom agent names seen in *either* config, so an id defined only in
        // the user config is not reported as unknown after the workspace config
        // replaces `[[agents.custom]]`. Pending presets are known too.
        let mut known_custom: Vec<String> = Vec::new();
        let mut pending_presets: Vec<&str> = Vec::new();
        if opencode_preset_bin.is_some() {
            known_custom.push("opencode".to_owned());
            pending_presets.push("opencode");
        }
        if grok_preset_bin.is_some() {
            known_custom.push("grok".to_owned());
            pending_presets.push("grok");
        }
        for parsed in configs {
            settings.apply_toml(parsed, &mut known_custom, &pending_presets);
        }
        let skill_warnings = settings.stage_skills.warnings();
        settings.config_warnings.extend(skill_warnings);
        // Appended last so existing custom agent indices do not move.
        // Opencode stays before grok.
        if let Some(program) = opencode_preset_bin
            && !settings
                .custom_agents
                .iter()
                .any(|spec| spec.name.eq_ignore_ascii_case("opencode"))
        {
            settings
                .custom_agents
                .push(crate::custom_agent::opencode_preset(
                    &program.to_string_lossy(),
                ));
        }
        if let Some(program) = grok_preset_bin
            && !settings
                .custom_agents
                .iter()
                .any(|spec| spec.name.eq_ignore_ascii_case("grok"))
        {
            settings
                .custom_agents
                .push(crate::custom_agent::grok_preset(&program.to_string_lossy()));
        }

        // SPEC-runner-fallback (FR-2101): build fallback settings now that
        // custom agents are known, so the validate closure can resolve them.
        {
            let custom_agents = &settings.custom_agents;
            let validate = |target: &crate::runner_fallback::FallbackTarget| -> Result<(), String> {
                let runner = target.runner.as_str();
                let is_builtin = matches!(runner, "cursor" | "agy" | "codex" | "opencode" | "grok");
                let custom_spec = custom_agents
                    .iter()
                    .find(|spec| spec.name.eq_ignore_ascii_case(runner));
                if is_builtin {
                    if let Some(model) = &target.model {
                        crate::model_policy::validate_model_for_runner(runner, model)
                    } else {
                        Ok(())
                    }
                } else if let Some(spec) = custom_spec {
                    if let Some(model) = &target.model {
                        crate::custom_agent::validate_custom_model(spec, model)
                    } else {
                        Ok(())
                    }
                } else {
                    Err(format!("unknown runner '{}'", runner))
                }
            };
            let user_tables = user_fallback_tables.unwrap_or_default();
            let ws_tables = workspace_fallback_tables.unwrap_or_default();
            let user_settings = crate::runner_fallback::FallbackSettings::from_tables(
                &user_tables,
                &validate,
                &mut settings.config_warnings,
            );
            let ws_settings = crate::runner_fallback::FallbackSettings::from_tables(
                &ws_tables,
                &validate,
                &mut settings.config_warnings,
            );
            settings.runner_fallback =
                crate::runner_fallback::FallbackSettings::merge(user_settings, ws_settings);
        }

        // FR-1731: validate per-runner model profiles now that custom agents
        // are known. Built-in runner ids use validate_model_for_runner; custom
        // agent ids use validate_custom_model; anything else is dropped.
        let runner_ids: Vec<String> = settings.runner_prompts.keys().cloned().collect();
        for id in runner_ids {
            let Some(cfg) = settings.runner_prompts.get(&id) else {
                continue;
            };
            let Some(model) = cfg.model.clone() else {
                continue;
            };
            let trimmed = model.trim();
            if trimmed.is_empty() {
                if let Some(cfg) = settings.runner_prompts.get_mut(&id) {
                    cfg.model = None;
                }
                continue;
            }
            let is_builtin = matches!(
                id.as_str(),
                "cursor" | "agy" | "codex" | "opencode" | "grok"
            );
            let custom_spec = settings
                .custom_agents
                .iter()
                .find(|spec| spec.name.eq_ignore_ascii_case(&id));
            if is_builtin {
                match crate::model_policy::validate_model_for_runner(&id, trimmed) {
                    Ok(()) => {
                        settings
                            .runner_models
                            .insert(id.clone(), trimmed.to_owned());
                    }
                    Err(err) => {
                        settings.config_warnings.push(format!(
                            "[runners.{id}] model '{trimmed}' is not allowed: {err}"
                        ));
                        if let Some(cfg) = settings.runner_prompts.get_mut(&id) {
                            cfg.model = None;
                        }
                    }
                }
            } else if let Some(spec) = custom_spec {
                match crate::custom_agent::validate_custom_model(spec, trimmed) {
                    Ok(()) => {
                        settings
                            .runner_models
                            .insert(id.clone(), trimmed.to_owned());
                    }
                    Err(err) => {
                        settings.config_warnings.push(format!(
                            "[runners.{id}] model '{trimmed}' is not allowed: {err}"
                        ));
                        if let Some(cfg) = settings.runner_prompts.get_mut(&id) {
                            cfg.model = None;
                        }
                    }
                }
            } else {
                settings.config_warnings.push(format!(
                    "[runners.{id}] model '{trimmed}' ignored: unknown runner '{id}'"
                ));
                if let Some(cfg) = settings.runner_prompts.get_mut(&id) {
                    cfg.model = None;
                }
            }
        }

        let mut known: Vec<&str> = vec!["cursor", "agy", "codex"];
        known.extend(known_custom.iter().map(String::as_str));
        let unknown = settings.theme.unknown_agent_ids(&known);
        drop(known);
        // Actually drop the offending entries so the "ignored" warning is true.
        settings
            .theme
            .agents
            .retain(|id, _| !unknown.iter().any(|u| u == id));
        for id in unknown {
            settings
                .config_warnings
                .push(format!("[theme.agents] unknown agent id '{id}' ignored"));
        }

        settings.resolve_memory(workspace, env);
        settings.resolve_jira();
        if env
            .reduced_motion
            .as_ref()
            .map(|v| {
                matches!(
                    v.to_string_lossy().trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
        {
            settings.reduced_motion = true;
            settings.theme.reduced_motion = true;
        }
        settings.dedupe_warnings();
        settings
    }

    /// SPEC-jira-todos FR-2400: merge one `[jira]` table, validating each key.
    fn merge_jira(&mut self, jira: JiraToml) {
        if let Some(base) = jira.base_url {
            match normalize_jira_config_base_url(&base) {
                Ok(normalized) => self.jira_raw.base_url = Some(normalized),
                Err(message) => self
                    .config_warnings
                    .push(format!("[jira] base_url ignored: {message}")),
            }
        }
        if let Some(email) = jira.email {
            match normalize_jira_email(&email) {
                Some(normalized) => self.jira_raw.email = Some(normalized),
                None => self
                    .config_warnings
                    .push("[jira] email ignored: must be a valid email address".to_owned()),
            }
        }
        if let Some(jql) = jira.jql {
            let trimmed = jql.trim();
            if !trimmed.is_empty() {
                self.jira_raw.jql = Some(trimmed.to_owned());
            }
        }
        if let Some(max_results) = jira.max_results {
            self.jira_raw.max_results = Some(max_results.clamp(1, 100));
        }
        if let Some(token_env) = jira.token_env {
            let trimmed = token_env.trim();
            if is_valid_env_name(trimmed) {
                self.jira_raw.token_env = Some(trimmed.to_owned());
            } else {
                self.config_warnings.push(format!(
                    "[jira] token_env ignored: must match [A-Z_][A-Z0-9_]*; using {DEFAULT_JIRA_TOKEN_ENV}"
                ));
                self.jira_raw.token_env = Some(DEFAULT_JIRA_TOKEN_ENV.to_owned());
            }
        }
    }

    /// SPEC-jira-todos FR-2400: build [`JiraSettings`] once every `[jira]` table
    /// has been merged. Configured requires a valid base URL and email.
    fn resolve_jira(&mut self) {
        let raw = std::mem::take(&mut self.jira_raw);
        let (Some(base_url), Some(email)) = (raw.base_url, raw.email) else {
            self.jira = None;
            return;
        };
        self.jira = Some(JiraSettings {
            base_url,
            email,
            jql: raw.jql.unwrap_or_else(|| DEFAULT_JIRA_JQL.to_owned()),
            max_results: raw.max_results.unwrap_or(DEFAULT_JIRA_MAX_RESULTS.into()) as u32,
            token_env: raw
                .token_env
                .unwrap_or_else(|| DEFAULT_JIRA_TOKEN_ENV.to_owned()),
        });
    }

    /// FR-720: turn the merged `[memory]` keys into a [`MemoryConfig`].
    fn resolve_memory(&mut self, workspace: &Path, env: &RuntimeEnv) {
        let raw = std::mem::take(&mut self.memory_raw);
        let Some(vault) = raw
            .vault
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        else {
            self.memory = None;
            return;
        };
        // A relative vault is relative to the workspace, not the process cwd.
        let vault_path = expand_home_from(vault, env);
        let vault_path = if vault_path.is_relative() {
            workspace.join(vault_path)
        } else {
            vault_path
        };
        let (root, dir) = match crate::memory::resolve_root(&vault_path, raw.dir.as_deref()) {
            Ok(resolved) => resolved,
            Err(message) => {
                let message = if vault_path.is_dir() {
                    format!("[memory] {message}; memory disabled")
                } else {
                    format!(
                        "[memory] vault = \"{vault}\" is not an existing directory; memory disabled"
                    )
                };
                self.config_warnings.push(message);
                self.memory = None;
                return;
            }
        };
        let canonical_workspace = std::fs::canonicalize(workspace).ok();
        let lexically_inside = vault_path.starts_with(workspace)
            || canonical_workspace
                .as_ref()
                .is_some_and(|canonical| vault_path.starts_with(canonical));
        if lexically_inside {
            let outside_workspace = |path: &Path| {
                let Ok(canonical) = std::fs::canonicalize(path) else {
                    return true;
                };
                canonical_workspace
                    .as_ref()
                    .is_none_or(|workspace| !canonical.starts_with(workspace))
            };
            if outside_workspace(&vault_path) || (root.exists() && outside_workspace(&root)) {
                self.config_warnings.push(
                    "[memory] vault resolves outside the workspace; memory disabled".to_owned(),
                );
                self.memory = None;
                return;
            }
        }
        let project = match raw.project.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(name) if crate::memory::is_valid_name(name) => Some(name.to_owned()),
            Some(name) => {
                self.config_warnings.push(format!(
                    "[memory] project = \"{name}\" must match [a-z0-9][a-z0-9_-]{{0,47}}; using the workspace name"
                ));
                None
            }
        };
        self.memory = Some(crate::memory::MemoryConfig {
            root,
            dir,
            project,
            inject: raw.inject.unwrap_or(true),
        });
    }

    /// Drop repeated warnings while preserving first-seen order.
    fn dedupe_warnings(&mut self) {
        for warning in &mut self.config_warnings {
            let escaped = crate::workspace_trust::escape_control(warning);
            *warning = escaped;
        }
        let mut seen = std::collections::HashSet::new();
        self.config_warnings.retain(|w| seen.insert(w.clone()));
    }

    fn apply_toml(
        &mut self,
        parsed: DmuxToml,
        known_custom: &mut Vec<String>,
        pending_presets: &[&str],
    ) {
        if parsed.modes.is_some() {
            self.config_warnings
                .push(MODES_SECTION_DEPRECATED.to_owned());
        }
        self.stage_skills.merge(parsed.stages, parsed.skills);
        if let Some(memory) = parsed.memory {
            if memory.vault.is_some() {
                self.memory_raw.vault = memory.vault;
            }
            if memory.dir.is_some() {
                self.memory_raw.dir = memory.dir;
            }
            if memory.project.is_some() {
                self.memory_raw.project = memory.project;
            }
            if memory.inject.is_some() {
                self.memory_raw.inject = memory.inject;
            }
        }
        if let Some(agents) = parsed.agents {
            if let Some(custom) = agents.custom {
                for spec in &custom {
                    known_custom.push(spec.name.clone());
                    if spec.output == Some(CustomAgentOutput::Text) && spec.has_jsonl_path_fields()
                    {
                        self.config_warnings.push(format!(
                            "[custom agent] {}: output = \"text\" is set but JSON-path fields are configured; set output = \"jsonl\" to use them",
                            spec.name
                        ));
                    }
                    if spec.uses_deprecated_mcp_config() {
                        self.config_warnings.push(format!(
                            "agents.custom \"{}\": {{mcp_config}} is deprecated and ignored; DmuxAI no longer passes MCP servers to agents (configure MCP in the agent itself)",
                            spec.name
                        ));
                    }
                }
                self.custom_agents = custom;
            }
            if let Some(protocol) = agents.grok_protocol {
                match RunnerProtocol::parse_config(&protocol) {
                    Some(value) => self.grok_protocol = value,
                    None => self.config_warnings.push(format!(
                        "[agents] grok_protocol ignored: expected acp or legacy, got {protocol:?}"
                    )),
                }
            }
            if agents.opencode_protocol.is_some() {
                // FR-1418: opencode stays on the headless `opencode run` path.
                self.config_warnings.push(
                    "[agents] opencode_protocol ignored: opencode runs headless (`opencode run`)"
                        .to_owned(),
                );
            }
        }
        if let Some(mcp) = parsed.mcp {
            self.merge_mcp_servers(mcp.servers);
        }
        if let Some(todo) = parsed.todo
            && let Some(base) = todo.jira_base_url
        {
            match normalize_jira_base_url(&base) {
                Ok(normalized) => self.todo_jira_base_url = Some(normalized),
                Err(message) => self
                    .config_warnings
                    .push(format!("[todo] jira_base_url ignored: {message}")),
            }
        }
        if let Some(jira) = parsed.jira {
            self.merge_jira(jira);
        }
        if let Some(cursor) = parsed.cursor {
            if let Some(prewarm) = cursor.prewarm {
                self.cursor_prewarm = prewarm;
            }
            if let Some(idle) = cursor.prewarm_idle_secs {
                self.cursor_prewarm_idle_secs = idle;
            }
            if let Some(protocol) = cursor.protocol {
                match RunnerProtocol::parse_config(&protocol) {
                    Some(value) => self.cursor_protocol = value,
                    None => self.config_warnings.push(format!(
                        "[cursor] protocol ignored: expected acp or legacy, got {protocol:?}"
                    )),
                }
            }
        }
        if let Some(routing) = parsed.routing {
            if let Some(default_agent) = routing.default_agent {
                self.routing_default_agent = Some(default_agent);
            }
            if let Some(cursor_models) = routing.cursor_models {
                self.routing_cursor_models = cursor_models;
            }
        }
        if let Some(theme) = &parsed.theme {
            self.theme
                .apply_toml_with_warnings(theme, &mut self.config_warnings);
            if let Some(force_color) = theme.force_color {
                self.theme_force_color = Some(force_color);
            }
            if let Some(rm) = theme.reduced_motion {
                self.reduced_motion = rm;
            }
        }
        if let Some(graph) = parsed.graph {
            if graph.enabled.is_some() {
                self.config_warnings
                    .push(GRAPH_ENABLED_DEPRECATED.to_owned());
            }
            if let Some(max_parallel) = graph.max_parallel {
                self.graph_max_parallel = max_parallel;
            }
            if let Some(max_retries) = graph.max_retries {
                self.graph_max_retries = max_retries;
            }
            if let Some(max_replans) = graph.max_replans {
                match clamp_replan_limit(
                    max_replans,
                    0..=10,
                    DEFAULT_GRAPH_MAX_REPLANS,
                    "max_replans",
                ) {
                    Ok(value) => self.graph_max_replans = value,
                    Err(warning) => self.config_warnings.push(warning),
                }
            }
            if let Some(budget) = graph.replan_budget_chars {
                match clamp_replan_limit(
                    budget,
                    2_000..=200_000,
                    DEFAULT_GRAPH_REPLAN_BUDGET_CHARS,
                    "replan_budget_chars",
                ) {
                    Ok(value) => self.graph_replan_budget_chars = value,
                    Err(warning) => self.config_warnings.push(warning),
                }
            }
            if let Some(max_nodes) = graph.max_nodes {
                match clamp_replan_limit(max_nodes, 1..=16, DEFAULT_GRAPH_MAX_NODES, "max_nodes") {
                    Ok(value) => self.graph_max_nodes = value,
                    Err(warning) => self.config_warnings.push(warning),
                }
            }
            if let Some(tasks_file) = graph.tasks_file {
                self.graph_tasks_file = tasks_file;
            }
            if let Some(runners) = graph.allowed_runners {
                let mut valid = Vec::new();
                for runner in runners {
                    let lower = runner.trim().to_ascii_lowercase();
                    if lower.is_empty() {
                        continue;
                    }
                    if matches!(lower.as_str(), "agy" | "codex" | "cursor")
                        || pending_presets
                            .iter()
                            .any(|name| name.eq_ignore_ascii_case(&lower))
                        || self
                            .custom_agents
                            .iter()
                            .any(|spec| spec.name.eq_ignore_ascii_case(&lower))
                        || crate::custom_agent::index_of(&lower).is_some()
                    {
                        if !valid.contains(&lower) {
                            valid.push(lower);
                        }
                    } else {
                        self.config_warnings.push(format!(
                            "unknown runner '{runner}' in graph.allowed_runners ignored"
                        ));
                    }
                }
                self.graph_allowed_runners = Some(valid);
            }
        }
        if let Some(ui) = parsed.ui {
            self.ui = ui;
        }
        if let Some(web) = parsed.web {
            if let Some(theme) = web.theme {
                match normalize_web_theme(&theme) {
                    Some(normalized) => self.web_theme.theme = normalized,
                    None => {
                        self.config_warnings.push(format!(
                            "[web] theme = \"{theme}\" is not a valid theme; using \"system\""
                        ));
                        self.web_theme.theme = "system".to_owned();
                    }
                }
            }
            if let Some(accent) = web.accent {
                match normalize_accent(&accent) {
                    Some(normalized) => self.web_theme.accent = Some(normalized),
                    None => {
                        self.config_warnings.push(format!(
                            "[web] accent = \"{accent}\" is not a valid color; using the default"
                        ));
                        self.web_theme.accent = None;
                    }
                }
            }
        }
        if let Some(notify) = parsed.notify {
            for warning in notify.validate() {
                self.config_warnings.push(format!("notify: {warning}"));
            }
            self.notify = notify;
        }
    }

    /// Merges `[[mcp.servers]]` entries by name: a later (repo) entry replaces
    /// an earlier (user) one, `enabled = false` removes it, and invalid entries
    /// become config warnings without reaching the resolved list.
    fn merge_mcp_servers(&mut self, incoming: Vec<crate::mcp::McpServerSpec>) {
        for spec in incoming {
            if spec.legacy_runners.is_some()
                && !self
                    .config_warnings
                    .iter()
                    .any(|w| w.contains("runners is deprecated and ignored"))
            {
                self.config_warnings.push(
                    "[[mcp.servers]] runners is deprecated and ignored; DmuxAI no longer passes MCP servers to agents (configure MCP in each agent)".to_owned(),
                );
            }
            if let Err(error) = spec.validate() {
                self.config_warnings.push(error);
                continue;
            }
            if !spec.enabled {
                self.mcp_servers.retain(|server| server.name != spec.name);
                continue;
            }
            match self
                .mcp_servers
                .iter_mut()
                .find(|server| server.name == spec.name)
            {
                Some(existing) => *existing = spec,
                None => self.mcp_servers.push(spec),
            }
        }
    }
}

impl DmuxSettings {
    /// Whether `runner` may take graph nodes (SPEC-first-run FR-813,
    /// SPEC-grok-runner FR-899). An unset `allowed_runners` admits every runner
    /// except built-in presets; a listed runner, including a preset, is admitted.
    pub fn graph_runner_allowed(&self, runner: &str) -> bool {
        let runner = runner.trim();
        match &self.graph_allowed_runners {
            Some(allowed) => allowed
                .iter()
                .any(|r| r.trim().eq_ignore_ascii_case(runner)),
            None => !self
                .custom_agents
                .iter()
                .any(|spec| spec.builtin && spec.name.eq_ignore_ascii_case(runner)),
        }
    }

    /// Pure resolver for a runner's system prompt (PR-2.5).
    pub fn resolve_agent_prompt(
        &self,
        runner: &str,
        role: crate::agent_prompt::PromptRole,
    ) -> Option<crate::agent_prompt::ResolvedPrompt> {
        crate::agent_prompt::resolve(self, runner, role)
    }

    /// Pure resolver for a turn's system prompt with per-model and built-in
    /// prompt support (SPEC-agent-system-prompt FR-2150..FR-2155).
    pub fn resolve_agent_prompt_for(
        &self,
        runner: &str,
        model: Option<&str>,
        role: crate::agent_prompt::PromptRole,
        write_turn: bool,
    ) -> Option<crate::agent_prompt::ResolvedPrompt> {
        crate::agent_prompt::resolve_for(self, runner, model, role, write_turn)
    }
}

fn path_inside_workspace(path: &Path, workspace: &Path) -> bool {
    let Ok(ws_norm) = std::fs::canonicalize(workspace) else {
        return false;
    };
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    // Reject any `..` component
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return false;
    }
    // Deepest existing ancestor
    let mut ancestor = candidate.as_path();
    let mut missing_suffixes = Vec::new();
    while !ancestor.exists() {
        if let Some(file_name) = ancestor.file_name() {
            missing_suffixes.push(file_name);
        }
        match ancestor.parent() {
            Some(p) if !p.as_os_str().is_empty() => ancestor = p,
            _ => return false,
        }
    }
    let Ok(mut canon) = std::fs::canonicalize(ancestor) else {
        return false;
    };
    for component in missing_suffixes.into_iter().rev() {
        canon.push(component);
    }
    canon == ws_norm || canon.starts_with(&ws_norm)
}

fn source_path_inside_workspace(
    source: &crate::stage_skills::SourceToml,
    workspace: &Path,
) -> bool {
    if let Some(sub) = &source.subdir {
        let sub_path = Path::new(sub);
        if sub_path.is_absolute()
            || sub.starts_with('/')
            || sub.starts_with('\\')
            || sub_path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return false;
        }
    }
    let Some(base_str) = &source.path else {
        return false;
    };
    let mut combined = if Path::new(base_str).is_absolute() {
        PathBuf::from(base_str)
    } else {
        workspace.join(base_str)
    };
    if let Some(sub) = &source.subdir {
        combined.push(sub);
    }
    path_inside_workspace(&combined, workspace)
}

fn prompt_file_outside_workspace(file_rel: &str, workspace: &Path) -> bool {
    let path = Path::new(file_rel);
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    !path_inside_workspace(&full, workspace)
}

fn restricted_workspace_keys(parsed: &DmuxToml, workspace: &Path, env: &RuntimeEnv) -> Vec<String> {
    restricted_workspace_keys_with_user(parsed, workspace, None, env)
}

fn restricted_workspace_keys_with_user(
    parsed: &DmuxToml,
    workspace: &Path,
    user_config_path: Option<&Path>,
    env: &RuntimeEnv,
) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(notify) = &parsed.notify {
        if notify.command.is_some() {
            keys.push("notify.command".to_owned());
        }
        if notify.webhook_url.is_some() {
            keys.push("notify.webhook_url".to_owned());
        }
        if notify.webhook_secret_env.is_some() {
            keys.push("notify.webhook_secret_env".to_owned());
        }
        if notify.secret_header.is_some() {
            keys.push("notify.secret_header".to_owned());
        }
    }
    if parsed
        .agents
        .as_ref()
        .and_then(|a| a.custom.as_ref())
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        keys.push("agents.custom".to_owned());
    }
    if parsed
        .mcp
        .as_ref()
        .map(|m| !m.servers.is_empty())
        .unwrap_or(false)
    {
        keys.push("mcp.servers".to_owned());
    }
    if let Some(skills) = &parsed.skills {
        for (name, source) in &skills.sources {
            if source.git.is_some() {
                keys.push(format!("skills.sources.{name}"));
                continue;
            }
            if source.path.is_some() && !source_path_inside_workspace(source, workspace) {
                keys.push(format!("skills.sources.{name}"));
            }
        }
    }
    if let Some(memory) = &parsed.memory {
        let (vault_key_restricted, dir_key_restricted) =
            memory_keys_restricted(memory, workspace, user_config_path, env);
        if vault_key_restricted {
            keys.push("memory.vault".to_owned());
        }
        if dir_key_restricted {
            keys.push("memory.dir".to_owned());
        }
    }
    if parsed
        .todo
        .as_ref()
        .and_then(|t| t.jira_base_url.as_ref())
        .is_some()
    {
        keys.push("todo.jira_base_url".to_owned());
    }
    if parsed.jira.is_some() {
        keys.push("jira".to_owned());
    }
    if let Some(runners) = &parsed.runners {
        for (id, raw) in runners {
            if let Some(file) = &raw.system_prompt_file
                && prompt_file_outside_workspace(file, workspace)
            {
                keys.push(format!("runners.{id}.system_prompt_file"));
            }
            if let Some(models) = &raw.models {
                for (model, entry) in models {
                    if let Some(file) = &entry.system_prompt_file
                        && prompt_file_outside_workspace(file, workspace)
                    {
                        keys.push(format!("runners.{id}.models.{model}.system_prompt_file"));
                    }
                }
            }
        }
    }
    keys
}

fn memory_keys_restricted(
    ws_memory: &MemoryToml,
    workspace: &Path,
    user_config_path: Option<&Path>,
    env: &RuntimeEnv,
) -> (bool, bool) {
    let mut ws_vault_restricted = false;
    let mut ws_dir_restricted = false;

    // Load user config if present to determine effective vault and dir
    let user_path = user_config_path
        .map(Path::to_path_buf)
        .or_else(|| default_user_config_path_from(env));
    let user_memory = user_path.and_then(|p| {
        let contents = std::fs::read_to_string(p).ok()?;
        toml::from_str::<DmuxToml>(&contents).ok()?.memory
    });

    let effective_vault_str = ws_memory
        .vault
        .as_deref()
        .or_else(|| user_memory.as_ref().and_then(|m| m.vault.as_deref()));

    let effective_dir_str = ws_memory
        .dir
        .as_deref()
        .or_else(|| user_memory.as_ref().and_then(|m| m.dir.as_deref()));

    if let Some(vault_str) = effective_vault_str {
        let trimmed_vault = vault_str.trim();
        if !trimmed_vault.is_empty() {
            let expanded_vault = expand_home_from(trimmed_vault, env);
            let vault_path = if expanded_vault.is_relative() {
                workspace.join(expanded_vault)
            } else {
                expanded_vault
            };

            let dir_normalized = effective_dir_str
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .unwrap_or(crate::memory::DEFAULT_DIR)
                .replace('\\', "/");
            let dir_path = Path::new(&dir_normalized);

            let bad_dir = dir_path.is_absolute()
                || dir_normalized.starts_with('/')
                || dir_path
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)));

            let outside = if bad_dir {
                true
            } else {
                let mut root = vault_path;
                for part in dir_normalized.split('/').filter(|p| !p.is_empty()) {
                    root.push(part);
                }
                !path_inside_workspace(&root, workspace)
            };

            if outside {
                if ws_memory.vault.is_some() {
                    ws_vault_restricted = true;
                }
                if ws_memory.dir.is_some() {
                    ws_dir_restricted = true;
                }
            }
        }
    } else {
        // Vault is not set, but workspace memory.dir might be specified with dotdot or absolute
        if let Some(dir) = &ws_memory.dir {
            let p = Path::new(dir);
            if p.is_absolute() || dir.contains("..") {
                ws_dir_restricted = true;
            }
        }
    }

    (ws_vault_restricted, ws_dir_restricted)
}

#[allow(dead_code)]
fn strip_restricted_workspace_keys(parsed: &mut DmuxToml, workspace: &Path, env: &RuntimeEnv) {
    strip_restricted_workspace_keys_with_user(parsed, workspace, None, env);
}

fn strip_restricted_workspace_keys_with_user(
    parsed: &mut DmuxToml,
    workspace: &Path,
    user_config_path: Option<&Path>,
    env: &RuntimeEnv,
) {
    if let Some(notify) = &mut parsed.notify {
        notify.command = None;
        notify.webhook_url = None;
        notify.webhook_secret_env = None;
        notify.secret_header = None;
    }
    if let Some(agents) = &mut parsed.agents {
        agents.custom = None;
    }
    parsed.mcp = None;
    if let Some(skills) = &mut parsed.skills {
        skills.sources.retain(|_name, source| {
            if source.git.is_some() {
                return false;
            }
            if source.path.is_some() && !source_path_inside_workspace(source, workspace) {
                return false;
            }
            true
        });
    }
    if let Some(memory) = &mut parsed.memory {
        let (strip_vault, strip_dir) =
            memory_keys_restricted(memory, workspace, user_config_path, env);
        if strip_vault {
            memory.vault = None;
        }
        if strip_dir {
            memory.dir = None;
        }
    }
    if let Some(todo) = &mut parsed.todo {
        todo.jira_base_url = None;
    }
    parsed.jira = None;
    if let Some(runners) = &mut parsed.runners {
        for raw in runners.values_mut() {
            if let Some(file) = &raw.system_prompt_file
                && prompt_file_outside_workspace(file, workspace)
            {
                raw.system_prompt_file = None;
            }
            if let Some(models) = &mut raw.models {
                for entry in models.values_mut() {
                    if let Some(file) = &entry.system_prompt_file
                        && prompt_file_outside_workspace(file, workspace)
                    {
                        entry.system_prompt_file = None;
                    }
                }
            }
        }
    }
}

/// Formats a command argument vector as a shell-quoted-ish string.
fn format_command_argv(cmd: &[String]) -> String {
    cmd.iter()
        .map(|arg| {
            if arg.is_empty() {
                "\"\"".to_string()
            } else if arg.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '=')
            }) {
                arg.clone()
            } else {
                format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Returns the key-value details of all restricted keys defined in a workspace's `dmuxai.toml`.
///
/// For security reasons, environment variable values are never read or printed;
/// only environment variable names are included.
pub fn restricted_workspace_details(workspace: &Path, env: &RuntimeEnv) -> Vec<(String, String)> {
    let config_path = workspace.join("dmuxai.toml");
    let Ok(contents) = std::fs::read_to_string(&config_path) else {
        return Vec::new();
    };
    restricted_workspace_details_from(&contents, workspace, env)
}

/// Returns the key-value details of all restricted keys in `contents`, a workspace's `dmuxai.toml`.
///
/// Keys and values are escaped for terminal display.
pub fn restricted_workspace_details_from(
    contents: &str,
    workspace: &Path,
    env: &RuntimeEnv,
) -> Vec<(String, String)> {
    let Ok(parsed) = toml::from_str::<DmuxToml>(contents) else {
        return Vec::new();
    };

    let mut details = Vec::new();

    if let Some(notify) = &parsed.notify {
        if let Some(cmd) = &notify.command {
            details.push(("notify.command".to_owned(), format_command_argv(cmd)));
        }
        if let Some(url) = &notify.webhook_url {
            details.push(("notify.webhook_url".to_owned(), url.clone()));
        }
        if let Some(secret_env) = &notify.webhook_secret_env {
            details.push(("notify.webhook_secret_env".to_owned(), secret_env.clone()));
        }
        if let Some(secret_header) = &notify.secret_header {
            details.push(("notify.secret_header".to_owned(), secret_header.clone()));
        }
    }

    if let Some(agents) = &parsed.agents
        && let Some(custom) = &agents.custom
    {
        for agent in custom {
            let mut parts = Vec::new();
            if !agent.command.is_empty() {
                parts.push(format!("command: {}", format_command_argv(&agent.command)));
            }
            if !agent.models_command.is_empty() {
                parts.push(format!(
                    "models_command: {}",
                    format_command_argv(&agent.models_command)
                ));
            }
            if !agent.env.is_empty() {
                let env_keys: Vec<_> = agent.env.keys().cloned().collect();
                parts.push(format!("env: [{}]", env_keys.join(", ")));
            }
            let val = if parts.is_empty() {
                "(custom agent)".to_owned()
            } else {
                parts.join(", ")
            };
            details.push((format!("agents.custom.{}", agent.name), val));
        }
    }

    if let Some(mcp) = &parsed.mcp {
        for server in &mcp.servers {
            let mut parts = Vec::new();
            if let Some(cmd) = &server.command {
                let mut argv = vec![cmd.clone()];
                argv.extend(server.args.clone());
                parts.push(format!("command: {}", format_command_argv(&argv)));
            }
            if let Some(url) = &server.url {
                parts.push(format!("url: {url}"));
            }
            if !server.env.is_empty() {
                let env_keys: Vec<_> = server.env.keys().cloned().collect();
                parts.push(format!("env: [{}]", env_keys.join(", ")));
            }
            let val = if parts.is_empty() {
                "(mcp server)".to_owned()
            } else {
                parts.join(", ")
            };
            details.push((format!("mcp.servers.{}", server.name), val));
        }
    }

    if let Some(skills) = &parsed.skills {
        for (name, source) in &skills.sources {
            if let Some(git) = &source.git {
                let rev = source.rev.as_deref().unwrap_or("HEAD");
                let sub = source
                    .subdir
                    .as_deref()
                    .map(|s| format!(" subdir: {s}"))
                    .unwrap_or_default();
                details.push((
                    format!("skills.sources.{name}"),
                    format!("git: {git} rev: {rev}{sub}"),
                ));
            } else if source.path.is_some() && !source_path_inside_workspace(source, workspace) {
                let p = source.path.as_deref().unwrap_or_default();
                let sub = source
                    .subdir
                    .as_deref()
                    .map(|s| format!(" subdir: {s}"))
                    .unwrap_or_default();
                details.push((format!("skills.sources.{name}"), format!("path: {p}{sub}")));
            }
        }
    }

    if let Some(memory) = &parsed.memory {
        let (vault_key_restricted, dir_key_restricted) =
            memory_keys_restricted(memory, workspace, None, env);
        if vault_key_restricted {
            details.push((
                "memory.vault".to_owned(),
                memory.vault.clone().unwrap_or_default(),
            ));
        }
        if dir_key_restricted {
            details.push((
                "memory.dir".to_owned(),
                memory.dir.clone().unwrap_or_default(),
            ));
        }
    }

    if let Some(todo) = &parsed.todo
        && let Some(jira) = &todo.jira_base_url
    {
        details.push(("todo.jira_base_url".to_owned(), jira.clone()));
    }

    if let Some(jira) = &parsed.jira {
        let mut parts = Vec::new();
        if let Some(base) = &jira.base_url {
            parts.push(format!("base_url: {base}"));
        }
        if let Some(email) = &jira.email {
            parts.push(format!("email: {email}"));
        }
        if let Some(token_env) = &jira.token_env {
            parts.push(format!("token_env: {token_env}"));
        }
        let value = if parts.is_empty() {
            "(jira settings)".to_owned()
        } else {
            parts.join(", ")
        };
        details.push(("jira".to_owned(), value));
    }

    if let Some(runners) = &parsed.runners {
        for (id, raw) in runners {
            if let Some(file) = &raw.system_prompt_file
                && prompt_file_outside_workspace(file, workspace)
            {
                details.push((format!("runners.{id}.system_prompt_file"), file.clone()));
            }
            if let Some(models) = &raw.models {
                for (model, entry) in models {
                    if let Some(file) = &entry.system_prompt_file
                        && prompt_file_outside_workspace(file, workspace)
                    {
                        details.push((
                            format!("runners.{id}.models.{model}.system_prompt_file"),
                            file.clone(),
                        ));
                    }
                }
            }
        }
    }

    details
        .into_iter()
        .map(|(key, value)| {
            (
                crate::workspace_trust::escape_control(&key),
                crate::workspace_trust::escape_control(&value),
            )
        })
        .collect()
}

/// Inspect a workspace's `dmuxai.toml` for trust purposes.
///
/// Returns `(exists, sha256, restricted_keys)` where `exists` is true when the
/// file is present and parses, `sha256` is the hash of its raw bytes, and
/// `restricted_keys` lists the keys that would be stripped if untrusted.
pub fn inspect_workspace_trust(workspace: &Path, env: &RuntimeEnv) -> (bool, String, Vec<String>) {
    let (exists, sha256, keys, _) = inspect_workspace_trust_full(workspace, env);
    (exists, sha256, keys)
}

/// Inspect a workspace's `dmuxai.toml` for trust purposes, reading it only once.
///
/// Returns `(exists, sha256, restricted_keys, details)` where `details` are the
/// restricted key-value pairs built from the same contents.
pub fn inspect_workspace_trust_full(
    workspace: &Path,
    env: &RuntimeEnv,
) -> (bool, String, Vec<String>, Vec<(String, String)>) {
    let config_path = workspace.join("dmuxai.toml");
    let Ok(contents) = std::fs::read_to_string(&config_path) else {
        return (false, String::new(), Vec::new(), Vec::new());
    };
    let details = restricted_workspace_details_from(&contents, workspace, env);
    let Ok(parsed) = toml::from_str::<DmuxToml>(&contents) else {
        return (true, content_hash(contents.as_bytes()), Vec::new(), details);
    };
    let keys = restricted_workspace_keys(&parsed, workspace, env);
    (true, content_hash(contents.as_bytes()), keys, details)
}

impl Default for DmuxSettings {
    fn default() -> Self {
        Self {
            cursor_prewarm: true,
            cursor_prewarm_idle_secs: 300,
            cursor_protocol: RunnerProtocol::Legacy,
            grok_protocol: RunnerProtocol::Acp,
            routing_default_agent: None,
            routing_cursor_models: Vec::new(),
            graph_max_parallel: 2,
            graph_max_retries: 2,
            graph_max_replans: DEFAULT_GRAPH_MAX_REPLANS,
            graph_replan_budget_chars: DEFAULT_GRAPH_REPLAN_BUDGET_CHARS,
            graph_max_nodes: DEFAULT_GRAPH_MAX_NODES,
            graph_tasks_file: false,
            graph_allowed_runners: None,
            theme: Theme::default(),
            theme_force_color: None,
            reduced_motion: false,
            custom_agents: Vec::new(),
            builtin_opencode: true,
            builtin_grok: true,
            mcp_servers: Vec::new(),
            todo_jira_base_url: None,
            jira: None,
            ui: UiSettings::default(),
            web_theme: WebThemeSettings::default(),
            notify: NotifySettings::default(),
            workspace: WorkspaceSettings::default(),
            stage_skills: Default::default(),
            config_warnings: Vec::new(),
            memory: None,
            memory_raw: MemoryToml::default(),
            jira_raw: JiraToml::default(),
            runner_prompts: BTreeMap::new(),
            runner_models: BTreeMap::new(),
            runner_fallback: Default::default(),
            workspace_trust: WorkspaceTrustStatus::NoWorkspaceConfig,
        }
    }
}

/// Loads `[workspace]` from the user config (`default_user_config_path()`) only (FR-870, FR-872).
pub fn load_user_workspace_settings() -> WorkspaceSettings {
    load_user_workspace_settings_from(default_user_config_path().as_deref())
}

/// Loads `[workspace]` from the given user config path (never from a workspace's `dmuxai.toml`).
pub fn load_user_workspace_settings_from(user_config_path: Option<&Path>) -> WorkspaceSettings {
    let Some(user_path) = user_config_path else {
        return WorkspaceSettings::default();
    };
    let Ok(contents) = std::fs::read_to_string(user_path) else {
        return WorkspaceSettings::default();
    };
    toml::from_str::<DmuxToml>(&contents)
        .ok()
        .and_then(|parsed| parsed.workspace)
        .unwrap_or_default()
}

/// Expand a leading `~/` (or `~\\`) with `HOME` / `USERPROFILE`.
pub fn expand_home(path: &str) -> PathBuf {
    expand_home_from(path, &RuntimeEnv::from_process())
}

/// Expand a leading `~/` (or `~\`) using the provided environment.
pub fn expand_home_from(path: &str, env: &RuntimeEnv) -> PathBuf {
    let rest = path
        .strip_prefix("~/")
        .or_else(|| path.strip_prefix("~\\"))
        .or_else(|| if path == "~" { Some("") } else { None });
    if let Some(rest) = rest
        && let Some(home) = env.home_dir()
    {
        let mut p = home;
        if !rest.is_empty() {
            p.push(rest);
        }
        return p;
    }
    PathBuf::from(path)
}

/// Normalize a `[web] theme` value, or `None` when it is not light/dark/system.
fn normalize_web_theme(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "light" => Some("light".to_owned()),
        "dark" => Some("dark".to_owned()),
        "system" => Some("system".to_owned()),
        _ => None,
    }
}

/// Normalize a `[web] accent` value to lowercase `#rrggbb`, or `None` if invalid.
///
/// Only `#rrggbb` hex (case-insensitive) is accepted; named colors are rejected.
fn normalize_accent(value: &str) -> Option<String> {
    let hex = value.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("#{}", hex.to_ascii_lowercase()))
}

/// Validates `[todo] jira_base_url` (FR-564): an absolute `http(s)` URL with a
/// host and no query/fragment; a trailing slash is trimmed.
fn normalize_jira_base_url(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("http://")
        .map(|_| &trimmed["http://".len()..])
        .or_else(|| {
            lower
                .strip_prefix("https://")
                .map(|_| &trimmed["https://".len()..])
        })
        .ok_or_else(|| "must be an absolute http(s) URL".to_owned())?;
    if trimmed.contains('?') || trimmed.contains('#') {
        return Err("must not contain a query or fragment".to_owned());
    }
    let end = rest.find('/').unwrap_or(rest.len());
    if rest[..end].is_empty() {
        return Err("must include a host".to_owned());
    }
    Ok(trimmed.trim_end_matches('/').to_owned())
}

/// Validates `[jira] base_url` (SPEC-jira-todos FR-2400): an absolute https URL,
/// or plain http only for loopback hosts. Trailing slashes are trimmed.
fn normalize_jira_config_base_url(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.contains('?') || trimmed.contains('#') {
        return Err("must not contain a query or fragment".to_owned());
    }
    let lower = trimmed.to_ascii_lowercase();
    let (secure, prefix_len) = if lower.starts_with("https://") {
        (true, "https://".len())
    } else if lower.starts_with("http://") {
        (false, "http://".len())
    } else {
        return Err("must be an absolute https URL".to_owned());
    };
    let rest = &trimmed[prefix_len..];
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return Err("must include a host".to_owned());
    }
    let host = if let Some(rest) = authority.strip_prefix('[') {
        match rest.split_once(']') {
            Some((host, _)) => host,
            None => return Err("must include a valid host".to_owned()),
        }
    } else {
        authority.split(':').next().unwrap_or(authority)
    };
    let loopback = matches!(
        host.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "::1" | "localhost"
    );
    if !secure && !loopback {
        return Err("plain http is allowed only for loopback hosts".to_owned());
    }
    Ok(trimmed.trim_end_matches('/').to_owned())
}

/// Basic RFC-ish email shape check for `[jira] email`; never echoes a value.
fn normalize_jira_email(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    let (local, domain) = trimmed.split_once('@')?;
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        return None;
    }
    Some(trimmed.to_owned())
}

/// Returns true when `value` matches `^[A-Z_][A-Z0-9_]*$`.
fn is_valid_env_name(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_uppercase() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// Finds the opencode executable (SPEC-first-run FR-810). `DMUXAI_OPENCODE_BIN`
/// is authoritative when set: it must name an existing file. Otherwise the first
/// `opencode` file on `PATH` is used.
pub fn find_opencode_bin() -> Option<PathBuf> {
    find_opencode_bin_from(&RuntimeEnv::from_process())
}

/// Lookup used by [`find_opencode_bin`]. `env.opencode_bin` is authoritative when set.
pub fn find_opencode_bin_from(env: &RuntimeEnv) -> Option<PathBuf> {
    if let Some(raw) = &env.opencode_bin {
        let path = PathBuf::from(raw);
        return path.is_file().then_some(path);
    }
    if let Some(found) = find_on_path_in("opencode", env.path.as_deref()) {
        return Some(found);
    }
    env.home_dir()
        .map(|h| h.join(".opencode").join("bin").join("opencode"))
        .filter(|p| p.is_file())
}

/// When `DMUXAI_OPENCODE_BIN` is set but does not name an existing file, returns
/// a user-facing message (SPEC-acp-runners FR-1326).
pub fn opencode_bin_explicit_missing_message() -> Option<String> {
    opencode_bin_explicit_missing_message_from(RuntimeEnv::from_process())
}

/// Argument accepted by [`opencode_bin_explicit_missing_message_from`].
pub enum OpencodeMissingArg {
    Env(Box<RuntimeEnv>),
    Explicit(Option<std::ffi::OsString>),
}

impl From<&RuntimeEnv> for OpencodeMissingArg {
    fn from(env: &RuntimeEnv) -> Self {
        Self::Env(Box::new(env.clone()))
    }
}

impl From<RuntimeEnv> for OpencodeMissingArg {
    fn from(env: RuntimeEnv) -> Self {
        Self::Env(Box::new(env))
    }
}

impl From<Option<std::ffi::OsString>> for OpencodeMissingArg {
    fn from(value: Option<std::ffi::OsString>) -> Self {
        Self::Explicit(value)
    }
}

impl From<std::ffi::OsString> for OpencodeMissingArg {
    fn from(value: std::ffi::OsString) -> Self {
        Self::Explicit(Some(value))
    }
}

impl From<&std::ffi::OsString> for OpencodeMissingArg {
    fn from(value: &std::ffi::OsString) -> Self {
        Self::Explicit(Some(value.clone()))
    }
}

impl From<Option<PathBuf>> for OpencodeMissingArg {
    fn from(value: Option<PathBuf>) -> Self {
        Self::Explicit(value.map(|p| p.into_os_string()))
    }
}

impl From<PathBuf> for OpencodeMissingArg {
    fn from(value: PathBuf) -> Self {
        Self::Explicit(Some(value.into_os_string()))
    }
}

pub fn opencode_bin_explicit_missing_message_from(
    arg: impl Into<OpencodeMissingArg>,
) -> Option<String> {
    let explicit = match arg.into() {
        OpencodeMissingArg::Env(env) => env.opencode_bin.clone(),
        OpencodeMissingArg::Explicit(value) => value,
    };
    let raw = explicit?;
    let path = PathBuf::from(raw);
    if path.is_file() {
        return None;
    }
    Some(format!(
        "opencode binary not found at {} (set by DMUXAI_OPENCODE_BIN)",
        path.display()
    ))
}

/// Finds the grok executable (SPEC-grok-runner FR-890). A non-empty
/// `DMUXAI_GROK_BIN` is authoritative and does not fall through. Otherwise the
/// first `grok` on `PATH`, then `~/.grok/bin/grok` when that file is executable.
pub fn find_grok_bin() -> Option<PathBuf> {
    find_grok_bin_from(&RuntimeEnv::from_process())
}

/// Lookup used by [`find_grok_bin`]. `env.grok_bin` `Some` is authoritative even
/// when the path is missing.
pub fn find_grok_bin_from(env: &RuntimeEnv) -> Option<PathBuf> {
    if let Some(raw) = &env.grok_bin {
        let path = PathBuf::from(raw);
        return path.is_file().then_some(path);
    }
    if let Some(found) = find_on_path_in("grok", env.path.as_deref()) {
        return Some(found);
    }
    let home = env.home_dir()?;
    let candidate = home.join(".grok").join("bin").join("grok");
    is_executable_file(&candidate).then_some(candidate)
}

/// Searches `PATH` from `env` for the first file called `name`. No shell is involved.
pub fn find_on_path_from(name: &str, env: &RuntimeEnv) -> Option<PathBuf> {
    find_on_path_in(name, env.path.as_deref())
}

/// Searches `PATH` for the first file called `name`. No shell is involved.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    find_on_path_from(name, &RuntimeEnv::from_process())
}

fn find_on_path_in(name: &str, path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let path_var = path_var?;
    let file_names = [name.to_owned()];
    std::env::split_paths(path_var)
        .flat_map(|dir| {
            file_names
                .iter()
                .map(move |file| dir.join(file))
                .collect::<Vec<_>>()
        })
        .find(|candidate| is_executable_file(candidate))
}

/// A regular file the current user may execute (any execute bit).
pub fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    use std::os::unix::fs::PermissionsExt;
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}

pub fn default_user_config_path() -> Option<PathBuf> {
    default_user_config_path_from(&RuntimeEnv::from_process())
}

pub fn default_user_config_path_from(env: &RuntimeEnv) -> Option<PathBuf> {
    if let Some(config) = &env.config_path {
        return Some(config.clone());
    }
    if let Some(base) = &env.xdg_config_home
        && !base.as_os_str().is_empty()
    {
        return Some(base.join("dmuxai").join("dmuxai.toml"));
    }
    let home = env.home_dir();
    if let Some(home_path) = home
        && !home_path.as_os_str().is_empty()
    {
        return Some(home_path.join(".config").join("dmuxai").join("dmuxai.toml"));
    }
    None
}

#[cfg(test)]
mod theme_config_tests {
    use super::*;
    use crate::activity::{ShowThinking, ShowTools};
    use ratatui::style::Color;

    #[test]
    fn parses_theme_section() {
        let dir = std::env::temp_dir().join(format!("dmuxai-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("dmuxai.toml"),
            "[theme]\nuser = \"lightblue\"\ncursor = \"#00ff00\"\nagy = \"bogus\"\n",
        )
        .unwrap();
        let settings = DmuxSettings::load(&dir);
        assert_eq!(settings.theme.user, Color::LightBlue);
        assert_eq!(settings.theme.cursor, Color::Rgb(0, 255, 0));
        assert_eq!(settings.theme.agy, Theme::default().agy);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn web_theme_settings_parse_with_fallbacks() {
        // TC-605, FR-603
        let temp_base = tempfile::tempdir().unwrap();
        let user_dir = temp_base.path().join("user");
        let ws_dir = temp_base.path().join("workspace");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&ws_dir).unwrap();

        let user_toml = user_dir.join("dmuxai.toml");
        std::fs::write(
            &user_toml,
            "[web]\ntheme = \"light\"\naccent = \"#AABBCC\"\n",
        )
        .unwrap();
        // The workspace config overrides the user config.
        std::fs::write(
            ws_dir.join("dmuxai.toml"),
            "[web]\ntheme = \"dark\"\naccent = \"#3366ff\"\n",
        )
        .unwrap();
        let settings = DmuxSettings::load_with_user_config(&ws_dir, Some(&user_toml));
        assert_eq!(settings.web_theme.theme, "dark");
        assert_eq!(settings.web_theme.accent.as_deref(), Some("#3366ff"));

        // Defaults when `[web]` is absent.
        let empty_dir = temp_base.path().join("empty");
        std::fs::create_dir_all(&empty_dir).unwrap();
        let defaults = DmuxSettings::load_with_user_config(&empty_dir, None);
        assert_eq!(defaults.web_theme.theme, "system");
        assert_eq!(defaults.web_theme.accent, None);

        // Invalid values fall back with warnings.
        let bad_dir = temp_base.path().join("bad");
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::write(
            bad_dir.join("dmuxai.toml"),
            "[web]\ntheme = \"neon\"\naccent = \"not-a-color\"\n",
        )
        .unwrap();
        let bad = DmuxSettings::load_with_user_config(&bad_dir, None);
        assert_eq!(bad.web_theme.theme, "system");
        assert_eq!(bad.web_theme.accent, None);
        assert!(
            bad.config_warnings
                .iter()
                .any(|w| w.contains("[web] theme = \"neon\""))
        );
        assert!(
            bad.config_warnings
                .iter()
                .any(|w| w.contains("[web] accent = \"not-a-color\""))
        );

        // Normalization: hex accent is lowercased and theme is lowercased.
        let norm_dir = temp_base.path().join("norm");
        std::fs::create_dir_all(&norm_dir).unwrap();
        std::fs::write(
            norm_dir.join("dmuxai.toml"),
            "[web]\ntheme = \"DARK\"\naccent = \"#AABBCC\"\n",
        )
        .unwrap();
        let norm = DmuxSettings::load_with_user_config(&norm_dir, None);
        assert_eq!(norm.web_theme.theme, "dark");
        assert_eq!(norm.web_theme.accent.as_deref(), Some("#aabbcc"));

        // Only `#rrggbb` is a valid accent; a named color falls back with a warning.
        let named_dir = temp_base.path().join("named");
        std::fs::create_dir_all(&named_dir).unwrap();
        std::fs::write(named_dir.join("dmuxai.toml"), "[web]\naccent = \"red\"\n").unwrap();
        let named = DmuxSettings::load_with_user_config(&named_dir, None);
        assert_eq!(named.web_theme.accent, None);
        assert!(
            named
                .config_warnings
                .iter()
                .any(|w| w.contains("[web] accent = \"red\""))
        );
    }

    #[test]
    fn theme_agents_real_nesting_and_unknown_ids() {
        // TC-602, FR-601: parse `[theme]` + `[theme.agents]` through the real
        // config path, honor custom agents from either config, and drop unknown
        // ids with a truthful warning. Identical warnings are deduped.
        let temp_base =
            std::env::temp_dir().join(format!("dmuxai-theme-agents-{}", std::process::id()));
        let user_dir = temp_base.join("user");
        let ws_dir = temp_base.join("workspace");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&ws_dir).unwrap();

        let user_toml = user_dir.join("dmuxai.toml");
        std::fs::write(
            &user_toml,
            r##"
[theme]
background = "#101010"
foreground = "dark-gray"
border = "bogus"

[theme.agents]
cursor = "yellow"
user-only = "#ff8800"

[[agents.custom]]
name = "user-only"
command = ["user-only"]
"##,
        )
        .unwrap();

        // The workspace replaces `[[agents.custom]]`; theme colors still merge.
        std::fs::write(
            ws_dir.join("dmuxai.toml"),
            r##"
[theme]
border = "bogus"

[theme.agents]
workspace-only = "#123456"
mystery = "cyan"

[[agents.custom]]
name = "workspace-only"
command = ["workspace-only"]
"##,
        )
        .unwrap();

        let mut env = RuntimeEnv::from_process();
        env.trust_workspace_config = true;
        let settings = DmuxSettings::load_with_user_config_and_env(&ws_dir, Some(&user_toml), &env);

        // `[theme]` surface colors apply through the nested table.
        assert_eq!(
            settings.theme.background,
            Some(Color::Rgb(0x10, 0x10, 0x10))
        );
        assert_eq!(settings.theme.foreground, Some(Color::DarkGray));
        // Built-ins and custom ids from either config resolve.
        assert_eq!(settings.theme.agent_color("cursor", None), Color::Yellow);
        assert_eq!(
            settings.theme.agent_color("user-only", None),
            Color::Rgb(0xff, 0x88, 0x00)
        );
        assert_eq!(
            settings.theme.agent_color("workspace-only", None),
            Color::Rgb(0x12, 0x34, 0x56)
        );
        // An id that is neither built-in nor custom is warned about and dropped.
        assert!(!settings.theme.agents.contains_key("mystery"));
        assert_eq!(
            settings.theme.agent_color("mystery", None),
            settings.theme.other
        );
        assert!(
            settings
                .config_warnings
                .iter()
                .any(|w| w == "[theme.agents] unknown agent id 'mystery' ignored"),
            "warnings: {:?}",
            settings.config_warnings
        );
        // Known custom ids from either config are not flagged.
        assert!(
            !settings
                .config_warnings
                .iter()
                .any(|w| w.contains("user-only") || w.contains("workspace-only")),
            "warnings: {:?}",
            settings.config_warnings
        );
        // The same invalid `[theme] border` in both configs warns exactly once.
        let border_warnings = settings
            .config_warnings
            .iter()
            .filter(|w| *w == "[theme] border = \"bogus\" is not a valid color; using the default")
            .count();
        assert_eq!(
            border_warnings, 1,
            "warnings: {:?}",
            settings.config_warnings
        );

        std::fs::remove_dir_all(&temp_base).ok();
    }

    // TC-350 / FR-337: explicit text output with JSONL path fields warns at load.
    #[test]
    fn tc_350_custom_agent_text_output_with_jsonl_paths_warns() {
        let dir = std::env::temp_dir().join(format!(
            "dmuxai-custom-text-jsonl-warn-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("dmuxai.toml"),
            r#"[[agents.custom]]
name = "warn-agent"
command = ["echo"]
output = "text"
text_path = "/part/text"
"#,
        )
        .unwrap();
        let mut env = RuntimeEnv::from_process();
        env.trust_workspace_config = true;
        let settings = DmuxSettings::load_with_user_config_and_env(&dir, None, &env);
        assert!(
            settings.config_warnings.iter().any(|w| {
                w.contains("[custom agent] warn-agent")
                    && w.contains("output = \"text\"")
                    && w.contains("output = \"jsonl\"")
            }),
            "warnings: {:?}",
            settings.config_warnings
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // TC-351 / FR-337: inferred jsonl (path fields only) does not warn about text/jsonl mismatch.
    #[test]
    fn tc_351_custom_agent_inferred_jsonl_no_text_output_warning() {
        let dir =
            std::env::temp_dir().join(format!("dmuxai-custom-infer-jsonl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("dmuxai.toml"),
            r#"[[agents.custom]]
name = "infer-agent"
command = ["echo"]
text_path = "/part/text"
"#,
        )
        .unwrap();
        let settings = DmuxSettings::load_with_user_config(&dir, None);
        assert!(
            !settings
                .config_warnings
                .iter()
                .any(|w| w.contains("[custom agent]")),
            "warnings: {:?}",
            settings.config_warnings
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // TC-1311 / FR-1308: legacy `[modes]` loads without error and emits one warning.
    #[test]
    fn tc_1311_legacy_modes_section_is_ignored_with_warning() {
        let dir =
            std::env::temp_dir().join(format!("dmuxai-modes-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("dmuxai.toml"),
            "[modes]\nask = \"x\"\nplan = \"y\"\nimplement = \"z\"\n",
        )
        .unwrap();
        let settings = DmuxSettings::load_with_user_config(&dir, None);
        assert!(
            settings
                .config_warnings
                .iter()
                .any(|w| w == MODES_SECTION_DEPRECATED),
            "warnings: {:?}",
            settings.config_warnings
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // TC-932 (FR-931): `[graph] enabled` is deprecated and ignored; other keys apply.
    #[test]
    fn graph_enabled_key_is_deprecated_and_ignored() {
        let dir =
            std::env::temp_dir().join(format!("dmuxai-graph-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("dmuxai.toml"),
            "[graph]\nenabled = false\nmax_parallel = 3\n",
        )
        .unwrap();

        for _ in 0..2 {
            let settings = DmuxSettings::load_with_user_config(&dir, None);
            assert_eq!(settings.graph_max_parallel, 3);
            let warnings = settings
                .config_warnings
                .iter()
                .filter(|w| {
                    *w == "[graph] enabled is deprecated: graph engineering is always on; remove the key"
                })
                .count();
            assert_eq!(warnings, 1, "warnings: {:?}", settings.config_warnings);
        }

        assert!(
            !crate::init::starter_config(false, false).contains("enabled"),
            "init template must not write [graph] enabled"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // TC-975 (FR-958): replan caps default, clamp out-of-range values and warn.
    #[test]
    fn graph_replan_limits_clamped() {
        let dir = std::env::temp_dir().join(format!("dmuxai-graph-limits-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let defaults = DmuxSettings::load_with_user_config(&dir, None);
        assert_eq!(defaults.graph_max_replans, 3);
        assert_eq!(defaults.graph_replan_budget_chars, 24_000);
        assert_eq!(defaults.graph_max_nodes, 12);
        assert!(defaults.config_warnings.is_empty());

        std::fs::write(
            dir.join("dmuxai.toml"),
            "[graph]\nmax_replans = 99\nreplan_budget_chars = 10\nmax_nodes = 0\n",
        )
        .unwrap();
        let clamped = DmuxSettings::load_with_user_config(&dir, None);
        assert_eq!(clamped.graph_max_replans, 3);
        assert_eq!(clamped.graph_replan_budget_chars, 24_000);
        assert_eq!(clamped.graph_max_nodes, 12);
        for key in ["max_replans", "replan_budget_chars", "max_nodes"] {
            assert!(
                clamped
                    .config_warnings
                    .iter()
                    .any(|w| w.contains(&format!("[graph] {key} ="))),
                "{key}: {:?}",
                clamped.config_warnings
            );
        }

        std::fs::write(
            dir.join("dmuxai.toml"),
            "[graph]\nmax_replans = 0\nreplan_budget_chars = 2000\nmax_nodes = 16\n",
        )
        .unwrap();
        let in_range = DmuxSettings::load_with_user_config(&dir, None);
        assert_eq!(in_range.graph_max_replans, 0);
        assert_eq!(in_range.graph_replan_budget_chars, 2_000);
        assert_eq!(in_range.graph_max_nodes, 16);
        assert!(in_range.config_warnings.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    // TC-998 / FR-967: `[graph] tasks_file` parses and defaults off.
    #[test]
    fn graph_tasks_file_parses() {
        let dir =
            std::env::temp_dir().join(format!("dmuxai-graph-tasks-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let defaults = DmuxSettings::load_with_user_config(&dir, None);
        assert!(!defaults.graph_tasks_file);

        std::fs::write(dir.join("dmuxai.toml"), "[graph]\ntasks_file = true\n").unwrap();
        let enabled = DmuxSettings::load_with_user_config(&dir, None);
        assert!(enabled.graph_tasks_file);
        assert!(enabled.config_warnings.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    // Run only via `ui_config_activity_rendering_parsing` (shared temp dirs).
    fn parses_ui_section() {
        let dir = std::env::temp_dir().join(format!("dmuxai-ui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("dmuxai.toml"),
            "[ui]\nshow_tools = \"expanded\"\nshow_thinking = \"hidden\"\n",
        )
        .unwrap();
        let settings = DmuxSettings::load(&dir);
        assert_eq!(settings.ui.show_tools, ShowTools::Expanded);
        assert_eq!(settings.ui.show_thinking, ShowThinking::Hidden);
        std::fs::remove_dir_all(&dir).ok();

        // Default when missing [ui]
        let dir_missing =
            std::env::temp_dir().join(format!("dmuxai-ui-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir_missing).unwrap();
        std::fs::write(dir_missing.join("dmuxai.toml"), "# no ui section\n").unwrap();
        let settings_missing = DmuxSettings::load(&dir_missing);
        assert_eq!(settings_missing.ui.show_tools, ShowTools::Collapsed);
        assert_eq!(settings_missing.ui.show_thinking, ShowThinking::Live);
        std::fs::remove_dir_all(&dir_missing).ok();

        // Fallback when invalid values
        let dir_invalid =
            std::env::temp_dir().join(format!("dmuxai-ui-invalid-{}", std::process::id()));
        std::fs::create_dir_all(&dir_invalid).unwrap();
        std::fs::write(
            dir_invalid.join("dmuxai.toml"),
            "[ui]\nshow_tools = \"unknown\"\nshow_thinking = \"invalid\"\n",
        )
        .unwrap();
        let settings_invalid = DmuxSettings::load(&dir_invalid);
        assert_eq!(settings_invalid.ui.show_tools, ShowTools::Collapsed);
        assert_eq!(settings_invalid.ui.show_thinking, ShowThinking::Live);
        std::fs::remove_dir_all(&dir_invalid).ok();
    }

    #[test]
    fn ui_config_activity_rendering_parsing() {
        parses_ui_section();
    }

    #[test]
    fn user_config_merged_and_overridden_by_workspace() {
        // TC-434, FR-433
        let temp_base =
            std::env::temp_dir().join(format!("dmuxai-user-cfg-{}", std::process::id()));
        let user_dir = temp_base.join("user");
        let ws_dir = temp_base.join("workspace");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&ws_dir).unwrap();

        let user_toml = user_dir.join("dmuxai.toml");
        std::fs::write(
            &user_toml,
            r#"
[graph]
allowed_runners = ["agy", "cursor"]
max_retries = 5

[theme]
user = "blue"
"#,
        )
        .unwrap();

        std::fs::write(
            ws_dir.join("dmuxai.toml"),
            r#"
[graph]
allowed_runners = ["agy"]
max_parallel = 4

[theme]
cursor = "green"
"#,
        )
        .unwrap();

        let settings = DmuxSettings::load_with_user_config(&ws_dir, Some(&user_toml));
        // Workspace overrides allowed_runners and sets max_parallel
        assert_eq!(settings.graph_allowed_runners, Some(vec!["agy".to_owned()]));
        assert_eq!(settings.graph_max_parallel, 4);
        // User config max_retries is preserved
        assert_eq!(settings.graph_max_retries, 5);
        // Theme merges user and cursor colors
        assert_eq!(settings.theme.user, Color::Blue);
        assert_eq!(settings.theme.cursor, Color::Green);

        // Also test missing user config falls back gracefully
        let settings_no_user = DmuxSettings::load_with_user_config(&ws_dir, None);
        assert_eq!(
            settings_no_user.graph_allowed_runners,
            Some(vec!["agy".to_owned()])
        );
        assert_eq!(settings_no_user.graph_max_parallel, 4);
        assert_eq!(settings_no_user.graph_max_retries, 2);

        std::fs::remove_dir_all(&temp_base).ok();
    }

    #[test]
    fn allowed_runners_parsing_and_unknown_runner_warning() {
        // TC-430, TC-433, FR-430
        let dir = std::env::temp_dir().join(format!("dmuxai-runners-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(
            dir.join("dmuxai.toml"),
            r#"
[graph]
allowed_runners = ["AGY", "bogus_agent", "cursor"]
"#,
        )
        .unwrap();

        let settings = DmuxSettings::load_with_user_config(&dir, None);
        assert_eq!(
            settings.graph_allowed_runners,
            Some(vec!["agy".to_owned(), "cursor".to_owned()])
        );
        assert!(
            settings.config_warnings.iter().any(
                |w| w.contains("unknown runner 'bogus_agent' in graph.allowed_runners ignored")
            )
        );

        // Empty list produces Some(vec![]) with no warnings
        std::fs::write(
            dir.join("dmuxai.toml"),
            r#"
[graph]
allowed_runners = []
"#,
        )
        .unwrap();
        let settings_empty = DmuxSettings::load_with_user_config(&dir, None);
        assert_eq!(settings_empty.graph_allowed_runners, Some(Vec::new()));

        std::fs::remove_dir_all(&dir).ok();
    }

    // FR-564: `[todo] jira_base_url` is validated, slash-trimmed, merged
    // user-then-repo, and an invalid value warns and is ignored.
    #[test]
    fn todo_jira_base_url_is_validated_and_merged() {
        let base = std::env::temp_dir().join(format!("dmuxai-todo-cfg-{}", std::process::id()));
        let user_dir = base.join("user");
        let ws_dir = base.join("workspace");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&ws_dir).unwrap();

        let user_toml = user_dir.join("dmuxai.toml");
        std::fs::write(
            &user_toml,
            "[todo]\njira_base_url = \"https://user.atlassian.net/\"\n",
        )
        .unwrap();
        std::fs::write(
            ws_dir.join("dmuxai.toml"),
            "[todo]\njira_base_url = \"https://example.atlassian.net\"\n",
        )
        .unwrap();

        let mut env = RuntimeEnv::from_process();
        env.trust_workspace_config = true;
        let settings = DmuxSettings::load_with_user_config_and_env(&ws_dir, Some(&user_toml), &env);
        assert_eq!(
            settings.todo_jira_base_url.as_deref(),
            Some("https://example.atlassian.net"),
            "repo config wins"
        );

        // Invalid repo value warns and keeps the (trimmed) user value.
        std::fs::write(
            ws_dir.join("dmuxai.toml"),
            "[todo]\njira_base_url = \"javascript:alert(1)\"\n",
        )
        .unwrap();
        let mut env = RuntimeEnv::from_process();
        env.trust_workspace_config = true;
        let settings = DmuxSettings::load_with_user_config_and_env(&ws_dir, Some(&user_toml), &env);
        assert_eq!(
            settings.todo_jira_base_url.as_deref(),
            Some("https://user.atlassian.net"),
            "invalid repo value is ignored"
        );
        assert!(
            settings
                .config_warnings
                .iter()
                .any(|warning| warning.contains("[todo] jira_base_url ignored"))
        );

        // Query strings and fragments are rejected too.
        for bad in [
            "https://example.atlassian.net/?x=1",
            "https://example.atlassian.net/#frag",
        ] {
            std::fs::write(
                ws_dir.join("dmuxai.toml"),
                format!("[todo]\njira_base_url = \"{bad}\"\n"),
            )
            .unwrap();
            let settings = DmuxSettings::load_with_user_config(&ws_dir, None);
            assert!(
                settings.todo_jira_base_url.is_none(),
                "{bad} must be rejected"
            );
        }

        std::fs::remove_dir_all(&base).ok();
    }

    // TC-2400 / FR-2400: `[jira]` defaults, trimming, clamping, and validation.
    #[test]
    fn tc_2400_jira_config_defaults_and_validation() {
        let base = tempfile::tempdir().unwrap();
        let ws = base.path().join("workspace");
        std::fs::create_dir_all(&ws).unwrap();
        let user = base.path().join("user.toml");

        let load = |contents: &str| {
            std::fs::write(&user, contents).unwrap();
            DmuxSettings::load_with_user_config(&ws, Some(&user))
        };

        let settings = load(
            "[jira]\nbase_url = \"https://example.atlassian.net/\"\nemail = \"user@example.com\"\n",
        );
        let jira = settings.jira.expect("configured");
        assert_eq!(jira.base_url, "https://example.atlassian.net");
        assert_eq!(jira.email, "user@example.com");
        assert_eq!(
            jira.jql,
            "assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC"
        );
        assert_eq!(jira.max_results, 50);
        assert_eq!(jira.token_env, "DMUXAI_JIRA_TOKEN");
        assert!(
            settings.config_warnings.is_empty(),
            "{:?}",
            settings.config_warnings
        );

        let settings = load(
            "[jira]\nbase_url = \"https://example.atlassian.net\"\nemail = \"user@example.com\"\njql = \"project = DEMO\"\nmax_results = 500\ntoken_env = \"DEMO_JIRA_TOKEN\"\n",
        );
        let jira = settings.jira.expect("configured");
        assert_eq!(jira.jql, "project = DEMO");
        assert_eq!(jira.max_results, 100, "upper clamp");
        assert_eq!(jira.token_env, "DEMO_JIRA_TOKEN");
        assert!(
            settings.config_warnings.is_empty(),
            "{:?}",
            settings.config_warnings
        );

        let settings = load(
            "[jira]\nbase_url = \"https://example.atlassian.net\"\nemail = \"user@example.com\"\nmax_results = 0\n",
        );
        assert_eq!(settings.jira.expect("configured").max_results, 1);

        // Plain http is accepted only for loopback hosts.
        let settings =
            load("[jira]\nbase_url = \"http://127.0.0.1:8080/\"\nemail = \"user@example.com\"\n");
        assert_eq!(
            settings.jira.expect("loopback http").base_url,
            "http://127.0.0.1:8080"
        );
        for bad in [
            "http://example.atlassian.net",
            "javascript:alert(1)",
            "https://example.atlassian.net/?x=1",
        ] {
            let settings = load(&format!(
                "[jira]\nbase_url = \"{bad}\"\nemail = \"user@example.com\"\n"
            ));
            assert!(settings.jira.is_none(), "{bad} must be rejected");
            assert!(
                settings
                    .config_warnings
                    .iter()
                    .any(|w| w.contains("[jira] base_url ignored")),
                "warnings: {:?}",
                settings.config_warnings
            );
        }

        let settings = load(
            "[jira]\nbase_url = \"https://example.atlassian.net\"\nemail = \"user@example.com\"\ntoken_env = \"bad-name\"\n",
        );
        assert_eq!(
            settings.jira.expect("configured").token_env,
            "DMUXAI_JIRA_TOKEN"
        );
        assert!(
            settings
                .config_warnings
                .iter()
                .any(|w| w.contains("[jira] token_env ignored")),
            "warnings: {:?}",
            settings.config_warnings
        );

        // Missing table and incomplete configuration are silently not configured.
        let settings = load("[web]\ntheme = \"dark\"\n");
        assert!(settings.jira.is_none());
        let settings = load("[jira]\nbase_url = \"https://example.atlassian.net\"\n");
        assert!(settings.jira.is_none());
    }

    // TC-2401 / FR-2401: untrusted repo `[jira]` is stripped; trusted keeps it.
    #[test]
    fn tc_2401_untrusted_repo_jira_table_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let user = dir.path().join("user.toml");
        std::fs::write(
            &user,
            "[jira]\nbase_url = \"https://user.atlassian.net\"\nemail = \"user@example.com\"\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("dmuxai.toml"),
            "[jira]\nbase_url = \"https://repo.atlassian.net\"\nemail = \"repo@example.com\"\ntoken_env = \"REPO_JIRA_TOKEN\"\n",
        )
        .unwrap();

        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir.clone()),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };
        let settings = DmuxSettings::load_with_user_config_and_env(&ws, Some(&user), &env);
        let jira = settings.jira.expect("user config applies");
        assert_eq!(jira.base_url, "https://user.atlassian.net");
        assert_eq!(jira.email, "user@example.com");
        assert_eq!(jira.token_env, "DMUXAI_JIRA_TOKEN");
        match &settings.workspace_trust {
            WorkspaceTrustStatus::Untrusted { ignored, .. } => {
                assert!(ignored.contains(&"jira".to_owned()), "{ignored:?}");
            }
            other => panic!("expected untrusted, got {other:?}"),
        }

        // Trusted workspace keeps the repo table.
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: true,
            ..RuntimeEnv::default()
        };
        let settings = DmuxSettings::load_with_user_config_and_env(&ws, Some(&user), &env);
        let jira = settings.jira.expect("repo config applies");
        assert_eq!(jira.base_url, "https://repo.atlassian.net");
        assert_eq!(jira.email, "repo@example.com");
        assert_eq!(jira.token_env, "REPO_JIRA_TOKEN");
    }
}

#[cfg(test)]
mod memory_settings_tests {
    use super::*;

    fn load(user: Option<&str>, workspace: Option<&str>, ws: &Path) -> DmuxSettings {
        let user_path = ws.join("user.toml");
        if let Some(u) = user {
            std::fs::write(&user_path, u).unwrap();
        }
        if let Some(w) = workspace {
            std::fs::write(ws.join("dmuxai.toml"), w).unwrap();
        }
        DmuxSettings::load_with_user_config(ws, Some(&user_path))
    }

    // TC-720 (FR-720): defaults, per-key workspace override, project, inject.
    #[test]
    fn memory_settings_resolve_root_dir_project_and_inject() {
        let ws = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        let v = vault.path().display().to_string().replace('\\', "/");
        let s = load(
            Some(&format!("[memory]\nvault = \"{v}\"\n")),
            None,
            ws.path(),
        );
        let m = s.memory.expect("enabled");
        assert_eq!(m.dir, "memory");
        assert_eq!(m.root, vault.path().join("memory"));
        assert_eq!(m.project, None);
        assert!(m.inject);

        let s = load(
            Some(&format!("[memory]\nvault = \"{v}\"\ndir = \"notes\"\n")),
            Some("[memory]\nproject = \"my-repo\"\ninject = false\n"),
            ws.path(),
        );
        let m = s.memory.expect("enabled");
        assert_eq!(m.root, vault.path().join("notes"));
        assert_eq!(m.project.as_deref(), Some("my-repo"));
        assert!(!m.inject);
        assert!(s.config_warnings.is_empty(), "{:?}", s.config_warnings);
    }

    // TC-720 (FR-720): no [memory] or empty vault -> disabled without warnings.
    #[test]
    fn memory_settings_absent_is_disabled_silently() {
        let ws = tempfile::tempdir().unwrap();
        let s = load(Some("[web]\ntheme = \"dark\"\n"), None, ws.path());
        assert!(s.memory.is_none());
        let s = load(Some("[memory]\ndir = \"x\"\n"), None, ws.path());
        assert!(s.memory.is_none());
        assert!(s.config_warnings.iter().all(|w| !w.contains("[memory]")));
    }

    // TC-720 (FR-720): missing vault, absolute or `..` dir, invalid project.
    #[test]
    fn memory_settings_warn_on_invalid_values() {
        let ws = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        let v = vault.path().display().to_string().replace('\\', "/");
        let missing = vault
            .path()
            .join("nope")
            .display()
            .to_string()
            .replace('\\', "/");
        let s = load(
            Some(&format!("[memory]\nvault = \"{missing}\"\n")),
            None,
            ws.path(),
        );
        assert!(s.memory.is_none());
        assert!(
            s.config_warnings
                .iter()
                .any(|w| w.contains("not an existing directory"))
        );

        for dir in ["../escape", "/abs/dir", "a/../../b"] {
            let s = load(
                Some(&format!("[memory]\nvault = \"{v}\"\ndir = \"{dir}\"\n")),
                None,
                ws.path(),
            );
            assert!(s.memory.is_none(), "{dir}");
            assert!(
                s.config_warnings.iter().any(|w| w.contains("[memory] dir")),
                "{dir}"
            );
        }

        let s = load(
            Some(&format!(
                "[memory]\nvault = \"{v}\"\nproject = \"Bad Name\"\n"
            )),
            None,
            ws.path(),
        );
        assert_eq!(s.memory.as_ref().and_then(|m| m.project.clone()), None);
        assert!(s.memory.is_some());
        assert!(
            s.config_warnings
                .iter()
                .any(|w| w.contains("[memory] project"))
        );
    }

    // TC-720 (FR-720): a relative vault resolves against the workspace.
    #[test]
    fn memory_settings_relative_vault_is_workspace_relative() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir(ws.path().join("vault")).unwrap();
        let s = load(None, Some("[memory]\nvault = \"vault\"\n"), ws.path());
        let m = s.memory.expect("enabled");
        assert_eq!(m.root, ws.path().join("vault").join("memory"));
    }

    // TC-720 (FR-720): `~/` expands to HOME.
    #[test]
    fn memory_settings_expand_home_prefix() {
        let p = expand_home("~/Obsidian/Vault");
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            assert_eq!(p, PathBuf::from(home).join("Obsidian/Vault"));
        }
        assert_eq!(expand_home("/x/y"), PathBuf::from("/x/y"));
    }
}

#[cfg(test)]
mod notify_config_tests {
    use super::*;

    #[test]
    fn notify_table_parses_and_warns() {
        // TC-772 config surface / FR-770..FR-772: `[notify]` parses into
        // `DmuxSettings.notify` and `NotifySettings::validate` warnings join
        // `config_warnings` prefixed with "notify: ".
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("dmuxai.toml"),
            r#"
[notify]
webhook_url = "ftp://example.invalid/hook"
command = []
events = ["completed", "bogus_state"]
timeout_secs = 0
"#,
        )
        .unwrap();

        let mut env = RuntimeEnv::from_process();
        env.trust_workspace_config = true;
        let settings = DmuxSettings::load_with_user_config_and_env(dir.path(), None, &env);
        assert_eq!(
            settings.notify.webhook_url.as_deref(),
            Some("ftp://example.invalid/hook")
        );
        assert_eq!(settings.notify.timeout_secs, Some(0));
        assert_eq!(
            settings.notify.events.as_deref(),
            Some(["completed".to_owned(), "bogus_state".to_owned()].as_slice())
        );

        let warnings = settings
            .config_warnings
            .iter()
            .filter(|w| w.starts_with("notify: "))
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("webhook_url must start with http:// or https://")),
            "warnings: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("command must not be empty")),
            "warnings: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("bogus_state") && w.contains("events")),
            "warnings: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("timeout_secs must be greater than 0")),
            "warnings: {warnings:?}"
        );

        // Defaults when `[notify]` is absent: no notify warnings.
        let empty = tempfile::tempdir().unwrap();
        let defaults = DmuxSettings::load_with_user_config(empty.path(), None);
        assert_eq!(defaults.notify, NotifySettings::default());
        assert!(
            !defaults
                .config_warnings
                .iter()
                .any(|w| w.starts_with("notify: "))
        );
    }
}

#[cfg(test)]
mod builtin_memory_mcp_tests {
    use super::*;

    /// Absolute on every platform, so the built-in entry is always resolvable.
    fn exe() -> PathBuf {
        std::env::temp_dir().join("dmuxai-bin").join("dmuxai")
    }

    fn write_memory_ws(ws: &Path, vault: &Path, extra: &str) {
        let v = vault.display().to_string().replace('\\', "/");
        std::fs::write(
            ws.join("dmuxai.toml"),
            format!("[memory]\nvault = \"{v}\"\n{extra}"),
        )
        .unwrap();
    }

    // TC-2081 / FR-2081: legacy [[mcp.servers]] runners key loads with exactly one warning.
    #[test]
    fn tc_2081_legacy_mcp_runners_key_loads_with_one_warning() {
        let ws = tempfile::tempdir().unwrap();
        let user = ws.path().join("user.toml");
        std::fs::write(
            &user,
            r#"
[[mcp.servers]]
name = "server-user"
command = "user-cmd"
runners = ["codex", "cursor"]
"#,
        )
        .unwrap();
        std::fs::write(
            ws.path().join("dmuxai.toml"),
            r#"
[[mcp.servers]]
name = "server-ws"
command = "ws-cmd"
runners = ["custom-agent"]
"#,
        )
        .unwrap();

        let mut env = RuntimeEnv::from_process();
        env.trust_workspace_config = true;
        let settings = DmuxSettings::load_with_user_config_and_env(ws.path(), Some(&user), &env);
        let names: Vec<&str> = settings
            .mcp_servers
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert!(names.contains(&"server-user"));
        assert!(names.contains(&"server-ws"));

        let runner_warnings: Vec<&String> = settings
            .config_warnings
            .iter()
            .filter(|w| w.contains("runners is deprecated"))
            .collect();
        assert_eq!(
            runner_warnings.len(),
            1,
            "warnings: {:?}",
            settings.config_warnings
        );
        assert!(runner_warnings[0].contains("runners is deprecated and ignored"));
    }

    // TC-2083 / FR-2083: [memory] vault set -> mcp_servers has no dmuxai-memory entry.
    #[test]
    fn tc_2083_memory_on_adds_no_builtin_mcp_entry() {
        let ws = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        write_memory_ws(ws.path(), vault.path(), "");

        let exe = exe();
        let mut env = RuntimeEnv::from_process();
        env.trust_workspace_config = true;
        let s = DmuxSettings::load_with_runners_and_env(
            ws.path(),
            None,
            Some(&exe),
            find_opencode_bin(),
            find_grok_bin(),
            &env,
        );
        assert!(s.memory.is_some());
        assert!(
            s.mcp_servers.iter().all(|s| s.name != "dmuxai-memory"),
            "mcp_servers must not contain dmuxai-memory: {:?}",
            s.mcp_servers
        );
        assert!(s.config_warnings.is_empty(), "{:?}", s.config_warnings);
    }

    #[test]
    fn cursor_prewarm_settings_parse_and_override() {
        let default_settings = DmuxSettings::default();
        assert!(default_settings.cursor_prewarm);
        assert_eq!(default_settings.cursor_prewarm_idle_secs, 300);

        let ws = tempfile::tempdir().unwrap();
        std::fs::write(
            ws.path().join("dmuxai.toml"),
            "[cursor]\nprewarm = false\nprewarm_idle_secs = 120\n",
        )
        .unwrap();
        let settings = DmuxSettings::load(ws.path());
        assert!(!settings.cursor_prewarm);
        assert_eq!(settings.cursor_prewarm_idle_secs, 120);
    }

    // TC-1305, TC-1410 / FR-1300, FR-1301, FR-1410: runner protocol defaults and TOML parsing.
    #[test]
    fn tc_1305_runner_protocol_defaults_and_parse() {
        let defaults = DmuxSettings::default();
        assert_eq!(defaults.cursor_protocol, RunnerProtocol::Legacy);
        assert_eq!(defaults.grok_protocol, RunnerProtocol::Acp);

        let ws = tempfile::tempdir().unwrap();
        std::fs::write(
            ws.path().join("dmuxai.toml"),
            "[cursor]\nprotocol = \"legacy\"\n[agents]\ngrok_protocol = \"legacy\"\nopencode_protocol = \"legacy\"\n",
        )
        .unwrap();
        let settings = DmuxSettings::load(ws.path());
        assert_eq!(settings.cursor_protocol, RunnerProtocol::Legacy);
        assert_eq!(settings.grok_protocol, RunnerProtocol::Legacy);
        // TC-1423 / FR-1418: opencode has no ACP switch.
        assert!(
            settings
                .config_warnings
                .iter()
                .any(|w| w.contains("opencode_protocol ignored"))
        );
    }

    // opencode binary resolution includes ~/.opencode/bin/opencode.
    #[test]
    fn find_opencode_bin_home_install() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".opencode").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("opencode");
        std::fs::write(&exe, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let env = crate::runtime_env::RuntimeEnv {
            home: Some(home.path().to_path_buf()),
            ..crate::runtime_env::RuntimeEnv::default()
        };
        assert_eq!(find_opencode_bin_from(&env), Some(exe));
    }

    // FR-1326: explicit DMUXAI_OPENCODE_BIN missing file → clear message.
    #[test]
    fn opencode_explicit_bin_missing_message() {
        let msg = opencode_bin_explicit_missing_message_from(Some(std::ffi::OsString::from(
            "/nonexistent/opencode",
        )))
        .expect("message");
        assert!(msg.contains("opencode binary not found"));
        assert!(msg.contains("/nonexistent/opencode"));
        assert!(msg.contains("DMUXAI_OPENCODE_BIN"));
    }
}

#[cfg(test)]
mod builtin_opencode_tests {
    use super::*;
    use crate::custom_agent::opencode_preset;

    /// Fake opencode executable: a plain file is enough for the PATH check.
    fn fake_opencode(dir: &Path) -> PathBuf {
        let path = dir.join("opencode");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        path
    }

    // TC-810 / FR-810, FR-811: the preset is registered only when opencode is
    // installed, no config names its own `opencode`, and `builtin_opencode` is
    // not false. `allowed_runners = ["opencode"]` validates with the preset.
    #[test]
    fn builtin_opencode_preset_registration() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_opencode(dir.path());
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let user = dir.path().join("user.toml");
        let env = RuntimeEnv::default();
        let load = |bin: Option<PathBuf>| {
            DmuxSettings::load_with_opencode_and_env(&ws, Some(&user), None, bin, &env)
        };

        // Installed, no config: the preset is registered from the template.
        let settings = load(Some(bin.clone()));
        assert_eq!(
            settings.custom_agents.len(),
            1,
            "{:?}",
            settings.custom_agents
        );
        let preset = &settings.custom_agents[0];
        assert!(preset.builtin);
        assert_eq!(preset.name, "opencode");
        assert_eq!(preset.command[0], bin.to_string_lossy());
        assert_eq!(preset.default_model, None);
        assert_eq!(preset.allowed_models.as_deref(), Some("^opencode/.*-free$"));
        assert_eq!(preset.models_filter.as_deref(), Some("-free$"));
        assert_eq!(
            preset.models_command,
            vec![bin.to_string_lossy().into_owned(), "models".to_owned()]
        );
        assert_eq!(
            preset.command,
            opencode_preset(&bin.to_string_lossy()).command
        );
        assert!(
            settings.config_warnings.is_empty(),
            "{:?}",
            settings.config_warnings
        );

        // Not installed: no preset and no error.
        let missing = load(None);
        assert!(missing.custom_agents.is_empty());
        assert!(missing.config_warnings.is_empty());

        // A user entry named opencode wins; the preset is absent.
        std::fs::write(
            &user,
            "[[agents.custom]]\nname = \"opencode\"\ncommand = [\"my-opencode\"]\n",
        )
        .unwrap();
        let user_wins = load(Some(bin.clone()));
        assert_eq!(user_wins.custom_agents.len(), 1);
        assert!(!user_wins.custom_agents[0].builtin);
        assert_eq!(user_wins.custom_agents[0].command, vec!["my-opencode"]);

        // `builtin_opencode = false` in the workspace hides the preset.
        std::fs::write(&user, "").unwrap();
        std::fs::write(
            ws.join("dmuxai.toml"),
            "[agents]\nbuiltin_opencode = false\n",
        )
        .unwrap();
        let disabled = load(Some(bin.clone()));
        assert!(!disabled.builtin_opencode);
        assert!(disabled.custom_agents.is_empty());

        // `[agents]` without custom agents keeps the user's custom agents.
        std::fs::write(
            &user,
            "[[agents.custom]]\nname = \"pi\"\ncommand = [\"pi\"]\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("dmuxai.toml"),
            "[agents]\nbuiltin_opencode = true\n",
        )
        .unwrap();
        let kept = load(Some(bin.clone()));
        let names: Vec<&str> = kept.custom_agents.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["pi", "opencode"]);

        // The preset can be named in allowed_runners without a warning.
        std::fs::write(&user, "").unwrap();
        std::fs::write(
            ws.join("dmuxai.toml"),
            "[graph]\nallowed_runners = [\"opencode\", \"cursor\"]\n",
        )
        .unwrap();
        let listed = load(Some(bin.clone()));
        assert_eq!(
            listed.graph_allowed_runners,
            Some(vec!["opencode".to_owned(), "cursor".to_owned()])
        );
        assert!(
            listed.config_warnings.is_empty(),
            "{:?}",
            listed.config_warnings
        );

        // Without the preset, the same list is reported as unknown.
        let unknown = load(None);
        assert!(
            unknown
                .config_warnings
                .iter()
                .any(|w| w.contains("unknown runner 'opencode'")),
            "{:?}",
            unknown.config_warnings
        );
    }

    // FR-813: with allowed_runners unset the preset takes no graph nodes; a
    // listed preset and user-configured custom agents keep the old rule.
    #[test]
    fn builtin_preset_is_not_graph_routed_unless_listed() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_opencode(dir.path());
        let user = dir.path().join("user.toml");
        std::fs::write(
            &user,
            "[[agents.custom]]\nname = \"pi\"\ncommand = [\"pi\"]\n",
        )
        .unwrap();
        let env = RuntimeEnv::default();
        let settings = DmuxSettings::load_with_opencode_and_env(
            dir.path(),
            Some(&user),
            None,
            Some(bin),
            &env,
        );
        assert!(!settings.graph_runner_allowed("opencode"));
        assert!(settings.graph_runner_allowed("pi"));
        assert!(settings.graph_runner_allowed("cursor"));

        let mut listed = settings.clone();
        listed.graph_allowed_runners = Some(vec!["opencode".to_owned()]);
        assert!(listed.graph_runner_allowed("opencode"));
        assert!(!listed.graph_runner_allowed("cursor"));
    }

    // TC-814 / FR-814: the starter config is all comments, so it parses to the
    // defaults, with or without the opencode example, and names every section.
    #[test]
    fn starter_config_parses() {
        let _guard = test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dmuxai.toml");
        let env = RuntimeEnv::default();
        for opencode in [false, true] {
            for grok in [false, true] {
                let text = crate::init::starter_config(opencode, grok);
                for key in [
                    "[theme]",
                    "[routing]",
                    "[graph]",
                    "[cursor]",
                    "[agents]",
                    "builtin_opencode",
                    "builtin_grok",
                ] {
                    assert!(text.contains(key), "missing {key}");
                }
                std::fs::write(&path, &text).unwrap();
                // No opencode executable here: the example stays a comment.
                let settings = DmuxSettings::load_with_opencode_and_env(
                    dir.path(),
                    Some(&path),
                    None,
                    None,
                    &env,
                );
                let expected = DmuxSettings {
                    workspace_trust: WorkspaceTrustStatus::NotNeeded,
                    ..DmuxSettings::default()
                };
                assert_eq!(settings, expected, "opencode={opencode} grok={grok}");
            }
        }
    }

    #[test]
    fn theme_reduced_motion_config_and_env_var_are_honored() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("dmuxai.toml");

        // 1. dmuxai.toml with [theme] reduced_motion = true
        std::fs::write(&config_path, "[theme]\nreduced_motion = true\n").unwrap();
        let settings = DmuxSettings::load_with_opencode(dir.path(), None, None, None);
        assert!(settings.reduced_motion);
        assert!(settings.theme.reduced_motion);

        // 2. dmuxai.toml with [theme] reduced_motion = false
        std::fs::write(&config_path, "[theme]\nreduced_motion = false\n").unwrap();
        let settings = {
            let _guard = test_env_lock();
            DmuxSettings::load_with_opencode(dir.path(), None, None, None)
        };
        assert!(!settings.reduced_motion);
        assert!(!settings.theme.reduced_motion);

        // 3. Environment variable DMUXAI_REDUCED_MOTION=1 overrides config.
        // Hold the env lock so no other test sees the variable in flux.
        let settings_env = {
            let _guard = test_env_lock();
            // SAFETY: test_env_lock() serializes every test that
            // touches DMUXAI_REDUCED_MOTION.
            unsafe { std::env::set_var("DMUXAI_REDUCED_MOTION", "1") };
            let loaded = DmuxSettings::load_with_opencode(dir.path(), None, None, None);
            // SAFETY: same exclusion as above.
            unsafe { std::env::remove_var("DMUXAI_REDUCED_MOTION") };
            loaded
        };
        assert!(settings_env.reduced_motion);
        assert!(settings_env.theme.reduced_motion);
    }
}

#[cfg(test)]
mod builtin_grok_tests {
    use super::*;
    use crate::custom_agent::grok_preset;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn bin_name() -> &'static str {
        "grok"
    }

    // TC-890 / FR-890, FR-891
    #[test]
    fn builtin_grok_preset_registration() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let user = dir.path().join("user.toml");
        std::fs::write(&user, "").unwrap();

        let path_dir = dir.path().join("pathbin");
        let path_bin = path_dir.join(bin_name());
        touch(&path_bin);
        let home = dir.path().join("home");
        let home_bin = home.join(".grok").join("bin").join(bin_name());
        touch(&home_bin);
        let explicit = dir.path().join("explicit-grok");
        touch(&explicit);

        let path_var = Some(std::ffi::OsString::from(path_dir.as_os_str()));
        let home_only = Some(home.clone());

        // PATH beats the home install.
        let env_path = crate::runtime_env::RuntimeEnv {
            path: path_var.clone(),
            home: home_only.clone(),
            ..crate::runtime_env::RuntimeEnv::default()
        };
        let from_path = find_grok_bin_from(&env_path).unwrap();
        assert_eq!(from_path, path_bin);

        // Home is used when PATH misses.
        let env_home = crate::runtime_env::RuntimeEnv {
            path: None,
            home: home_only.clone(),
            ..crate::runtime_env::RuntimeEnv::default()
        };
        let from_home = find_grok_bin_from(&env_home).unwrap();
        assert_eq!(from_home, home_bin);

        // An explicit path wins over PATH and home.
        let env_explicit = crate::runtime_env::RuntimeEnv {
            grok_bin: Some(explicit.as_os_str().to_owned()),
            path: path_var.clone(),
            home: home_only.clone(),
            ..crate::runtime_env::RuntimeEnv::default()
        };
        let from_explicit = find_grok_bin_from(&env_explicit).unwrap();
        assert_eq!(from_explicit, explicit);

        // A missing DMUXAI_GROK_BIN does not fall through.
        let missing = dir.path().join("missing-grok");
        let env_missing = crate::runtime_env::RuntimeEnv {
            grok_bin: Some(missing.as_os_str().to_owned()),
            path: path_var.clone(),
            home: home_only.clone(),
            ..crate::runtime_env::RuntimeEnv::default()
        };
        assert!(find_grok_bin_from(&env_missing).is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let quiet_home = dir.path().join("quiet-home");
            let quiet = quiet_home.join(".grok").join("bin").join("grok");
            touch(&quiet);
            std::fs::set_permissions(&quiet, std::fs::Permissions::from_mode(0o644)).unwrap();
            let env_quiet = crate::runtime_env::RuntimeEnv {
                home: Some(quiet_home),
                ..crate::runtime_env::RuntimeEnv::default()
            };
            assert!(find_grok_bin_from(&env_quiet).is_none());
        }

        let env = RuntimeEnv::default();
        let load = |bin: Option<PathBuf>| {
            DmuxSettings::load_with_runners_and_env(&ws, Some(&user), None, None, bin, &env)
        };

        let settings = load(Some(path_bin.clone()));
        assert!(settings.builtin_grok);
        assert_eq!(
            settings.custom_agents.len(),
            1,
            "{:?}",
            settings.custom_agents
        );
        let preset = &settings.custom_agents[0];
        assert!(preset.builtin);
        assert_eq!(preset.name, "grok");
        assert_eq!(preset.command[0], path_bin.to_string_lossy());
        assert_eq!(
            preset.command,
            grok_preset(&path_bin.to_string_lossy()).command
        );
        assert_eq!(preset.color.as_deref(), Some("#6366f1"));
        assert!(
            settings.config_warnings.is_empty(),
            "{:?}",
            settings.config_warnings
        );

        // Missing binary: no preset and no error.
        let missing_settings = load(None);
        assert!(missing_settings.custom_agents.is_empty());
        assert!(missing_settings.config_warnings.is_empty());

        // A custom entry named grok replaces the preset.
        std::fs::write(
            &user,
            "[[agents.custom]]\nname = \"grok\"\ncommand = [\"my-grok\"]\n",
        )
        .unwrap();
        let replaced = load(Some(path_bin.clone()));
        assert_eq!(replaced.custom_agents.len(), 1);
        assert!(!replaced.custom_agents[0].builtin);
        assert_eq!(replaced.custom_agents[0].command, vec!["my-grok"]);

        // An invalid custom grok still blocks the preset.
        std::fs::write(&user, "[[agents.custom]]\nname = \"grok\"\n").unwrap();
        let invalid = load(Some(path_bin.clone()));
        assert!(invalid.custom_agents.iter().any(|spec| spec.name == "grok"));
        assert!(invalid.custom_agents.iter().all(|spec| !spec.builtin));

        // builtin_grok = false registers nothing and shows no error.
        std::fs::write(&user, "").unwrap();
        std::fs::write(ws.join("dmuxai.toml"), "[agents]\nbuiltin_grok = false\n").unwrap();
        let disabled = load(Some(path_bin.clone()));
        assert!(!disabled.builtin_grok);
        assert!(disabled.custom_agents.is_empty());
        assert!(
            disabled.config_warnings.is_empty(),
            "{:?}",
            disabled.config_warnings
        );

        // A flag-only workspace file keeps the user's custom agents.
        // pi stays before opencode and grok.
        let opencode_bin = dir.path().join("opencode-bin");
        touch(&opencode_bin);
        std::fs::write(
            &user,
            "[[agents.custom]]\nname = \"pi\"\ncommand = [\"pi\"]\n",
        )
        .unwrap();
        std::fs::write(ws.join("dmuxai.toml"), "[agents]\nbuiltin_grok = true\n").unwrap();
        let kept = DmuxSettings::load_with_runners_and_env(
            &ws,
            Some(&user),
            None,
            Some(opencode_bin.clone()),
            Some(path_bin.clone()),
            &env,
        );
        let names: Vec<&str> = kept
            .custom_agents
            .iter()
            .map(|spec| spec.name.as_str())
            .collect();
        assert_eq!(names, ["pi", "opencode", "grok"]);
        assert!(kept.custom_agents[2].builtin);

        // Workspace flag wins over the user flag.
        std::fs::write(&user, "[agents]\nbuiltin_grok = false\n").unwrap();
        std::fs::write(ws.join("dmuxai.toml"), "[agents]\nbuiltin_grok = true\n").unwrap();
        let workspace_on = load(Some(path_bin.clone()));
        assert!(workspace_on.builtin_grok);
        assert!(
            workspace_on
                .custom_agents
                .iter()
                .any(|spec| spec.builtin && spec.name == "grok")
        );

        std::fs::write(&user, "[agents]\nbuiltin_grok = true\n").unwrap();
        std::fs::write(ws.join("dmuxai.toml"), "[agents]\nbuiltin_grok = false\n").unwrap();
        let workspace_off = load(Some(path_bin.clone()));
        assert!(!workspace_off.builtin_grok);
        assert!(workspace_off.custom_agents.is_empty());

        // allowed_runners = ["grok"] is accepted when the preset will be registered.
        std::fs::write(&user, "").unwrap();
        std::fs::write(
            ws.join("dmuxai.toml"),
            "[graph]\nallowed_runners = [\"grok\", \"cursor\"]\n[theme.agents]\ngrok = \"#112233\"\n",
        )
        .unwrap();
        let listed = load(Some(path_bin.clone()));
        assert_eq!(
            listed.graph_allowed_runners,
            Some(vec!["grok".to_owned(), "cursor".to_owned()])
        );
        assert!(
            listed.config_warnings.is_empty(),
            "{:?}",
            listed.config_warnings
        );
        assert!(listed.theme.agents.contains_key("grok"));

        let unknown = load(None);
        assert!(
            unknown
                .config_warnings
                .iter()
                .any(|warning| warning.contains("unknown runner 'grok'")),
            "{:?}",
            unknown.config_warnings
        );
    }

    // TC-1673: presets use bin overrides from RuntimeEnv, and an explicit
    // RuntimeEnv without overrides ignores process bin environment variables.
    #[test]
    fn tc_1673_presets_ignore_process_bin_env() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();

        let fake_opencode = dir.path().join("fake-opencode");
        let fake_grok = dir.path().join("fake-grok");
        std::fs::write(&fake_opencode, "#!/bin/sh\n").unwrap();
        std::fs::write(&fake_grok, "#!/bin/sh\n").unwrap();

        // 1. RuntimeEnv with opencode/grok bin overrides -> presets use those paths.
        let env_with_overrides = RuntimeEnv {
            opencode_bin: Some(fake_opencode.as_os_str().to_owned()),
            grok_bin: Some(fake_grok.as_os_str().to_owned()),
            ..RuntimeEnv::default()
        };
        let settings = DmuxSettings::load_with_env(&ws, &env_with_overrides);
        let opencode_spec = settings
            .custom_agents
            .iter()
            .find(|s| s.builtin && s.name == "opencode")
            .expect("builtin opencode preset registered");
        assert_eq!(opencode_spec.command[0], fake_opencode.to_string_lossy());

        let grok_spec = settings
            .custom_agents
            .iter()
            .find(|s| s.builtin && s.name == "grok")
            .expect("builtin grok preset registered");
        assert_eq!(grok_spec.command[0], fake_grok.to_string_lossy());

        // 2. An explicit RuntimeEnv without overrides ignores real process env variables.
        let _lock = crate::state_path::test_env_lock();
        unsafe {
            std::env::set_var("DMUXAI_OPENCODE_BIN", "/bogus/nonexistent/opencode");
            std::env::set_var("DMUXAI_GROK_BIN", "/bogus/nonexistent/grok");
        }
        let env_clean = RuntimeEnv::default();
        let settings_clean = DmuxSettings::load_with_env(&ws, &env_clean);
        unsafe {
            std::env::remove_var("DMUXAI_OPENCODE_BIN");
            std::env::remove_var("DMUXAI_GROK_BIN");
        }
        assert!(
            settings_clean
                .custom_agents
                .iter()
                .all(|s| s.name != "opencode"),
            "clean RuntimeEnv must ignore DMUXAI_OPENCODE_BIN from process env"
        );
        assert!(
            settings_clean
                .custom_agents
                .iter()
                .all(|s| s.name != "grok"),
            "clean RuntimeEnv must ignore DMUXAI_GROK_BIN from process env"
        );
    }

    // TC-1674: a single shared test env lock is used across crates and modules.
    #[test]
    fn tc_1674_single_shared_test_env_lock() {
        let guard = crate::state_path::test_env_lock();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let locked = crate::settings::test_env_lock();
            tx.send(()).unwrap();
            drop(locked);
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(50))
                .is_err()
        );
        drop(guard);
        assert!(rx.recv_timeout(std::time::Duration::from_secs(1)).is_ok());
    }
}

#[cfg(test)]
mod agent_prompt_tests {
    use super::*;
    use crate::agent_prompt::{PromptInject, PromptRole, resolve, resolve_for};

    // TC-1707 (FR-1700): inline text
    #[test]
    fn agent_prompt_inline_text() {
        let ws = tempfile::tempdir().unwrap();
        let config = r#"
[runners.default]
system_prompt = "You are a helpful coding assistant."
"#;
        std::fs::write(ws.path().join("dmuxai.toml"), config).unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);
        let resolved = resolve(&settings, "codex", PromptRole::Chat).expect("prompt resolved");
        assert_eq!(resolved.text, "You are a helpful coding assistant.");
        assert_eq!(resolved.source, "workspace:[runners.default]");
        assert_eq!(resolved.inject, PromptInject::Session);
    }

    // TC-1707 (FR-1700): file text
    #[test]
    fn agent_prompt_file_text() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("prompt.txt"), "Instructions from file.").unwrap();
        let config = r#"
[runners.codex]
system_prompt_file = "prompt.txt"
"#;
        std::fs::write(ws.path().join("dmuxai.toml"), config).unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);
        let resolved = resolve(&settings, "codex", PromptRole::Chat).expect("prompt resolved");
        assert_eq!(resolved.text, "Instructions from file.");
        assert_eq!(resolved.source, "workspace:[runners.codex] (prompt.txt)");
    }

    // TC-1707 (FR-1700): relative path
    #[test]
    fn agent_prompt_relative_path_resolved_against_config_file_dir() {
        let user_dir = tempfile::tempdir().unwrap();
        let prompts_dir = user_dir.path().join("prompts");
        std::fs::create_dir_all(&prompts_dir).unwrap();
        std::fs::write(
            prompts_dir.join("user_prompt.txt"),
            "User prompt from relative file.",
        )
        .unwrap();

        let user_toml = user_dir.path().join("user.toml");
        std::fs::write(
            &user_toml,
            r#"
[runners.default]
system_prompt_file = "prompts/user_prompt.txt"
"#,
        )
        .unwrap();

        let ws = tempfile::tempdir().unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), Some(&user_toml));
        let resolved =
            resolve(&settings, "cursor", PromptRole::Chat).expect("prompt resolved from user file");
        assert_eq!(resolved.text, "User prompt from relative file.");
        assert_eq!(
            resolved.source,
            "user:[runners.default] (prompts/user_prompt.txt)"
        );
    }

    // TC-1707 (FR-1700): absolute path
    #[test]
    fn agent_prompt_absolute_path_allowed() {
        let ws = tempfile::tempdir().unwrap();
        let prompt_dir = tempfile::tempdir().unwrap();
        let abs_file = prompt_dir.path().join("abs_prompt.txt");
        std::fs::write(&abs_file, "Absolute prompt text.").unwrap();

        let config = format!(
            r#"
[runners.default]
system_prompt_file = "{}"
"#,
            abs_file.to_string_lossy()
        );
        std::fs::write(ws.path().join("dmuxai.toml"), config).unwrap();
        let mut env = RuntimeEnv::from_process();
        env.trust_workspace_config = true;
        let settings = DmuxSettings::load_with_user_config_and_env(ws.path(), None, &env);
        let resolved = resolve(&settings, "codex", PromptRole::Chat).expect("resolved");
        assert_eq!(resolved.text, "Absolute prompt text.");
        assert!(resolved.source.starts_with("workspace:[runners.default] ("));
    }

    // TC-1708 (FR-1700): both set warns
    #[test]
    fn agent_prompt_both_set_warns_and_ignored() {
        let ws = tempfile::tempdir().unwrap();
        let config = r#"
[runners.default]
system_prompt = "Inline prompt"
system_prompt_file = "some_file.txt"
"#;
        std::fs::write(ws.path().join("dmuxai.toml"), config).unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);

        assert!(
            settings
                .config_warnings
                .iter()
                .any(|w| w.contains("[runners.default]")
                    && w.contains("both system_prompt and system_prompt_file")
                    && w.contains("prompt ignored")),
            "expected both configured warning, got {:?}",
            settings.config_warnings
        );
        assert!(resolve(&settings, "cursor", PromptRole::Chat).is_none());
    }

    // TC-1708 (FR-1700): over 32KiB warns
    #[test]
    fn agent_prompt_greater_than_32kib_warns_and_ignored() {
        let ws = tempfile::tempdir().unwrap();
        let too_large = "x".repeat(32_769);
        let config = format!(
            r#"
[runners.default]
system_prompt = "{too_large}"
"#
        );
        std::fs::write(ws.path().join("dmuxai.toml"), config).unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);

        assert!(
            settings
                .config_warnings
                .iter()
                .any(|w| w.contains("[runners.default]")
                    && w.contains("exceeds 32 KiB (32768 bytes)")
                    && w.contains("prompt ignored")),
            "expected >32 KiB warning, got {:?}",
            settings.config_warnings
        );
        assert!(resolve(&settings, "cursor", PromptRole::Chat).is_none());

        // Exactly 32 KiB (32768 bytes) is accepted
        let ws_exact = tempfile::tempdir().unwrap();
        let exact_32kib = "y".repeat(32_768);
        let config_exact = format!(
            r#"
[runners.default]
system_prompt = "{exact_32kib}"
"#
        );
        std::fs::write(ws_exact.path().join("dmuxai.toml"), config_exact).unwrap();
        let settings_exact = DmuxSettings::load_with_user_config(ws_exact.path(), None);
        assert!(
            !settings_exact
                .config_warnings
                .iter()
                .any(|w| w.contains("exceeds 32 KiB")),
            "unexpected 32 KiB warning"
        );
        let resolved = resolve(&settings_exact, "cursor", PromptRole::Chat)
            .expect("exact 32 KiB prompt accepted");
        assert_eq!(resolved.text.len(), 32_768);
    }

    // TC-1708 (FR-1700): unreadable warns
    #[test]
    fn agent_prompt_unreadable_file_warns_and_ignored() {
        let ws = tempfile::tempdir().unwrap();
        let config = r#"
[runners.default]
system_prompt_file = "missing_prompt_file.txt"
"#;
        std::fs::write(ws.path().join("dmuxai.toml"), config).unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);

        assert!(
            settings
                .config_warnings
                .iter()
                .any(|w| w.contains("[runners.default]")
                    && w.contains("failed to read system_prompt_file")
                    && w.contains("prompt ignored")),
            "expected file read warning, got {:?}",
            settings.config_warnings
        );
        assert!(resolve(&settings, "cursor", PromptRole::Chat).is_none());
    }

    // TC-1709 (FR-1703): workspace overrides user
    #[test]
    fn agent_prompt_workspace_overrides_user() {
        let user_dir = tempfile::tempdir().unwrap();
        let user_toml = user_dir.path().join("user.toml");
        std::fs::write(
            &user_toml,
            r#"
[runners.default]
system_prompt = "User prompt"
prompt_inject = "always"
"#,
        )
        .unwrap();

        let ws = tempfile::tempdir().unwrap();
        std::fs::write(
            ws.path().join("dmuxai.toml"),
            r#"
[runners.default]
system_prompt = "Workspace prompt"
"#,
        )
        .unwrap();

        let settings = DmuxSettings::load_with_user_config(ws.path(), Some(&user_toml));
        let resolved = resolve(&settings, "cursor", PromptRole::Chat).expect("resolved");
        assert_eq!(resolved.text, "Workspace prompt");
        assert_eq!(resolved.source, "workspace:[runners.default]");
        // prompt_inject was not specified in workspace, so it inherited "always" from user config
        assert_eq!(resolved.inject, PromptInject::Always);
    }

    // TC-1709 (FR-1703): runner overrides default
    #[test]
    fn agent_prompt_runner_id_overrides_default() {
        let ws = tempfile::tempdir().unwrap();
        let config = r#"
[runners.default]
system_prompt = "Default prompt"
prompt_inject = "always"

[runners.codex]
system_prompt = "Codex prompt"
"#;
        std::fs::write(ws.path().join("dmuxai.toml"), config).unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);

        // codex gets its own prompt, but inherits prompt_inject = always from default
        let codex_prompt = resolve(&settings, "codex", PromptRole::Chat).expect("codex prompt");
        assert_eq!(codex_prompt.text, "Codex prompt");
        assert_eq!(codex_prompt.source, "workspace:[runners.codex]");
        assert_eq!(codex_prompt.inject, PromptInject::Always);

        // cursor inherits from default
        let cursor_prompt = resolve(&settings, "cursor", PromptRole::Chat).expect("cursor prompt");
        assert_eq!(cursor_prompt.text, "Default prompt");
        assert_eq!(cursor_prompt.source, "workspace:[runners.default]");
        assert_eq!(cursor_prompt.inject, PromptInject::Always);
    }

    // TC-1710 (FR-1701): default inject session
    #[test]
    fn agent_prompt_default_inject_session() {
        let ws = tempfile::tempdir().unwrap();
        let config = r#"
[runners.default]
system_prompt = "Test prompt"
"#;
        std::fs::write(ws.path().join("dmuxai.toml"), config).unwrap();
        let mut env = RuntimeEnv::from_process();
        env.trust_workspace_config = true;
        let settings = DmuxSettings::load_with_user_config_and_env(ws.path(), None, &env);
        let resolved = resolve(&settings, "codex", PromptRole::Chat).expect("resolved");
        assert_eq!(resolved.inject, PromptInject::Session);
    }

    // TC-1710 (FR-1701): always parsed
    #[test]
    fn agent_prompt_always_parsed() {
        let ws = tempfile::tempdir().unwrap();
        let config = r#"
[runners.default]
system_prompt = "Test prompt"
prompt_inject = "always"
"#;
        std::fs::write(ws.path().join("dmuxai.toml"), config).unwrap();
        let mut env = RuntimeEnv::from_process();
        env.trust_workspace_config = true;
        let settings = DmuxSettings::load_with_user_config_and_env(ws.path(), None, &env);
        let resolved = resolve(&settings, "codex", PromptRole::Chat).expect("resolved");
        assert_eq!(resolved.inject, PromptInject::Always);
    }

    // TC-1710 (FR-1701): unknown inject warns
    #[test]
    fn agent_prompt_unknown_inject_warns() {
        let ws = tempfile::tempdir().unwrap();
        let config = r#"
[runners.default]
system_prompt = "Test prompt"
prompt_inject = "immediately"
"#;
        std::fs::write(ws.path().join("dmuxai.toml"), config).unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);

        assert!(
            settings
                .config_warnings
                .iter()
                .any(|w| w.contains("[runners.default]")
                    && w.contains("unknown prompt_inject 'immediately'")
                    && w.contains("using default 'session'")),
            "expected unknown prompt_inject warning, got {:?}",
            settings.config_warnings
        );
        let resolved = resolve(&settings, "codex", PromptRole::Chat).expect("resolved");
        assert_eq!(resolved.inject, PromptInject::Session);
    }

    // TC-1711 (FR-1702): default roles
    #[test]
    fn agent_prompt_default_roles() {
        let ws = tempfile::tempdir().unwrap();
        let config = r#"
[runners.default]
system_prompt = "Default roles prompt"
"#;
        std::fs::write(ws.path().join("dmuxai.toml"), config).unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);

        assert!(resolve(&settings, "cursor", PromptRole::Chat).is_some());
        assert!(resolve(&settings, "cursor", PromptRole::Worker).is_some());
        assert!(resolve(&settings, "cursor", PromptRole::Judge).is_none());
    }

    // TC-1711 (FR-1702): excluded roles dropped
    #[test]
    fn agent_prompt_judge_triage_planner_replan_summary_interview_excluded_even_if_listed() {
        let ws = tempfile::tempdir().unwrap();
        let config = r#"
[runners.default]
system_prompt = "Test prompt"
prompt_roles = [
    "chat",
    "worker",
    "judge",
    "triage",
    "planner",
    "replan",
    "summary",
    "interview",
    "unknown_role"
]
"#;
        std::fs::write(ws.path().join("dmuxai.toml"), config).unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);

        for excluded in &[
            "judge",
            "triage",
            "planner",
            "replan",
            "summary",
            "interview",
        ] {
            assert!(
                settings
                    .config_warnings
                    .iter()
                    .any(|w| w.contains("[runners.default]")
                        && w.contains(&format!(
                            "role '{excluded}' in prompt_roles cannot receive system prompts"
                        ))),
                "expected warning for role {excluded}, got {:?}",
                settings.config_warnings
            );
        }
        assert!(
            settings
                .config_warnings
                .iter()
                .any(|w| w.contains("[runners.default]")
                    && w.contains("unknown role 'unknown_role' in prompt_roles ignored")),
            "expected warning for unknown_role, got {:?}",
            settings.config_warnings
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

    // TC-1711 (FR-1702): runner case insensitive
    #[test]
    fn agent_prompt_runner_case_insensitive() {
        let ws = tempfile::tempdir().unwrap();
        let config = r#"
[runners.CoDeX]
system_prompt = "Case-insensitive codex prompt"
"#;
        std::fs::write(ws.path().join("dmuxai.toml"), config).unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);

        let r1 = resolve(&settings, "codex", PromptRole::Chat).expect("codex resolved");
        let r2 = resolve(&settings, "CODEX", PromptRole::Chat).expect("CODEX resolved");
        let r3 = resolve(&settings, "  CoDeX  ", PromptRole::Chat).expect("padded resolved");
        assert_eq!(r1.text, "Case-insensitive codex prompt");
        assert_eq!(r2.text, "Case-insensitive codex prompt");
        assert_eq!(r3.text, "Case-insensitive codex prompt");
    }

    // TC-1709 (FR-1703): workspace runner overrides user default
    #[test]
    fn agent_prompt_workspace_runner_overrides_user_default() {
        let user_dir = tempfile::tempdir().unwrap();
        let user_toml = user_dir.path().join("user.toml");
        std::fs::write(
            &user_toml,
            r#"
[runners.default]
system_prompt = "User default"
"#,
        )
        .unwrap();

        let ws = tempfile::tempdir().unwrap();
        std::fs::write(
            ws.path().join("dmuxai.toml"),
            r#"
[runners.codex]
system_prompt = "Workspace codex"
"#,
        )
        .unwrap();

        let settings = DmuxSettings::load_with_user_config(ws.path(), Some(&user_toml));
        let codex_prompt = resolve(&settings, "codex", PromptRole::Chat).expect("codex");
        assert_eq!(codex_prompt.text, "Workspace codex");
        assert_eq!(codex_prompt.source, "workspace:[runners.codex]");

        let cursor_prompt = resolve(&settings, "cursor", PromptRole::Chat).expect("cursor");
        assert_eq!(cursor_prompt.text, "User default");
        assert_eq!(cursor_prompt.source, "user:[runners.default]");
    }

    // TC-1753 / FR-1731: accepted runner model and workspace override; invalid dropped
    #[test]
    fn tc_1753_runner_model_accepted_and_invalid_dropped() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(
            ws.path().join("dmuxai.toml"),
            r#"
[runners.agy]
model = "gemini-3.8-flash-high"
"#,
        )
        .unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);
        assert_eq!(
            settings.runner_models.get("agy"),
            Some(&"gemini-3.8-flash-high".to_owned())
        );

        let user_dir = tempfile::tempdir().unwrap();
        let user_toml = user_dir.path().join("user.toml");
        std::fs::write(
            &user_toml,
            r#"
[runners.agy]
model = "gemini-3.0-flash-low"
"#,
        )
        .unwrap();
        let ws2 = tempfile::tempdir().unwrap();
        std::fs::write(
            ws2.path().join("dmuxai.toml"),
            r#"
[runners.agy]
model = "gemini-3.8-flash-high"
"#,
        )
        .unwrap();
        let settings2 = DmuxSettings::load_with_user_config(ws2.path(), Some(&user_toml));
        assert_eq!(
            settings2.runner_models.get("agy"),
            Some(&"gemini-3.8-flash-high".to_owned())
        );

        let ws3 = tempfile::tempdir().unwrap();
        std::fs::write(
            ws3.path().join("dmuxai.toml"),
            r#"
[runners.agy]
model = "gemini-4-argon"
"#,
        )
        .unwrap();
        let settings3 = DmuxSettings::load_with_user_config(ws3.path(), None);
        assert!(!settings3.runner_models.contains_key("agy"));
        assert!(
            settings3
                .config_warnings
                .iter()
                .any(|w| w.contains("gemini-4-argon") && w.contains("not allowed"))
        );
    }

    // TC-1754 / FR-1731: default ignored, custom accepted, unknown dropped
    #[test]
    fn tc_1754_runner_model_default_custom_unknown() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(
            ws.path().join("dmuxai.toml"),
            r#"
[runners.default]
model = "gemini-3.8-flash-high"
"#,
        )
        .unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);
        assert!(!settings.runner_models.contains_key("default"));
        assert!(settings.config_warnings.iter().any(|w| {
            w.contains("[runners.default] model is not allowed under [runners.default]; ignored")
        }));

        let ws2 = tempfile::tempdir().unwrap();
        std::fs::write(
            ws2.path().join("dmuxai.toml"),
            r#"
[[agents.custom]]
name = "oc"
command = ["oc"]
allowed_models = "^opencode/.*-free$"

[runners.oc]
model = "opencode/beam-free"
"#,
        )
        .unwrap();
        let mut env = RuntimeEnv::from_process();
        env.trust_workspace_config = true;
        let settings2 = DmuxSettings::load_with_user_config_and_env(ws2.path(), None, &env);
        assert_eq!(
            settings2.runner_models.get("oc"),
            Some(&"opencode/beam-free".to_owned())
        );

        let ws3 = tempfile::tempdir().unwrap();
        std::fs::write(
            ws3.path().join("dmuxai.toml"),
            r#"
[runners.bogus]
model = "opencode/beam-free"
"#,
        )
        .unwrap();
        let settings3 = DmuxSettings::load_with_user_config(ws3.path(), None);
        assert!(!settings3.runner_models.contains_key("bogus"));
        assert!(
            settings3
                .config_warnings
                .iter()
                .any(|w| w.contains("unknown runner"))
        );
    }

    // TC-2150 / FR-2150: parse per-model prompts and reject the default table.
    #[test]
    fn tc_2150_settings_parse_model_prompts_and_default_warns() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("model.txt"), "Model file prompt").unwrap();
        std::fs::write(
            ws.path().join("dmuxai.toml"),
            r#"
[runners.cursor]
system_prompt = "Runner prompt"
prompt_inject = "always"

[runners.cursor.models."Composer-2.5"]
system_prompt_file = "model.txt"
prompt_inject = "session"

[runners.cursor.models."inline-model"]
system_prompt = "Inline model prompt"

[runners.default.models."composer-2.5"]
system_prompt = "ignored"
"#,
        )
        .unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);

        let cursor = settings.runner_prompts.get("cursor").expect("cursor table");
        let model = cursor
            .models
            .as_ref()
            .and_then(|models| models.get("composer-2.5"))
            .expect("case-insensitive model entry");
        let prompt = model.prompt.as_ref().expect("model prompt");
        assert_eq!(prompt.text, "Model file prompt");
        assert_eq!(
            prompt.source,
            "workspace:[runners.cursor.models.\"Composer-2.5\"] (model.txt)"
        );
        assert_eq!(model.inject, Some(PromptInject::Session));

        let inline = cursor
            .models
            .as_ref()
            .and_then(|models| models.get("inline-model"))
            .and_then(|model| model.prompt.as_ref())
            .expect("inline model prompt");
        assert_eq!(inline.text, "Inline model prompt");
        assert_eq!(
            inline.source,
            "workspace:[runners.cursor.models.\"inline-model\"]"
        );

        let default = settings
            .runner_prompts
            .get("default")
            .expect("default table exists");
        assert!(
            default.models.is_none(),
            "[runners.default.models] must be ignored"
        );
        assert!(
            settings.config_warnings.iter().any(|w| {
                w.contains("[runners.default.models]")
                    && w.contains("not allowed under [runners.default]")
                    && w.contains("ignored")
            }),
            "warnings: {:?}",
            settings.config_warnings
        );
        assert!(
            !settings
                .config_warnings
                .iter()
                .any(|w| w.contains("unknown") && w.contains("models")),
            "models key must not trigger an unknown-key warning: {:?}",
            settings.config_warnings
        );

        let resolved = resolve_for(
            &settings,
            "cursor",
            Some("composer-2.5"),
            PromptRole::Chat,
            true,
        )
        .expect("model prompt resolved");
        assert_eq!(resolved.text, "Model file prompt");
    }

    // TC-2159 / FR-2150: model prompt tables share runner validation rules.
    #[test]
    fn tc_2159_settings_model_prompt_validation_warns() {
        let ws = tempfile::tempdir().unwrap();
        let too_large = "x".repeat(32_769);
        let config = format!(
            r#"
[runners.cursor.models."composer-2.5"]
system_prompt = "inline"
system_prompt_file = "missing.txt"

[runners.cursor.models."big"]
system_prompt = "{too_large}"

[runners.cursor.models."bad-inject"]
system_prompt = "ok"
prompt_inject = "immediately"
"#
        );
        std::fs::write(ws.path().join("dmuxai.toml"), config).unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);

        assert!(settings.config_warnings.iter().any(|w| {
            w.contains("[runners.cursor.models.\"composer-2.5\"]")
                && w.contains("both system_prompt and system_prompt_file")
        }));
        assert!(settings.config_warnings.iter().any(|w| {
            w.contains("[runners.cursor.models.\"big\"]")
                && w.contains("exceeds 32 KiB (32768 bytes)")
        }));
        assert!(settings.config_warnings.iter().any(|w| {
            w.contains("[runners.cursor.models.\"bad-inject\"]")
                && w.contains("unknown prompt_inject 'immediately'")
        }));
    }

    // TC-2151 / FR-2150, FR-2151: workspace overrides user per model key;
    // builtin_prompt merges per key too.
    #[test]
    fn tc_2151_settings_model_prompt_workspace_overrides_user() {
        let user_dir = tempfile::tempdir().unwrap();
        let user_toml = user_dir.path().join("user.toml");
        std::fs::write(
            &user_toml,
            r#"
[runners.cursor]
builtin_prompt = false

[runners.default]
builtin_prompt = false

[runners.cursor.models."composer-2.5"]
system_prompt = "user model"
prompt_inject = "always"

[runners.cursor.models."user-only"]
system_prompt = "user only model"
"#,
        )
        .unwrap();

        let ws = tempfile::tempdir().unwrap();
        std::fs::write(
            ws.path().join("dmuxai.toml"),
            r#"
[runners.cursor]
builtin_prompt = true

[runners.cursor.models."composer-2.5"]
system_prompt = "workspace model"
"#,
        )
        .unwrap();

        let settings = DmuxSettings::load_with_user_config(ws.path(), Some(&user_toml));
        let cursor = settings.runner_prompts.get("cursor").expect("cursor table");
        assert_eq!(cursor.builtin_prompt, Some(true));

        let models = cursor.models.as_ref().expect("models");
        let composer = models.get("composer-2.5").expect("composer entry");
        let prompt = composer.prompt.as_ref().expect("composer prompt");
        assert_eq!(prompt.text, "workspace model");
        assert_eq!(
            prompt.source,
            "workspace:[runners.cursor.models.\"composer-2.5\"]"
        );
        // The workspace entry did not set prompt_inject, so the user value stays.
        assert_eq!(composer.inject, Some(PromptInject::Always));

        let user_only = models.get("user-only").expect("user-only entry");
        assert_eq!(
            user_only.prompt.as_ref().map(|p| p.text.as_str()),
            Some("user only model")
        );

        // A runner table's builtin_prompt beats the default table's value, so
        // cursor still resolves its built-in while agy (unset) does not.
        assert_eq!(
            settings
                .runner_prompts
                .get("default")
                .and_then(|c| c.builtin_prompt),
            Some(false)
        );
        let cursor_builtin = resolve_for(
            &settings,
            "cursor",
            Some("unknown-model"),
            PromptRole::Chat,
            true,
        )
        .expect("cursor builtin");
        assert_eq!(cursor_builtin.source, "builtin:runner:cursor");
        assert!(
            resolve_for(
                &settings,
                "agy",
                Some("gemini-3.8-flash-low"),
                PromptRole::Chat,
                true,
            )
            .is_none()
        );
    }

    // TC-2158 / FR-2152: an explicitly empty prompt at the model or runner
    // level means "no prompt" and stops resolution.
    #[test]
    fn tc_2158_explicit_empty_prompt_stops_resolution() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(
            ws.path().join("dmuxai.toml"),
            r#"
[runners.default]
system_prompt = "default prompt"

[runners.cursor]
system_prompt = "runner prompt"

[runners.cursor.models."composer-2.5"]
system_prompt = "   "

[runners.agy]
system_prompt = ""
"#,
        )
        .unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);

        // The blank model entry stops before the runner prompt and built-in.
        assert!(
            resolve_for(
                &settings,
                "cursor",
                Some("composer-2.5"),
                PromptRole::Chat,
                true,
            )
            .is_none()
        );

        // The blank runner prompt stops before the default and built-in.
        assert!(
            resolve_for(
                &settings,
                "agy",
                Some("gemini-3.8-flash-low"),
                PromptRole::Chat,
                true,
            )
            .is_none()
        );

        // The non-blank runner prompt still wins over the default.
        let resolved = resolve_for(&settings, "cursor", Some("other"), PromptRole::Chat, true)
            .expect("runner prompt");
        assert_eq!(resolved.text, "runner prompt");

        // An unset runner still falls through to the default prompt.
        let resolved =
            resolve_for(&settings, "codex", None, PromptRole::Chat, true).expect("default prompt");
        assert_eq!(resolved.text, "default prompt");
    }
}

#[cfg(test)]
mod runner_fallback_settings_tests {
    use super::*;
    use crate::workspace_trust::TrustSource;

    // tc_2101 (settings): parse fallback tables from user + workspace configs.
    #[test]
    fn tc_2101_settings_parse_fallback_tables() {
        let ws = tempfile::tempdir().unwrap();
        let user_dir = tempfile::tempdir().unwrap();
        let user_toml = user_dir.path().join("user.toml");
        std::fs::write(&user_toml, "[runners.default]\nfallback = [\"codex\"]\n").unwrap();
        std::fs::write(
            ws.path().join("dmuxai.toml"),
            "[runners.agy]\nfallback = [\"agy:gemini-3.8-flash-low\", \"codex\"]\nstall_timeout_secs = 120\n",
        )
        .unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), Some(&user_toml));

        let chain = settings
            .runner_fallback
            .chain_for("agy", Some("gemini-3.8-flash-high"));
        assert_eq!(
            chain,
            vec![
                crate::runner_fallback::FallbackTarget {
                    runner: "agy".to_owned(),
                    model: Some("gemini-3.8-flash-low".to_owned()),
                },
                crate::runner_fallback::FallbackTarget {
                    runner: "codex".to_owned(),
                    model: None,
                },
            ]
        );

        let cfg = settings.runner_fallback.config_for("agy").unwrap();
        assert_eq!(cfg.stall_timeout_secs, Some(120));
        assert_eq!(cfg.effective_stall_api_retries(), 3);

        let chain2 = settings.runner_fallback.chain_for("cursor", None);
        assert_eq!(
            chain2,
            vec![crate::runner_fallback::FallbackTarget {
                runner: "codex".to_owned(),
                model: None,
            }]
        );
    }

    // tc_2102 (settings): disallowed model and unknown runner are ignored.
    #[test]
    fn tc_2102_settings_fallback_rejects_disallowed_model() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(
            ws.path().join("dmuxai.toml"),
            "[runners.agy]\nfallback = [\"agy:claude-opus\", \"nosuchrunner\", \"codex\"]\n",
        )
        .unwrap();
        let settings = DmuxSettings::load_with_user_config(ws.path(), None);

        let chain = settings.runner_fallback.chain_for("agy", None);
        assert_eq!(
            chain,
            vec![crate::runner_fallback::FallbackTarget {
                runner: "codex".to_owned(),
                model: None,
            }]
        );

        assert!(
            settings
                .config_warnings
                .iter()
                .any(|w| w.contains("fallback 'agy:claude-opus' ignored")),
            "warnings: {:?}",
            settings.config_warnings
        );
        assert!(
            settings
                .config_warnings
                .iter()
                .any(|w| w
                    .contains("fallback 'nosuchrunner' ignored: unknown runner 'nosuchrunner'")),
            "warnings: {:?}",
            settings.config_warnings
        );
    }

    #[test]
    fn tc_2198_untrusted_notify_keys_stripped_and_warnings_sanitized() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();

        let ws_toml = r#"
[notify]
command = ["sh", "-c", "echo malicious"]
webhook_url = "https://secret-hook.invalid/hook"
webhook_secret_env = "VERY_SECRET_ENV_VAR"
secret_header = "X-Super-Secret"
events = ["run_failed", "run_succeeded"]
"#;
        std::fs::write(ws.join("dmuxai.toml"), ws_toml).unwrap();

        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_env(&ws, &env);
        assert_eq!(settings.notify.command, None);
        assert_eq!(settings.notify.webhook_url, None);
        assert_eq!(settings.notify.webhook_secret_env, None);
        assert_eq!(settings.notify.secret_header, None);
        // Events are kept!
        assert_eq!(
            settings.notify.events.as_deref(),
            Some(["run_failed".to_owned(), "run_succeeded".to_owned()].as_slice())
        );

        match &settings.workspace_trust {
            WorkspaceTrustStatus::Untrusted { sha256: _, ignored } => {
                assert!(ignored.contains(&"notify.command".to_owned()));
                assert!(ignored.contains(&"notify.webhook_url".to_owned()));
                assert!(ignored.contains(&"notify.webhook_secret_env".to_owned()));
                assert!(ignored.contains(&"notify.secret_header".to_owned()));
            }
            other => panic!("expected Untrusted, got {other:?}"),
        }

        // Exactly one trust warning naming the keys and NOT containing URL or env var name values
        let trust_warnings: Vec<&String> = settings
            .config_warnings
            .iter()
            .filter(|w| w.contains("workspace dmuxai.toml is not trusted"))
            .collect();
        assert_eq!(trust_warnings.len(), 1);
        let warn = trust_warnings[0];
        assert!(warn.contains("notify.command"));
        assert!(warn.contains("notify.webhook_url"));
        assert!(warn.contains("notify.webhook_secret_env"));
        assert!(warn.contains("notify.secret_header"));
        assert!(!warn.contains("https://secret-hook.invalid"));
        assert!(!warn.contains("VERY_SECRET_ENV_VAR"));
        assert!(!warn.contains("X-Super-Secret"));
    }

    #[test]
    fn tc_2199_untrusted_custom_agents_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();

        let ws_toml = r#"
[[agents.custom]]
name = "ws-agent"
command = ["echo", "test"]
"#;
        std::fs::write(ws.join("dmuxai.toml"), ws_toml).unwrap();

        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_env(&ws, &env);
        assert!(
            settings.custom_agents.iter().all(|a| a.name != "ws-agent"),
            "custom agent should be absent"
        );
        assert!(matches!(
            settings.workspace_trust,
            WorkspaceTrustStatus::Untrusted { .. }
        ));
    }

    #[test]
    fn tc_2200_untrusted_mcp_servers_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();

        let ws_toml = r#"
[[mcp.servers]]
name = "ws-mcp"
command = "echo"
args = ["mcp"]
"#;
        std::fs::write(ws.join("dmuxai.toml"), ws_toml).unwrap();

        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_env(&ws, &env);
        assert!(
            settings.mcp_servers.iter().all(|s| s.name != "ws-mcp"),
            "mcp server should be absent"
        );
        assert!(matches!(
            settings.workspace_trust,
            WorkspaceTrustStatus::Untrusted { .. }
        ));
    }

    #[test]
    fn tc_2201_untrusted_skills_git_vault_outside_and_jira_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let outside_vault = dir.path().join("outside_vault");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&outside_vault).unwrap();

        let ws_toml = format!(
            r#"
[skills.sources.remote-git]
git = "https://github.com/foo/bar.git"

[memory]
vault = "{}"

[todo]
jira_base_url = "https://malicious.atlassian.net"
"#,
            outside_vault.display()
        );
        std::fs::write(ws.join("dmuxai.toml"), ws_toml).unwrap();

        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_env(&ws, &env);
        assert!(!settings.stage_skills.sources.contains_key("remote-git"));
        assert_eq!(settings.memory, None);
        assert_eq!(settings.todo_jira_base_url, None);
        assert!(matches!(
            settings.workspace_trust,
            WorkspaceTrustStatus::Untrusted { .. }
        ));
    }

    #[test]
    fn tc_2202_user_config_path_loads_unchanged_without_warning() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let user_toml = dir.path().join("user.toml");
        std::fs::create_dir_all(&ws).unwrap();

        let user_content = r#"
[notify]
command = ["echo", "from-user"]
webhook_url = "https://user-hook.invalid/hook"
webhook_secret_env = "USER_SECRET_ENV"
secret_header = "X-User-Secret"
events = ["run_failed"]

[[agents.custom]]
name = "user-agent"
command = ["echo", "user"]

[[mcp.servers]]
name = "user-mcp"
command = "echo"
args = ["user"]
"#;
        std::fs::write(&user_toml, user_content).unwrap();

        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_user_config_and_env(&ws, Some(&user_toml), &env);
        // Status is NoWorkspaceConfig when workspace has no dmuxai.toml
        assert_eq!(
            settings.workspace_trust,
            WorkspaceTrustStatus::NoWorkspaceConfig
        );
        assert_eq!(
            settings.notify.command,
            Some(vec!["echo".to_owned(), "from-user".to_owned()])
        );
        assert_eq!(
            settings.notify.webhook_url.as_deref(),
            Some("https://user-hook.invalid/hook")
        );
        assert_eq!(
            settings.notify.webhook_secret_env.as_deref(),
            Some("USER_SECRET_ENV")
        );
        assert_eq!(
            settings.notify.secret_header.as_deref(),
            Some("X-User-Secret")
        );
        assert!(
            settings
                .custom_agents
                .iter()
                .any(|a| a.name == "user-agent")
        );
        assert!(settings.mcp_servers.iter().any(|s| s.name == "user-mcp"));
        assert!(
            !settings
                .config_warnings
                .iter()
                .any(|w| w.contains("is not trusted")),
            "unexpected trust warning in user config: {:?}",
            settings.config_warnings
        );
    }

    #[test]
    fn tc_2203_trust_record_applied_modify_file_reverts_to_untrusted() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let config_path = ws.join("dmuxai.toml");

        let content1 = r#"
[notify]
command = ["echo", "ws-notify"]
"#;
        std::fs::write(&config_path, content1).unwrap();
        let hash1 = content_hash(content1.as_bytes());

        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir.clone()),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };

        // Initially untrusted
        let settings_init = DmuxSettings::load_with_env(&ws, &env);
        assert!(matches!(
            settings_init.workspace_trust,
            WorkspaceTrustStatus::Untrusted { .. }
        ));

        // Trust exact hash
        let mut store = TrustStore::load(&env);
        store.trust(&ws, &hash1);
        store.save().unwrap();

        // Applied as Trusted(Record)
        let settings_trusted = DmuxSettings::load_with_env(&ws, &env);
        assert_eq!(
            settings_trusted.workspace_trust,
            WorkspaceTrustStatus::Trusted(TrustSource::Record)
        );
        assert_eq!(
            settings_trusted.notify.command,
            Some(vec!["echo".to_owned(), "ws-notify".to_owned()])
        );

        // Modify the file after trusting
        let content2 = format!("{content1}\n# modified comment\n");
        std::fs::write(&config_path, &content2).unwrap();
        let hash2 = content_hash(content2.as_bytes());
        assert_ne!(hash1, hash2);

        // Untrusted again with the new sha
        let settings_modified = DmuxSettings::load_with_env(&ws, &env);
        match settings_modified.workspace_trust {
            WorkspaceTrustStatus::Untrusted { sha256, .. } => {
                assert_eq!(sha256, hash2);
            }
            other => panic!("expected Untrusted, got {other:?}"),
        }
    }

    #[test]
    fn tc_2204_opt_in_yields_trusted_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(
            ws.join("dmuxai.toml"),
            "[notify]\ncommand = [\"echo\", \"optin\"]\n",
        )
        .unwrap();

        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: true,
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_env(&ws, &env);
        assert_eq!(
            settings.workspace_trust,
            WorkspaceTrustStatus::Trusted(TrustSource::OptIn)
        );
        assert_eq!(
            settings.notify.command,
            Some(vec!["echo".to_owned(), "optin".to_owned()])
        );
    }

    #[test]
    fn tc_2205_harmless_workspace_config_is_not_needed_without_warning() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();

        let ws_toml = r#"
[ui]
status_bar = false

[graph]
max_parallel = 4
"#;
        std::fs::write(ws.join("dmuxai.toml"), ws_toml).unwrap();

        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_env(&ws, &env);
        assert_eq!(settings.workspace_trust, WorkspaceTrustStatus::NotNeeded);
        assert!(
            !settings
                .config_warnings
                .iter()
                .any(|w| w.contains("is not trusted")),
            "unexpected warning: {:?}",
            settings.config_warnings
        );
    }

    #[test]
    fn tc_2206_corrupt_store_yields_untrusted_and_warning() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();

        std::fs::write(
            ws.join("dmuxai.toml"),
            "[notify]\ncommand = [\"echo\", \"corrupt\"]\n",
        )
        .unwrap();

        let state_dir = dir.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("trusted-workspaces.json"),
            "INVALID NOT JSON",
        )
        .unwrap();

        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_env(&ws, &env);
        assert!(matches!(
            settings.workspace_trust,
            WorkspaceTrustStatus::Untrusted { .. }
        ));
        assert!(
            settings
                .config_warnings
                .iter()
                .any(|w| w.contains("is corrupt")),
            "missing corrupt warning: {:?}",
            settings.config_warnings
        );
    }

    #[test]
    fn test_untrusted_skills_source_subdir_dotdot_and_absolute_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();

        let ws_toml = r#"
[skills.sources.bad-dotdot]
path = "skills"
subdir = "../outside"

[skills.sources.bad-abs]
path = "skills"
subdir = "/etc"
"#;
        std::fs::write(ws.join("dmuxai.toml"), ws_toml).unwrap();

        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_env(&ws, &env);
        assert!(!settings.stage_skills.sources.contains_key("bad-dotdot"));
        assert!(!settings.stage_skills.sources.contains_key("bad-abs"));
        assert!(matches!(
            settings.workspace_trust,
            WorkspaceTrustStatus::Untrusted { .. }
        ));
    }

    #[test]
    #[cfg(unix)]
    fn tc_2220_untrusted_skills_source_symlink_dir_outside_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let outside = dir.path().join("outside_skills");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        // Create symlink inside ws pointing outside
        let symlink_path = ws.join("linked_skills");
        std::os::unix::fs::symlink(&outside, &symlink_path).unwrap();

        let ws_toml = r#"
[skills.sources.symlink-source]
path = "linked_skills"
"#;
        std::fs::write(ws.join("dmuxai.toml"), ws_toml).unwrap();

        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_env(&ws, &env);
        assert!(!settings.stage_skills.sources.contains_key("symlink-source"));
        assert!(matches!(
            settings.workspace_trust,
            WorkspaceTrustStatus::Untrusted { .. }
        ));
    }

    #[test]
    #[cfg(unix)]
    fn tc_2221_untrusted_memory_symlink_dir_outside_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let outside = dir.path().join("outside_mem");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        // Create symlink inside ws pointing outside
        let symlink_path = ws.join("linked_mem");
        std::os::unix::fs::symlink(&outside, &symlink_path).unwrap();

        let ws_toml = r#"
[memory]
vault = "linked_mem"
dir = "notes"
"#;
        std::fs::write(ws.join("dmuxai.toml"), ws_toml).unwrap();

        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_env(&ws, &env);
        assert_eq!(settings.memory, None);
        assert!(matches!(
            settings.workspace_trust,
            WorkspaceTrustStatus::Untrusted { .. }
        ));
    }

    #[test]
    fn test_untrusted_memory_user_vault_workspace_dir_outside_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let user_toml = dir.path().join("user.toml");
        let outside = dir.path().join("outside_vault");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        std::fs::write(
            &user_toml,
            format!("[memory]\nvault = \"{}\"\n", outside.display()),
        )
        .unwrap();

        let ws_toml = r#"
[memory]
dir = "sub"
"#;
        std::fs::write(ws.join("dmuxai.toml"), ws_toml).unwrap();

        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_user_config_and_env(&ws, Some(&user_toml), &env);
        // The resulting root (outside/sub) is outside ws, so workspace memory.dir is stripped.
        // As a result, memory is resolved using user vault + default dir, not workspace dir.
        assert!(matches!(
            settings.workspace_trust,
            WorkspaceTrustStatus::Untrusted { .. }
        ));
        if let Some(mem) = &settings.memory {
            assert_ne!(mem.dir, "sub");
        }
    }

    #[test]
    fn test_untrusted_workspace_user_config_skills_source_outside_workspace_loads() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let user_toml = dir.path().join("user.toml");
        let outside_skill = dir.path().join("outside_skill");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&outside_skill).unwrap();

        // Write a skill in outside_skill
        std::fs::write(
            outside_skill.join("SKILL.md"),
            "---\nname: user-ext\n---\nUser ext skill",
        )
        .unwrap();

        std::fs::write(
            &user_toml,
            format!(
                r#"
[skills.sources.user-source]
path = "{}"
"#,
                outside_skill.display().to_string().replace('\\', "/")
            ),
        )
        .unwrap();

        // Workspace config has restricted keys to make it Untrusted
        let ws_toml = r#"
[notify]
command = ["echo", "untrusted"]
"#;
        std::fs::write(ws.join("dmuxai.toml"), ws_toml).unwrap();

        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_user_config_and_env(&ws, Some(&user_toml), &env);
        assert!(matches!(
            settings.workspace_trust,
            WorkspaceTrustStatus::Untrusted { .. }
        ));

        // The user-config source should still resolve even though workspace is untrusted
        let ctx = crate::stage_skills::SkillContext {
            workspace: ws.clone(),
            home: None,
        };
        let resolved = crate::stage_skills::resolve(
            "user-source",
            &settings.stage_skills,
            &ctx,
            &crate::stage_skills::Approvals::default(),
        );
        assert!(
            resolved.is_ok(),
            "user source outside workspace should load"
        );
        assert_eq!(resolved.unwrap().name, "user-ext");
    }

    #[test]
    fn tc_2224_restricted_workspace_details_hides_secret_values() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();

        let secret_env_name = "DMUXAI_TEST_SECRET_SENTINEL";
        let sentinel_value = "SUPER_SECRET_VALUE_DO_NOT_LEAK";

        let ws_toml = format!(
            r#"
[notify]
command = ["sh", "-c", "echo notify"]
webhook_url = "https://example.com/webhook"
webhook_secret_env = "{secret_env_name}"
secret_header = "X-Secret-Auth"

[[agents.custom]]
name = "myagent"
command = ["run-agent", "--flag"]
models_command = ["list-models"]
env = {{ MYAGENT_KEY = "agent_secret_123" }}

[[mcp.servers]]
name = "myserver"
command = "mcp-bin"
args = ["serve", "--port", "1234"]
env = {{ MCPSERVER_TOKEN = "server_secret_456" }}

[skills.sources.remote-skills]
git = "https://github.com/example/skills.git"
rev = "v1.0"
subdir = "pack"

[skills.sources.outside-skills]
path = "../outside"

[memory]
vault = "../external-vault"
dir = "sub"

[todo]
jira_base_url = "https://jira.example.com"

[runners.cursor]
system_prompt_file = "../cursor.prompt"

[runners.cursor.models."claude-3-5"]
system_prompt_file = "../claude.prompt"
"#
        );
        std::fs::write(ws.join("dmuxai.toml"), ws_toml).unwrap();

        let env = RuntimeEnv::default();
        // Set the env var in the test process environment to the sentinel value
        unsafe {
            std::env::set_var(secret_env_name, sentinel_value);
        }

        let details = restricted_workspace_details(&ws, &env);

        let details_map: std::collections::BTreeMap<String, String> = details.into_iter().collect();

        // 1. Notify details
        assert_eq!(
            details_map.get("notify.command"),
            Some(&"sh -c \"echo notify\"".to_string())
        );
        assert_eq!(
            details_map.get("notify.webhook_url"),
            Some(&"https://example.com/webhook".to_string())
        );
        assert_eq!(
            details_map.get("notify.webhook_secret_env"),
            Some(&secret_env_name.to_string())
        );
        assert_eq!(
            details_map.get("notify.secret_header"),
            Some(&"X-Secret-Auth".to_string())
        );

        // 2. Custom agents
        let myagent_detail = details_map.get("agents.custom.myagent").unwrap();
        assert!(myagent_detail.contains("command: run-agent --flag"));
        assert!(myagent_detail.contains("models_command: list-models"));
        assert!(myagent_detail.contains("env: [MYAGENT_KEY]"));
        assert!(!myagent_detail.contains("agent_secret_123"));

        // 3. MCP servers
        let myserver_detail = details_map.get("mcp.servers.myserver").unwrap();
        assert!(myserver_detail.contains("command: mcp-bin serve --port 1234"));
        assert!(myserver_detail.contains("env: [MCPSERVER_TOKEN]"));
        assert!(!myserver_detail.contains("server_secret_456"));

        // 4. Skills sources
        assert_eq!(
            details_map.get("skills.sources.remote-skills"),
            Some(&"git: https://github.com/example/skills.git rev: v1.0 subdir: pack".to_string())
        );
        assert_eq!(
            details_map.get("skills.sources.outside-skills"),
            Some(&"path: ../outside".to_string())
        );

        // 5. Memory
        assert_eq!(
            details_map.get("memory.vault"),
            Some(&"../external-vault".to_string())
        );
        assert_eq!(details_map.get("memory.dir"), Some(&"sub".to_string()));

        // 6. Todo
        assert_eq!(
            details_map.get("todo.jira_base_url"),
            Some(&"https://jira.example.com".to_string())
        );

        // 7. Runners prompt files
        assert_eq!(
            details_map.get("runners.cursor.system_prompt_file"),
            Some(&"../cursor.prompt".to_string())
        );
        assert_eq!(
            details_map.get("runners.cursor.models.claude-3-5.system_prompt_file"),
            Some(&"../claude.prompt".to_string())
        );

        // Crucial security invariant: sentinel value never appears in any detail value
        for (k, v) in &details_map {
            assert!(
                !v.contains(sentinel_value),
                "Detail for {k} exposed secret env value: {v}"
            );
        }

        unsafe {
            std::env::remove_var(secret_env_name);
        }
    }

    #[test]
    fn tc_2243_restricted_details_escape_control_chars() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();

        // TOML basic-string escapes decode to ESC and CR in the command argv.
        let ws_toml = "[notify]\ncommand = [\"echo\", \"a\\u001b[2Jb\\rc\"]\n";
        std::fs::write(ws.join("dmuxai.toml"), ws_toml).unwrap();

        let env = RuntimeEnv {
            dmuxai_state_dir: Some(dir.path().join("state")),
            ..RuntimeEnv::default()
        };

        let details = restricted_workspace_details(&ws, &env);
        let details_map: std::collections::BTreeMap<_, _> = details.into_iter().collect();

        let command = details_map
            .get("notify.command")
            .expect("notify.command detail");
        assert!(
            command.contains("\\u{1b}"),
            "ESC should be escaped: {command}"
        );

        for (key, value) in &details_map {
            assert!(
                !key.chars().any(char::is_control),
                "key {key:?} contains a control character"
            );
            assert!(
                !value.chars().any(char::is_control),
                "value for {key:?} contains a control character: {value:?}"
            );
        }
    }

    #[test]
    fn tc_2244_config_warnings_escape_control_chars() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();

        // A raw ESC byte in an invalid key makes parsing fail before escaping.
        std::fs::write(ws.join("dmuxai.toml"), b"bad\x1b = = 1\n").unwrap();

        let env = RuntimeEnv {
            dmuxai_state_dir: Some(dir.path().join("state")),
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_user_config_and_env(&ws, None, &env);
        assert!(
            !settings.config_warnings.is_empty(),
            "invalid config should produce a warning"
        );
        for warning in &settings.config_warnings {
            assert!(
                !warning.chars().any(char::is_control),
                "warning contains a control character: {warning:?}"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn tc_2245_user_memory_vault_symlink_in_workspace_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let outside = dir.path().join("outside_vault");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let linked = ws.join("linked_vault");
        std::os::unix::fs::symlink(&outside, &linked).unwrap();

        let user_toml = dir.path().join("user.toml");
        std::fs::write(
            &user_toml,
            format!(
                "[memory]\nvault = \"{}\"\n",
                linked.display().to_string().replace('\\', "/")
            ),
        )
        .unwrap();

        let env = RuntimeEnv {
            dmuxai_state_dir: Some(dir.path().join("state")),
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_user_config_and_env(&ws, Some(&user_toml), &env);
        assert_eq!(settings.memory, None);
        assert!(
            settings
                .config_warnings
                .iter()
                .any(|w| w.contains("vault resolves outside the workspace")),
            "warnings: {:?}",
            settings.config_warnings
        );
    }

    #[test]
    fn tc_2246_workspace_memory_vault_inside_ok() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let vault = ws.join("vault");
        std::fs::create_dir_all(&vault).unwrap();

        let user_toml = dir.path().join("user.toml");
        std::fs::write(
            &user_toml,
            format!(
                "[memory]\nvault = \"{}\"\n",
                vault.display().to_string().replace('\\', "/")
            ),
        )
        .unwrap();

        let env = RuntimeEnv {
            dmuxai_state_dir: Some(dir.path().join("state")),
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_user_config_and_env(&ws, Some(&user_toml), &env);
        assert!(
            settings.memory.is_some(),
            "warnings: {:?}",
            settings.config_warnings
        );
    }

    #[test]
    #[cfg(unix)]
    fn tc_2247_relative_memory_root_symlink_escape_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let vault = ws.join("vault");
        let outside = dir.path().join("outside_notes");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        std::os::unix::fs::symlink(&outside, vault.join("notes")).unwrap();

        let user_toml = dir.path().join("user.toml");
        std::fs::write(
            &user_toml,
            format!(
                "[memory]\nvault = \"{}\"\ndir = \"notes\"\n",
                vault.display().to_string().replace('\\', "/")
            ),
        )
        .unwrap();

        let env = RuntimeEnv {
            dmuxai_state_dir: Some(dir.path().join("state")),
            ..RuntimeEnv::default()
        };

        let settings = DmuxSettings::load_with_user_config_and_env(&ws, Some(&user_toml), &env);
        assert_eq!(settings.memory, None);
    }
}
