use std::{ffi::OsString, io, path::PathBuf};

use tokio::process::Command;

/// FR-1010: child agent/runner processes must not inherit the dashboard token.
pub fn strip_dashboard_env(cmd: &mut std::process::Command) {
    cmd.env_remove("DMUXAI_WEB_TOKEN");
}

/// Resolves a provider executable without invoking a shell.
///
/// The environment override is shared across platforms. Only the fallback
/// executable name differs because Windows commonly exposes an `.exe` shim.
pub fn resolve_program(override_variable: &str, windows_name: &str, unix_name: &str) -> PathBuf {
    let override_value = std::env::var_os(override_variable);
    resolve_program_value(override_value, windows_name, unix_name)
}

fn resolve_program_value(
    override_value: Option<OsString>,
    windows_name: &str,
    unix_name: &str,
) -> PathBuf {
    override_value.map(PathBuf::from).unwrap_or_else(|| {
        #[cfg(windows)]
        let default_name = {
            let _ = unix_name;
            windows_name
        };

        #[cfg(not(windows))]
        let default_name = {
            let _ = windows_name;
            unix_name
        };

        PathBuf::from(default_name)
    })
}

/// Creates a headless Tokio process command for the current OS.
pub fn headless_command(program: PathBuf) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        let mut command = Command::new(program);
        // CREATE_NO_WINDOW: do not flash a second console when DmuxAI launches
        // a console-subsystem provider executable on Windows.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
        strip_dashboard_env(command.as_std_mut());
        command
    }

    #[cfg(not(windows))]
    {
        let mut command = Command::new(program);
        strip_dashboard_env(command.as_std_mut());
        command
    }
}

/// Builds the Cursor Agent command without routing prompts through a shell.
///
/// Cursor's Windows installer exposes PowerShell/CMD wrappers around a bundled
/// Node runtime. Launching that Node executable directly keeps cancellation
/// attached to the real provider process and avoids shell quoting differences.
pub fn cursor_command() -> io::Result<Command> {
    if let Some(program) = std::env::var_os("DMUXAI_CURSOR_BIN") {
        return Ok(headless_command(PathBuf::from(program)));
    }

    #[cfg(windows)]
    {
        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is not set"))?;
        let install_root = PathBuf::from(local_app_data).join("cursor-agent");
        let version_root = install_root.join("versions");
        let version = newest_cursor_version(&version_root)?;
        let node = version.join("node.exe");
        let entrypoint = version.join("index.js");
        let mut command = headless_command(node);
        command.arg(entrypoint);
        Ok(command)
    }

    #[cfg(not(windows))]
    {
        Ok(headless_command(PathBuf::from("cursor-agent")))
    }
}

#[cfg(windows)]
fn newest_cursor_version(version_root: &std::path::Path) -> io::Result<PathBuf> {
    let mut candidates = std::fs::read_dir(version_root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir() && path.join("node.exe").is_file() && path.join("index.js").is_file()
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    candidates.into_iter().next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no usable Cursor Agent version found in {}",
                version_root.display()
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_program_override_is_platform_independent() {
        let path = resolve_program_value(
            Some(OsString::from("/custom/tools/codex-test")),
            "codex.exe",
            "codex",
        );

        assert_eq!(path, PathBuf::from("/custom/tools/codex-test"));
    }

    #[test]
    fn default_program_matches_the_target_os() {
        let path = resolve_program_value(None, "codex.exe", "codex");

        #[cfg(windows)]
        assert_eq!(path, PathBuf::from("codex.exe"));

        #[cfg(not(windows))]
        assert_eq!(path, PathBuf::from("codex"));
    }
}
