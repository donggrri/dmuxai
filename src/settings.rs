use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::activity::UiSettings;
use crate::theme::{Theme, ThemeToml};

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
    /// Problems found while reading dmuxai.toml, shown at startup.
    pub config_warnings: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DmuxToml {
    cursor: Option<CursorToml>,
    routing: Option<RoutingToml>,
    graph: Option<GraphToml>,
    theme: Option<ThemeToml>,
    agents: Option<AgentsToml>,
    ui: Option<UiSettings>,
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
        if env_flag("DMUXAI_CURSOR_WINDOWS_UNSANDBOXED") {
            settings.cursor_windows_unsandboxed = true;
        }

        if let Some(user_path) = user_config_path
            && let Ok(contents) = std::fs::read_to_string(user_path)
        {
            match toml::from_str::<DmuxToml>(&contents) {
                Ok(parsed) => settings.apply_toml(parsed),
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
                    settings.apply_toml(parsed);
                }
                Err(error) => {
                    settings
                        .config_warnings
                        .push(format!("dmuxai.toml ignored: {error}"));
                }
            }
        }

        settings
    }

    fn apply_toml(&mut self, parsed: DmuxToml) {
        if let Some(agents) = parsed.agents {
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
            self.theme.apply_toml(theme);
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
            config_warnings: Vec::new(),
        }
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
