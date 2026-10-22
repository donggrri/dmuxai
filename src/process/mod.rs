use std::{io, path::PathBuf};

use tokio::process::Command;

/// FR-1010: child agent/runner processes must not inherit the dashboard token.
pub fn strip_dashboard_env(cmd: &mut std::process::Command) {
    cmd.env_remove("DMUXAI_WEB_TOKEN");
}

/// Resolves a provider executable without invoking a shell.
///
/// The environment override is shared across platforms. Only the fallback
/// executable name differs per provider.
pub fn resolve_program(override_variable: &str, default_name: &str) -> PathBuf {
    let override_value = std::env::var_os(override_variable);
    resolve_program_value(override_value, default_name)
}

fn resolve_program_value(
    override_value: Option<std::ffi::OsString>,
    default_name: &str,
) -> PathBuf {
    override_value
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default_name))
}

/// Creates a headless Tokio process command for the current OS.
pub fn headless_command(program: PathBuf) -> Command {
    let mut command = Command::new(program);
    strip_dashboard_env(command.as_std_mut());
    command
}

/// Builds the Cursor Agent command without routing prompts through a shell.
pub fn cursor_command() -> io::Result<Command> {
    if let Some(program) = std::env::var_os("DMUXAI_CURSOR_BIN") {
        return Ok(headless_command(PathBuf::from(program)));
    }

    Ok(headless_command(PathBuf::from("cursor-agent")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_program_override_is_platform_independent() {
        let path = resolve_program_value(
            Some(std::ffi::OsString::from("/custom/tools/codex-test")),
            "codex",
        );

        assert_eq!(path, PathBuf::from("/custom/tools/codex-test"));
    }

    #[test]
    fn default_program_matches_the_target_os() {
        let path = resolve_program_value(None, "codex");

        assert_eq!(path, PathBuf::from("codex"));
    }
}
