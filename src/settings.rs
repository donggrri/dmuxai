use std::path::Path;

use serde::Deserialize;

/// Workspace-local and environment overrides for DmuxAI behavior.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DmuxSettings {
    pub cursor_windows_unsandboxed: bool,
    pub routing_default_agent: Option<String>,
    pub routing_cursor_models: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DmuxToml {
    cursor: Option<CursorToml>,
    routing: Option<RoutingToml>,
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
        let mut settings = Self::default();
        if env_flag("DMUXAI_CURSOR_WINDOWS_UNSANDBOXED") {
            settings.cursor_windows_unsandboxed = true;
        }
        let config_path = workspace.join("dmuxai.toml");
        if let Ok(contents) = std::fs::read_to_string(&config_path)
            && let Ok(parsed) = toml::from_str::<DmuxToml>(&contents)
        {
            if parsed
                .cursor
                .and_then(|cursor| cursor.windows_unsandboxed)
                .unwrap_or(false)
            {
                settings.cursor_windows_unsandboxed = true;
            }
            if let Some(routing) = parsed.routing {
                settings.routing_default_agent = routing.default_agent;
                settings.routing_cursor_models = routing.cursor_models.unwrap_or_default();
            }
        }
        settings
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
