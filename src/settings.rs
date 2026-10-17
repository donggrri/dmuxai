use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::activity::UiSettings;
use crate::mcp::config::ToolNames;
use crate::mcp::{McpServerSpec, ServerKind};
use crate::notify::NotifySettings;
use crate::theme::{Theme, ThemeToml};

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

/// Workspace-local and environment overrides for DmuxAI behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmuxSettings {
    pub cursor_windows_unsandboxed: bool,
    pub routing_default_agent: Option<String>,
    pub routing_cursor_models: Vec<String>,
    pub graph_enabled: bool,
    pub graph_max_parallel: usize,
    pub graph_max_retries: u32,
    pub graph_allowed_runners: Option<Vec<String>>,
    pub theme: Theme,
    pub theme_force_color: Option<bool>,
    pub reduced_motion: bool,
    /// `[[agents.custom]]` headless agents (installed into the registry at startup).
    pub custom_agents: Vec<crate::custom_agent::CustomAgentSpec>,
    /// `[agents] builtin_opencode` (default true). `false` hides the built-in
    /// opencode preset (SPEC-first-run FR-811).
    pub builtin_opencode: bool,
    /// `[[mcp.servers]]` merged from user and repo configs by name (repo wins).
    pub mcp_servers: Vec<crate::mcp::McpServerSpec>,
    /// `[todo] jira_base_url` (user or repo, repo wins) used to derive links
    /// for to-dos that carry a Jira issue key (FR-564).
    pub todo_jira_base_url: Option<String>,
    /// Activity rendering settings (tool calls and model thinking).
    pub ui: UiSettings,
    /// Web dashboard theme settings from `[web]`.
    pub web_theme: WebThemeSettings,
    /// Event delivery settings from `[notify]` (FR-770..FR-772).
    pub notify: NotifySettings,
    /// Problems found while reading dmuxai.toml, shown at startup.
    pub config_warnings: Vec<String>,
    /// Resolved `[memory]` (SPEC-obsidian-memory FR-720); `None` = disabled.
    pub memory: Option<crate::memory::MemoryConfig>,
    /// Raw `[memory]` keys merged per key (workspace wins), resolved in `load`.
    memory_raw: MemoryToml,
    /// True when any configured `[[mcp.servers]]` entry (enabled or disabled)
    /// is named `dmuxai-memory`; suppresses the built-in entry (FR-733).
    memory_server_configured: bool,
}

/// Name of the built-in shared MCP entry added when `[memory]` is set (FR-733).
pub const BUILTIN_MEMORY_SERVER_NAME: &str = "dmuxai-memory";

/// Builds the built-in `dmuxai-memory` stdio entry (FR-733): the DmuxAI
/// executable (absolute path) running `mcp-memory --workspace <root>`. The vault
/// path and any secrets are never placed in the entry; the subcommand reads
/// `[memory]` from the workspace. Errors carry the config warning text.
pub fn builtin_memory_mcp_server(
    exe: Option<&Path>,
    workspace_root: &Path,
) -> Result<McpServerSpec, String> {
    let exe = match exe {
        Some(path) if path.is_absolute() => path,
        _ => {
            return Err(format!(
                "mcp server {BUILTIN_MEMORY_SERVER_NAME} not added: the DmuxAI executable path could not be resolved"
            ));
        }
    };
    Ok(McpServerSpec {
        name: BUILTIN_MEMORY_SERVER_NAME.to_owned(),
        command: Some(exe.to_string_lossy().into_owned()),
        args: vec![
            "mcp-memory".to_owned(),
            "--workspace".to_owned(),
            workspace_root.to_string_lossy().into_owned(),
        ],
        env: BTreeMap::new(),
        url: None,
        headers: BTreeMap::new(),
        kind: ServerKind::Generic,
        runners: None,
        enabled: true,
        tools: ToolNames::default(),
    })
}

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
}

