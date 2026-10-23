use std::{io, path::PathBuf};

use tokio::process::Command;

/// DmuxAI-internal environment variables that must be removed from every
/// agent or runner child process.
///
/// Child environment policy decisions for `DMUXAI_*` variables:
///
/// **Removed (internal / sensitive):**
/// - `DMUXAI_WEB_TOKEN`: Secret bearer token for the DmuxAI web dashboard API
///   (FR-1010). Untrusted agent child processes must never inherit this secret.
/// - `DMUXAI_MCP_CONFIG`: Internal path to a per-run MCP configuration JSON file
///   (FR-1663). Built-in runners never pass MCP configuration via environment
///   variables; custom agent runners set this per-run only after this policy
///   has run.
/// - `DMUXAI_CONFIG`: Path to the user configuration file (`dmuxai.toml`)
///   set via `--config` or the environment. Child processes must not inherit this
///   override, ensuring child runs (such as MCP servers or nested tools) use
///   workspace or default configuration isolation.
///
/// **Preserved (needed by child processes or user preferences):**
/// - `DMUXAI_STATE_DIR`: Root directory for DmuxAI state. Preserved so that
///   MCP servers started by agents (e.g., via `dmuxai mcp`) or CLI tools
///   can locate the active database and state storage.
/// - `DMUXAI_WORKSPACE`: Canonical workspace directory. Preserved so child
///   MCP servers and subcommands resolve the expected workspace root.
/// - `DMUXAI_*_BIN` (`DMUXAI_CURSOR_BIN`, `DMUXAI_AGY_BIN`, `DMUXAI_CODEX_BIN`,
///   `DMUXAI_OPENCODE_BIN`, `DMUXAI_GROK_BIN`): User- or environment-configured
///   executable paths for runners. Preserved so nested invocations or tools find
///   the configured executables.
/// - `DMUXAI_REDUCED_MOTION`: User accessibility preference for reduced motion.
///   Preserved because it is a non-sensitive user preference.
/// - `DMUXAI_FORCE_COLOR`: Terminal color output override. Preserved as a
///   non-sensitive display preference.
///
/// **Standard system variables:**
/// - `XDG_*` (such as `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `XDG_DATA_HOME`)
///   and `HOME`: The policy must NEVER modify or remove these variables.
pub const INTERNAL_ENV_VARS: &[&str] = &["DMUXAI_WEB_TOKEN", "DMUXAI_MCP_CONFIG", "DMUXAI_CONFIG"];

/// Applies the child process environment policy to a standard process command.
///
/// Strips all DmuxAI-internal environment variables listed in [`INTERNAL_ENV_VARS`].
/// This policy never modifies `XDG_*` or `HOME`.
pub fn apply_child_env_policy(cmd: &mut std::process::Command) {
    for var in INTERNAL_ENV_VARS {
        cmd.env_remove(var);
    }
}

/// Applies the child process environment policy to a Tokio process command.
///
/// Strips all DmuxAI-internal environment variables listed in [`INTERNAL_ENV_VARS`].
/// This policy never modifies `XDG_*` or `HOME`.
pub fn apply_child_env_policy_tokio(cmd: &mut Command) {
    apply_child_env_policy(cmd.as_std_mut());
}

/// FR-1010: child agent/runner processes must not inherit the dashboard token.
///
/// Kept for backwards compatibility; delegates to [`apply_child_env_policy`] to
/// enforce the complete child environment policy.
pub fn strip_dashboard_env(cmd: &mut std::process::Command) {
    apply_child_env_policy(cmd);
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
    apply_child_env_policy_tokio(&mut command);
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

    #[test]
    fn child_env_policy_removes_internal_env_vars() {
        let mut std_cmd = std::process::Command::new("true");
        apply_child_env_policy(&mut std_cmd);
        let removed: Vec<_> = std_cmd
            .get_envs()
            .filter_map(|(k, v)| {
                if v.is_none() {
                    Some(k.to_str().unwrap())
                } else {
                    None
                }
            })
            .collect();
        for var in INTERNAL_ENV_VARS {
            assert!(
                removed.contains(var),
                "expected {var} to be removed in std::process::Command"
            );
        }

        let mut tokio_cmd = Command::new("true");
        apply_child_env_policy_tokio(&mut tokio_cmd);
        let tokio_removed: Vec<_> = tokio_cmd
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| {
                if v.is_none() {
                    Some(k.to_str().unwrap())
                } else {
                    None
                }
            })
            .collect();
        for var in INTERNAL_ENV_VARS {
            assert!(
                tokio_removed.contains(var),
                "expected {var} to be removed in tokio::process::Command"
            );
        }
    }

    #[test]
    fn child_env_policy_preserves_external_and_non_internal_env() {
        let mut cmd = std::process::Command::new("true");
        cmd.env("HOME", "/home/test");
        cmd.env("XDG_CONFIG_HOME", "/home/test/.config");
        cmd.env("DMUXAI_STATE_DIR", "/tmp/state");
        cmd.env("DMUXAI_WORKSPACE", "/tmp/ws");
        cmd.env("DMUXAI_CURSOR_BIN", "/bin/cursor");
        cmd.env("DMUXAI_REDUCED_MOTION", "1");
        apply_child_env_policy(&mut cmd);

        let env_map: std::collections::HashMap<_, _> = cmd
            .get_envs()
            .filter_map(|(k, v)| v.map(|v| (k.to_str().unwrap(), v.to_str().unwrap())))
            .collect();

        assert_eq!(env_map.get("HOME"), Some(&"/home/test"));
        assert_eq!(env_map.get("XDG_CONFIG_HOME"), Some(&"/home/test/.config"));
        assert_eq!(env_map.get("DMUXAI_STATE_DIR"), Some(&"/tmp/state"));
        assert_eq!(env_map.get("DMUXAI_WORKSPACE"), Some(&"/tmp/ws"));
        assert_eq!(env_map.get("DMUXAI_CURSOR_BIN"), Some(&"/bin/cursor"));
        assert_eq!(env_map.get("DMUXAI_REDUCED_MOTION"), Some(&"1"));
    }

    #[test]
    fn headless_command_applies_child_env_policy() {
        let cmd = headless_command(PathBuf::from("test-agent"));
        let removed: Vec<_> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| {
                if v.is_none() {
                    Some(k.to_str().unwrap())
                } else {
                    None
                }
            })
            .collect();
        for var in INTERNAL_ENV_VARS {
            assert!(
                removed.contains(var),
                "expected headless_command to remove {var}"
            );
        }
    }
}
