use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::activity::UiSettings;
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
    /// `[[agents.custom]]` headless agents (installed into the registry at startup).
    pub custom_agents: Vec<crate::custom_agent::CustomAgentSpec>,
    /// Activity rendering settings (tool calls and model thinking).
    pub ui: UiSettings,
    /// Web dashboard theme settings from `[web]`.
    pub web_theme: WebThemeSettings,
    /// Problems found while reading dmuxai.toml, shown at startup.
    pub config_warnings: Vec<String>,
    /// Resolved `[memory]` (SPEC-obsidian-memory FR-720); `None` = disabled.
    pub memory: Option<crate::memory::MemoryConfig>,
    /// Raw `[memory]` keys merged per key (workspace wins), resolved in `load`.
    memory_raw: MemoryToml,
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
    memory: Option<MemoryToml>,
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
    #[serde(default)]
    custom: Vec<crate::custom_agent::CustomAgentSpec>,
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
        let mut settings = Self::default();
        // Custom agent names seen in *either* config, so an id defined only in
        // the user config is not reported as unknown after the workspace config
        // replaces `[[agents.custom]]`.
        let mut known_custom: Vec<String> = Vec::new();
        if env_flag("DMUXAI_CURSOR_WINDOWS_UNSANDBOXED") {
            settings.cursor_windows_unsandboxed = true;
        }

        if let Some(user_path) = user_config_path
            && let Ok(contents) = std::fs::read_to_string(user_path)
        {
            match toml::from_str::<DmuxToml>(&contents) {
                Ok(parsed) => settings.apply_toml(parsed, &mut known_custom),
                Err(error) => settings.config_warnings.push(format!(
                    "user config {} ignored: {error}",
                    user_path.display()
                )),
            }
        }

        let config_path = workspace.join("dmuxai.toml");
        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            match toml::from_str::<DmuxToml>(&contents) {
                Ok(parsed) => {
                    settings.apply_toml(parsed, &mut known_custom);
                }
                Err(error) => {
                    settings
                        .config_warnings
                        .push(format!("dmuxai.toml ignored: {error}"));
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

        settings.resolve_memory(workspace);
        settings.dedupe_warnings();
        settings
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

    fn apply_toml(&mut self, parsed: DmuxToml, known_custom: &mut Vec<String>) {
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
            for spec in &agents.custom {
                known_custom.push(spec.name.clone());
            }
            self.custom_agents = agents.custom;
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
            custom_agents: Vec::new(),
            ui: UiSettings::default(),
            web_theme: WebThemeSettings::default(),
            config_warnings: Vec::new(),
            memory: None,
            memory_raw: MemoryToml::default(),
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

fn env_flag(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

fn default_user_config_path() -> Option<PathBuf> {
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
        let temp_base =
            std::env::temp_dir().join(format!("dmuxai-web-theme-{}", std::process::id()));
        let user_dir = temp_base.join("user");
        let ws_dir = temp_base.join("workspace");
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
        let empty_dir = temp_base.join("empty");
        std::fs::create_dir_all(&empty_dir).unwrap();
        let defaults = DmuxSettings::load_with_user_config(&empty_dir, None);
        assert_eq!(defaults.web_theme.theme, "system");
        assert_eq!(defaults.web_theme.accent, None);

        // Invalid values fall back with warnings.
        let bad_dir = temp_base.join("bad");
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
        let norm_dir = temp_base.join("norm");
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
        let named_dir = temp_base.join("named");
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

        std::fs::remove_dir_all(&temp_base).ok();
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