#[derive(Debug, Default, Deserialize)]
struct TodoToml {
    jira_base_url: Option<String>,
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
    /// that only sets `builtin_opencode` keeps the other config's agents.
    custom: Option<Vec<crate::custom_agent::CustomAgentSpec>>,
    builtin_opencode: Option<bool>,
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
    allowed_runners: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct CursorToml {
    windows_unsandboxed: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct RoutingToml {
    default_agent: Option<String>,
    cursor_models: Option<Vec<String>>,
}

impl DmuxSettings {
    pub fn load(workspace: &Path) -> Self {
        Self::load_with_user_config(workspace, default_user_config_path().as_deref())
    }

    pub fn load_with_user_config(workspace: &Path, user_config_path: Option<&Path>) -> Self {
        let exe = std::env::current_exe().ok();
        Self::load_with_exe(workspace, user_config_path, exe.as_deref())
    }

    /// Like [`Self::load_with_user_config`] with the DmuxAI executable injected
    /// for the built-in `dmuxai-memory` entry (FR-733), so tests are deterministic.
    /// The opencode executable is looked up in the environment (FR-810).
    pub(crate) fn load_with_exe(
        workspace: &Path,
        user_config_path: Option<&Path>,
        exe: Option<&Path>,
    ) -> Self {
        Self::load_with_opencode(workspace, user_config_path, exe, find_opencode_bin())
    }

    /// Loads settings with the opencode executable injected (`None` = not
    /// installed), so the preset decision is deterministic in tests (FR-810).
    pub(crate) fn load_with_opencode(
        workspace: &Path,
        user_config_path: Option<&Path>,
        exe: Option<&Path>,
        opencode_bin: Option<PathBuf>,
    ) -> Self {
        let mut settings = Self::default();
        if env_flag("DMUXAI_CURSOR_WINDOWS_UNSANDBOXED") {
            settings.cursor_windows_unsandboxed = true;
        }

        let mut configs: Vec<DmuxToml> = Vec::new();
        if let Some(user_path) = user_config_path
            && let Ok(contents) = std::fs::read_to_string(user_path)
        {
            match toml::from_str::<DmuxToml>(&contents) {
                Ok(parsed) => configs.push(parsed),
                Err(error) => settings.config_warnings.push(format!(
                    "user config {} ignored: {error}",
                    user_path.display()
                )),
            }
        }

        let config_path = workspace.join("dmuxai.toml");
        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            match toml::from_str::<DmuxToml>(&contents) {
                Ok(parsed) => configs.push(parsed),
                Err(error) => {
                    settings
                        .config_warnings
                        .push(format!("dmuxai.toml ignored: {error}"));
                }
            }
        }

        // FR-810/FR-811: the preset is registered only when `builtin_opencode` is
        // not false in either config, no config names its own `opencode` agent,
        // and the executable exists. This is decided before `allowed_runners` is
        // validated, so `allowed_runners = ["opencode"]` is accepted.
        let agents = || configs.iter().filter_map(|parsed| parsed.agents.as_ref());
        settings.builtin_opencode = agents()
            .filter_map(|agents| agents.builtin_opencode)
            .next_back()
            .unwrap_or(true);
        let names_opencode = agents()
            .flat_map(|agents| agents.custom.iter().flatten())
            .any(|spec| spec.name.eq_ignore_ascii_case("opencode"));
        let preset_bin = opencode_bin.filter(|_| settings.builtin_opencode && !names_opencode);

        // Custom agent names seen in *either* config, so an id defined only in
        // the user config is not reported as unknown after the workspace config
        // replaces `[[agents.custom]]`.
        let mut known_custom: Vec<String> = Vec::new();
        if preset_bin.is_some() {
            known_custom.push("opencode".to_owned());
        }
        for parsed in configs {
            settings.apply_toml(parsed, &mut known_custom, preset_bin.is_some());
        }
        if let Some(program) = preset_bin
            && !settings
                .custom_agents
                .iter()
                .any(|spec| spec.name.eq_ignore_ascii_case("opencode"))
        {
            // Appended last so existing custom agent indices do not move.
            settings
                .custom_agents
                .push(crate::custom_agent::opencode_preset(
                    &program.to_string_lossy(),
                ));
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

        settings.resolve_memory(workspace);
        settings.add_builtin_memory_server(workspace, exe);
        if env_flag("DMUXAI_REDUCED_MOTION") {
            settings.reduced_motion = true;
            settings.theme.reduced_motion = true;
        }
        settings.dedupe_warnings();
        settings
    }

    /// FR-733: append the built-in `dmuxai-memory` shared entry when memory is
    /// enabled and no configured entry (enabled or disabled) claims the name.
    fn add_builtin_memory_server(&mut self, workspace: &Path, exe: Option<&Path>) {
        if self.memory.is_none() || self.memory_server_configured {
            return;
        }
        match builtin_memory_mcp_server(exe, workspace) {
            Ok(spec) => self.mcp_servers.push(spec),
            Err(warning) => self.config_warnings.push(warning),
        }
    }

    /// FR-720: turn the merged `[memory]` keys into a [`MemoryConfig`].
    fn resolve_memory(&mut self, workspace: &Path) {
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
        let vault_path = expand_home(vault);
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
        let mut seen = std::collections::HashSet::new();
        self.config_warnings.retain(|w| seen.insert(w.clone()));
    }

    fn apply_toml(
        &mut self,
        parsed: DmuxToml,
        known_custom: &mut Vec<String>,
        preset_pending: bool,
    ) {
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
        if let Some(agents) = parsed.agents
            && let Some(custom) = agents.custom
        {
            for spec in &custom {
                known_custom.push(spec.name.clone());
            }
            self.custom_agents = custom;
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
        if parsed
            .cursor
            .and_then(|cursor| cursor.windows_unsandboxed)
            .unwrap_or(false)
        {
            self.cursor_windows_unsandboxed = true;
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
            if let Some(enabled) = graph.enabled {
                self.graph_enabled = enabled;
            }
            if let Some(max_parallel) = graph.max_parallel {
                self.graph_max_parallel = max_parallel;
            }
            if let Some(max_retries) = graph.max_retries {
                self.graph_max_retries = max_retries;
            }
            if let Some(runners) = graph.allowed_runners {
                let mut valid = Vec::new();
                for runner in runners {
                    let lower = runner.trim().to_ascii_lowercase();
                    if lower.is_empty() {
                        continue;
                    }
                    if matches!(lower.as_str(), "agy" | "codex" | "cursor")
                        || (preset_pending && lower == "opencode")
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
            if spec.name == BUILTIN_MEMORY_SERVER_NAME {
                self.memory_server_configured = true;
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
    /// Whether `runner` may take graph nodes (SPEC-first-run FR-813). An unset
    /// `allowed_runners` admits every runner except the built-in opencode
    /// preset; a listed runner, including the preset, is admitted.
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
}

impl Default for DmuxSettings {
    fn default() -> Self {
        Self {
            cursor_windows_unsandboxed: false,
            routing_default_agent: None,
            routing_cursor_models: Vec::new(),
            graph_enabled: true,
            graph_max_parallel: 2,
            graph_max_retries: 2,
            graph_allowed_runners: None,
            theme: Theme::default(),
            theme_force_color: None,
            reduced_motion: false,
            custom_agents: Vec::new(),
            builtin_opencode: true,
            mcp_servers: Vec::new(),
            todo_jira_base_url: None,
            ui: UiSettings::default(),
            web_theme: WebThemeSettings::default(),
            notify: NotifySettings::default(),
            config_warnings: Vec::new(),
            memory: None,
            memory_raw: MemoryToml::default(),
            memory_server_configured: false,
        }
    }
}

/// Expand a leading `~/` (or `~\\`) with `HOME` / `USERPROFILE`.
pub fn expand_home(path: &str) -> PathBuf {
    let rest = path
        .strip_prefix("~/")
        .or_else(|| path.strip_prefix("~\\"))
        .or_else(|| (path == "~").then_some(""));
    if let Some(rest) = rest {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .or_else(|| std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()));
        if let Some(home) = home {
            let mut p = PathBuf::from(home);
            if !rest.is_empty() {
                p.push(rest);
            }
            return p;
        }
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

/// Finds the opencode executable (SPEC-first-run FR-810). `DMUXAI_OPENCODE_BIN`
/// is authoritative when set: it must name an existing file. Otherwise the first
/// `opencode` file on `PATH` is used.
pub fn find_opencode_bin() -> Option<PathBuf> {
    match std::env::var_os("DMUXAI_OPENCODE_BIN").filter(|value| !value.is_empty()) {
        Some(raw) => {
            let path = PathBuf::from(raw);
            path.is_file().then_some(path)
        }
        None => find_on_path("opencode"),
    }
}

/// Searches `PATH` for the first file called `name` (`name.exe` or `name.cmd`
/// on Windows). No shell is involved.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let file_names: Vec<String> = if cfg!(windows) {
        vec![
            format!("{name}.exe"),
            format!("{name}.cmd"),
            format!("{name}.bat"),
            name.to_owned(),
        ]
    } else {
        vec![name.to_owned()]
    };
    std::env::split_paths(&path_var)
        .flat_map(|dir| {
            file_names
                .iter()
                .map(move |file| dir.join(file))
                .collect::<Vec<_>>()
        })
        .find(|candidate| is_executable_file(candidate))
}

/// A regular file the current user may execute (on Unix, any execute bit).
fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

fn env_flag(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

pub fn default_user_config_path() -> Option<PathBuf> {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        let base_path = PathBuf::from(base);
        if !base_path.as_os_str().is_empty() {
            return Some(base_path.join("dmuxai").join("dmuxai.toml"));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home_path = PathBuf::from(home);
        if !home_path.as_os_str().is_empty() {
            return Some(home_path.join(".config").join("dmuxai").join("dmuxai.toml"));
        }
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

        let settings = DmuxSettings::load_with_user_config(&ws_dir, Some(&user_toml));

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

    #[test]
    fn graph_enabled_by_default_and_can_be_disabled() {
        let dir =
            std::env::temp_dir().join(format!("dmuxai-graph-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let default_settings = DmuxSettings::load(&dir);
        assert!(default_settings.graph_enabled);

        std::fs::write(dir.join("dmuxai.toml"), "[graph]\nenabled = false\n").unwrap();
        let disabled_settings = DmuxSettings::load(&dir);
        assert!(!disabled_settings.graph_enabled);
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

        let settings = DmuxSettings::load_with_user_config(&ws_dir, Some(&user_toml));
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
        let settings = DmuxSettings::load_with_user_config(&ws_dir, Some(&user_toml));
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
        assert_eq!(m.dir, "cursor-workflow/memory");
        assert_eq!(m.root, vault.path().join("cursor-workflow").join("memory"));
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
        assert_eq!(
            m.root,
            ws.path()
                .join("vault")
                .join("cursor-workflow")
                .join("memory")
        );
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

        let settings = DmuxSettings::load_with_user_config(dir.path(), None);
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

    // TC-733 (FR-733): memory on -> exactly one built-in entry, last, with the
    // exe + `mcp-memory --workspace <root>` argv and no vault path or secrets.
    #[test]
    fn builtin_memory_mcp_server_appends_last_entry_when_memory_on() {
        let ws = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        let user = ws.path().join("user.toml");
        std::fs::write(
            &user,
            "[[mcp.servers]]\nname = \"other\"\ncommand = \"other-cmd\"\n",
        )
        .unwrap();
        write_memory_ws(ws.path(), vault.path(), "");

        let exe = exe();
        let s = DmuxSettings::load_with_exe(ws.path(), Some(&user), Some(&exe));
        assert!(s.memory.is_some());
        let builtins: Vec<&McpServerSpec> = s
            .mcp_servers
            .iter()
            .filter(|spec| spec.name == BUILTIN_MEMORY_SERVER_NAME)
            .collect();
        assert_eq!(builtins.len(), 1, "{:?}", s.mcp_servers);
        let spec = builtins[0];
        assert_eq!(
            s.mcp_servers.last().map(|x| x.name.as_str()),
            Some("dmuxai-memory")
        );
        assert_eq!(
            spec.command.as_deref(),
            Some(exe.to_string_lossy().as_ref())
        );
        assert_eq!(
            spec.args,
            vec![
                "mcp-memory".to_owned(),
                "--workspace".to_owned(),
                ws.path().to_string_lossy().into_owned(),
            ]
        );
        assert!(spec.env.is_empty() && spec.headers.is_empty() && spec.url.is_none());
        assert_eq!(spec.kind, ServerKind::Generic);
        assert!(spec.runners.is_none() && spec.enabled);
        let vault_str = vault.path().to_string_lossy().into_owned();
        assert!(spec.args.iter().all(|arg| !arg.contains(&vault_str)));
        assert!(s.config_warnings.is_empty(), "{:?}", s.config_warnings);
    }

    // TC-733 (FR-733): memory off -> no built-in entry, no warning.
    #[test]
    fn builtin_memory_mcp_server_absent_when_memory_off() {
        let ws = tempfile::tempdir().unwrap();
        let exe = exe();
        let s = DmuxSettings::load_with_exe(ws.path(), None, Some(&exe));
        assert!(s.memory.is_none());
        assert!(
            s.mcp_servers
                .iter()
                .all(|x| x.name != BUILTIN_MEMORY_SERVER_NAME)
        );
        assert!(s.config_warnings.is_empty(), "{:?}", s.config_warnings);
    }

    // TC-733 (FR-733): a repo entry with the same name replaces the built-in one.
    #[test]
    fn builtin_memory_mcp_server_repo_entry_replaces_builtin() {
        let ws = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        write_memory_ws(
            ws.path(),
            vault.path(),
            "\n[[mcp.servers]]\nname = \"dmuxai-memory\"\ncommand = \"custom-memory\"\n",
        );
        let exe = exe();
        let s = DmuxSettings::load_with_exe(ws.path(), None, Some(&exe));
        let builtins: Vec<&McpServerSpec> = s
            .mcp_servers
            .iter()
            .filter(|spec| spec.name == BUILTIN_MEMORY_SERVER_NAME)
            .collect();
        assert_eq!(builtins.len(), 1, "{:?}", s.mcp_servers);
        assert_eq!(builtins[0].command.as_deref(), Some("custom-memory"));
        assert!(builtins[0].args.is_empty());
    }

    // TC-733 (FR-733): `enabled = false` on the name removes it and it is not
    // re-added, even though memory is on.
    #[test]
    fn builtin_memory_mcp_server_disabled_name_is_not_readded() {
        let ws = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        write_memory_ws(
            ws.path(),
            vault.path(),
            "\n[[mcp.servers]]\nname = \"dmuxai-memory\"\nenabled = false\n",
        );
        let exe = exe();
        let s = DmuxSettings::load_with_exe(ws.path(), None, Some(&exe));
        assert!(s.memory.is_some());
        assert!(
            s.mcp_servers
                .iter()
                .all(|x| x.name != BUILTIN_MEMORY_SERVER_NAME)
        );
        assert!(s.config_warnings.is_empty(), "{:?}", s.config_warnings);
    }

    // TC-733 (FR-733): an unresolvable (absent or relative) exe -> one warning,
    // no entry.
    #[test]
    fn builtin_memory_mcp_server_unresolvable_exe_warns() {
        let ws = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        write_memory_ws(ws.path(), vault.path(), "");

        let s = DmuxSettings::load_with_exe(ws.path(), None, None);
        assert!(
            s.mcp_servers
                .iter()
                .all(|x| x.name != BUILTIN_MEMORY_SERVER_NAME)
        );
        let warnings: Vec<&String> = s
            .config_warnings
            .iter()
            .filter(|w| w.contains(BUILTIN_MEMORY_SERVER_NAME))
            .collect();
        assert_eq!(warnings.len(), 1, "{:?}", s.config_warnings);

        let relative = Path::new("dmuxai");
        assert!(builtin_memory_mcp_server(Some(relative), ws.path()).is_err());
        assert!(builtin_memory_mcp_server(None, ws.path()).is_err());
    }

    // TC-733 (FR-733): user disables the name, repo re-enables it with its own
    // entry -> the repo entry is the only one and no built-in duplicate is added.
    #[test]
    fn builtin_memory_mcp_server_user_disabled_repo_reenables_with_own_entry() {
        let ws = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        let user = ws.path().join("user.toml");
        std::fs::write(
            &user,
            "[[mcp.servers]]\nname = \"dmuxai-memory\"\nenabled = false\n",
        )
        .unwrap();
        write_memory_ws(
            ws.path(),
            vault.path(),
            "\n[[mcp.servers]]\nname = \"dmuxai-memory\"\ncommand = \"repo-memory\"\n",
        );
        let exe = exe();
        let s = DmuxSettings::load_with_exe(ws.path(), Some(&user), Some(&exe));
        let builtins: Vec<&McpServerSpec> = s
            .mcp_servers
            .iter()
            .filter(|spec| spec.name == BUILTIN_MEMORY_SERVER_NAME)
            .collect();
        assert_eq!(builtins.len(), 1, "{:?}", s.mcp_servers);
        assert_eq!(builtins[0].command.as_deref(), Some("repo-memory"));
        assert!(builtins[0].args.is_empty(), "repo entry, not built-in");
        assert!(s.config_warnings.is_empty(), "{:?}", s.config_warnings);
    }

    // TC-733 (FR-733): user custom entry, repo `enabled = false` on the name ->
    // absent, and the built-in entry is not re-added.
    #[test]
    fn builtin_memory_mcp_server_user_custom_repo_disabled_stays_absent() {
        let ws = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        let user = ws.path().join("user.toml");
        std::fs::write(
            &user,
            "[[mcp.servers]]\nname = \"dmuxai-memory\"\ncommand = \"user-memory\"\n",
        )
        .unwrap();
        write_memory_ws(
            ws.path(),
            vault.path(),
            "\n[[mcp.servers]]\nname = \"dmuxai-memory\"\nenabled = false\n",
        );
        let exe = exe();
        let s = DmuxSettings::load_with_exe(ws.path(), Some(&user), Some(&exe));
        assert!(s.memory.is_some());
        assert!(
            s.mcp_servers
                .iter()
                .all(|spec| spec.name != BUILTIN_MEMORY_SERVER_NAME),
            "{:?}",
            s.mcp_servers
        );
        assert!(s.config_warnings.is_empty(), "{:?}", s.config_warnings);
    }

    // TC-733 (FR-733): a configured `dmuxai-memory` entry that fails validation
    // warns and suppresses the built-in entry (README: a configured entry with
    // that name wins even when invalid).
    #[test]
    fn builtin_memory_mcp_server_invalid_configured_entry_warns_and_suppresses() {
        let ws = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        let exe = exe();
        let cases = [
            (
                "\n[[mcp.servers]]\nname = \"dmuxai-memory\"\ncommand = \"a\"\nurl = \"https://mcp.example.invalid/mcp\"\n",
                "sets both command and url",
            ),
            (
                "\n[[mcp.servers]]\nname = \"dmuxai-memory\"\n",
                "needs either command or url",
            ),
        ];
        for (extra, expected) in cases {
            write_memory_ws(ws.path(), vault.path(), extra);
            let s = DmuxSettings::load_with_exe(ws.path(), None, Some(&exe));
            assert!(s.memory.is_some());
            assert!(
                s.mcp_servers
                    .iter()
                    .all(|spec| spec.name != BUILTIN_MEMORY_SERVER_NAME),
                "built-in must not be added: {:?}",
                s.mcp_servers
            );
            assert!(
                s.config_warnings
                    .iter()
                    .any(|w| w.contains("dmuxai-memory") && w.contains(expected)),
                "{:?}",
                s.config_warnings
            );
        }
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

        // Installed, no config: the preset is registered from the template.
        let settings = DmuxSettings::load_with_opencode(&ws, Some(&user), None, Some(bin.clone()));
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
        let missing = DmuxSettings::load_with_opencode(&ws, Some(&user), None, None);
        assert!(missing.custom_agents.is_empty());
        assert!(missing.config_warnings.is_empty());

        // A user entry named opencode wins; the preset is absent.
        std::fs::write(
            &user,
            "[[agents.custom]]\nname = \"opencode\"\ncommand = [\"my-opencode\"]\n",
        )
        .unwrap();
        let user_wins = DmuxSettings::load_with_opencode(&ws, Some(&user), None, Some(bin.clone()));
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
        let disabled = DmuxSettings::load_with_opencode(&ws, Some(&user), None, Some(bin.clone()));
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
        let kept = DmuxSettings::load_with_opencode(&ws, Some(&user), None, Some(bin.clone()));
        let names: Vec<&str> = kept.custom_agents.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["pi", "opencode"]);

        // The preset can be named in allowed_runners without a warning.
        std::fs::write(&user, "").unwrap();
        std::fs::write(
            ws.join("dmuxai.toml"),
            "[graph]\nallowed_runners = [\"opencode\", \"cursor\"]\n",
        )
        .unwrap();
        let listed = DmuxSettings::load_with_opencode(&ws, Some(&user), None, Some(bin.clone()));
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
        let unknown = DmuxSettings::load_with_opencode(&ws, Some(&user), None, None);
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
        let settings = DmuxSettings::load_with_opencode(dir.path(), Some(&user), None, Some(bin));
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
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dmuxai.toml");
        for detected in [false, true] {
            let text = crate::init::starter_config(detected);
            for key in [
                "[theme]",
                "[routing]",
                "[graph]",
                "[cursor]",
                "[agents]",
                "builtin_opencode",
            ] {
                assert!(text.contains(key), "missing {key}");
            }
            std::fs::write(&path, &text).unwrap();
            // No opencode executable here: the example stays a comment.
            let settings = DmuxSettings::load_with_opencode(dir.path(), Some(&path), None, None);
            assert_eq!(settings, DmuxSettings::default(), "detected={detected}");
        }
    }
}
