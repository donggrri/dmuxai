use std::ffi::OsString;
use std::path::PathBuf;

/// Environment values that settings loading depends on. Built from the process
/// environment via [`RuntimeEnv::from_process`] or constructed explicitly in
/// tests so settings loading never has to read `std::env` directly.
#[derive(Debug, Clone, Default)]
pub struct RuntimeEnv {
    pub opencode_bin: Option<OsString>,
    pub grok_bin: Option<OsString>,
    pub cursor_bin: Option<OsString>,
    pub agy_bin: Option<OsString>,
    pub codex_bin: Option<OsString>,
    /// Override for the user config file (`DMUXAI_CONFIG` / `--config`).
    pub config_path: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub userprofile: Option<PathBuf>,
    pub path: Option<OsString>,
    pub reduced_motion: Option<OsString>,
}

impl RuntimeEnv {
    pub fn from_process() -> Self {
        Self {
            opencode_bin: std::env::var_os("DMUXAI_OPENCODE_BIN").filter(|v| !v.is_empty()),
            grok_bin: std::env::var_os("DMUXAI_GROK_BIN").filter(|v| !v.is_empty()),
            cursor_bin: std::env::var_os("DMUXAI_CURSOR_BIN").filter(|v| !v.is_empty()),
            agy_bin: std::env::var_os("DMUXAI_AGY_BIN").filter(|v| !v.is_empty()),
            codex_bin: std::env::var_os("DMUXAI_CODEX_BIN").filter(|v| !v.is_empty()),
            config_path: std::env::var_os("DMUXAI_CONFIG")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            home: std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            userprofile: std::env::var_os("USERPROFILE")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            path: std::env::var_os("PATH"),
            reduced_motion: std::env::var_os("DMUXAI_REDUCED_MOTION").filter(|v| !v.is_empty()),
        }
    }

    pub fn home_dir(&self) -> Option<PathBuf> {
        self.home.clone().or_else(|| self.userprofile.clone())
    }
}
