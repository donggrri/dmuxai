use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const MIN_DMUXAI_VERSION: &str = "0.1.1";

/// Trait abstracting OS/WSL environment probing so unit tests can use a fake.
pub trait Probe {
    fn wsl_installed(&self) -> bool;
    fn list_distros(&self) -> Result<Vec<String>, String>;
    fn wslpath(&self, distro: &str, win_path: &str) -> Result<String, String>;
    fn has_command(&self, distro: &str, name: &str) -> bool;
    fn dmuxai_version(&self, distro: &str) -> Result<String, String>;
    fn has_windows_terminal(&self) -> bool;
}

/// Fake probe for deterministic unit testing.
#[derive(Debug, Clone, Default)]
pub struct FakeProbe {
    pub wsl_installed_result: bool,
    pub distros: Vec<String>,
    pub wslpaths: HashMap<(String, String), Result<String, String>>,
    pub commands: HashSet<(String, String)>,
    pub versions: HashMap<String, Result<String, String>>,
    pub has_wt: bool,
}

impl Probe for FakeProbe {
    fn wsl_installed(&self) -> bool {
        self.wsl_installed_result
    }

    fn list_distros(&self) -> Result<Vec<String>, String> {
        Ok(self.distros.clone())
    }

    fn wslpath(&self, distro: &str, win_path: &str) -> Result<String, String> {
        if let Some(res) = self
            .wslpaths
            .get(&(distro.to_string(), win_path.to_string()))
        {
            return res.clone();
        }
        if win_path.len() >= 2 && win_path.chars().nth(1) == Some(':') {
            let drive = win_path.chars().next().unwrap().to_ascii_lowercase();
            let rest = &win_path[2..].replace('\\', "/");
            Ok(format!("/mnt/{drive}{rest}"))
        } else {
            Ok(win_path.replace('\\', "/"))
        }
    }

    fn has_command(&self, distro: &str, name: &str) -> bool {
        self.commands
            .contains(&(distro.to_string(), name.to_string()))
    }

    fn dmuxai_version(&self, distro: &str) -> Result<String, String> {
        self.versions
            .get(distro)
            .cloned()
            .unwrap_or_else(|| Err("Version probe failed".to_string()))
    }

    fn has_windows_terminal(&self) -> bool {
        self.has_wt
    }
}

/// Real probe that executes WSL and Windows commands via `std::process::Command`.
pub struct RealProbe;

#[allow(dead_code)]
fn decode_utf16_or_utf8(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let mut u16s = Vec::with_capacity((bytes.len() - 2) / 2);
        let mut i = 2;
        while i + 1 < bytes.len() {
            u16s.push(u16::from_le_bytes([bytes[i], bytes[i + 1]]));
            i += 2;
        }
        return String::from_utf16_lossy(&u16s);
    }
    if bytes.len() >= 2 && bytes.len().is_multiple_of(2) && bytes[1] == 0 {
        let mut u16s = Vec::with_capacity(bytes.len() / 2);
        let mut i = 0;
        while i + 1 < bytes.len() {
            u16s.push(u16::from_le_bytes([bytes[i], bytes[i + 1]]));
            i += 2;
        }
        return String::from_utf16_lossy(&u16s);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Builds argv vector for running wslpath inside WSL.
pub fn probe_wslpath_argv(distro: &str, win_path: &str) -> Vec<String> {
    vec![
        "-d".to_string(),
        distro.to_string(),
        "--exec".to_string(),
        "wslpath".to_string(),
        "-u".to_string(),
        win_path.to_string(),
    ]
}

/// Builds argv vector for running dmuxai --version inside WSL.
pub fn probe_version_argv(distro: &str) -> Vec<String> {
    vec![
        "-d".to_string(),
        distro.to_string(),
        "--exec".to_string(),
        "sh".to_string(),
        "-lc".to_string(),
        "exec dmuxai --version".to_string(),
    ]
}

/// Builds argv vector for probing command existence via login shell inside WSL.
pub fn probe_has_command_argv(distro: &str, name: &str) -> Vec<String> {
    vec![
        "-d".to_string(),
        distro.to_string(),
        "--exec".to_string(),
        "sh".to_string(),
        "-lc".to_string(),
        "command -v \"$1\" >/dev/null".to_string(),
        "sh".to_string(),
        name.to_string(),
    ]
}

impl Probe for RealProbe {
    fn wsl_installed(&self) -> bool {
        #[cfg(windows)]
        {
            std::process::Command::new("wsl.exe")
                .arg("--status")
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false)
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    fn list_distros(&self) -> Result<Vec<String>, String> {
        #[cfg(windows)]
        {
            let output = std::process::Command::new("wsl.exe")
                .args(["-l", "-q"])
                .output()
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
            }
            let text = decode_utf16_or_utf8(&output.stdout);
            let distros: Vec<String> = text
                .lines()
                .map(|l| l.trim().trim_matches('\0').to_string())
                .filter(|l| !l.is_empty())
                .collect();
            Ok(distros)
        }
        #[cfg(not(windows))]
        {
            Err("WSL is only available on Windows".to_string())
        }
    }

    fn wslpath(&self, distro: &str, win_path: &str) -> Result<String, String> {
        #[cfg(windows)]
        {
            let args = probe_wslpath_argv(distro, win_path);
            let output = std::process::Command::new("wsl.exe")
                .args(&args)
                .output()
                .map_err(|e| e.to_string())?;
            if output.status.success() {
                let converted = decode_utf16_or_utf8(&output.stdout)
                    .trim()
                    .trim_matches('\0')
                    .to_string();
                Ok(converted)
            } else {
                Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (distro, win_path);
            Err("wslpath is only available on Windows".to_string())
        }
    }

    fn has_command(&self, distro: &str, name: &str) -> bool {
        #[cfg(windows)]
        {
            let args = probe_has_command_argv(distro, name);
            std::process::Command::new("wsl.exe")
                .args(&args)
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false)
        }
        #[cfg(not(windows))]
        {
            let _ = (distro, name);
            false
        }
    }

    fn dmuxai_version(&self, distro: &str) -> Result<String, String> {
        #[cfg(windows)]
        {
            let args = probe_version_argv(distro);
            let output = std::process::Command::new("wsl.exe")
                .args(&args)
                .output()
                .map_err(|e| e.to_string())?;
            if output.status.success() {
                Ok(decode_utf16_or_utf8(&output.stdout)
                    .trim()
                    .trim_matches('\0')
                    .to_string())
            } else {
                Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
            }
        }
        #[cfg(not(windows))]
        {
            let _ = distro;
            Err("dmuxai_version is only available on Windows".to_string())
        }
    }

    fn has_windows_terminal(&self) -> bool {
        #[cfg(windows)]
        {
            std::process::Command::new("where.exe")
                .arg("wt.exe")
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false)
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

/// Launcher configuration stored in JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LauncherConfig {
    pub distro: Option<String>,
    pub last_workspace: Option<String>,
    #[serde(default)]
    pub allow_mnt: bool,
}

impl LauncherConfig {
    pub fn load_from_str(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }

    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => Self::load_from_str(&content),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    pub fn default_path() -> Option<PathBuf> {
        #[cfg(windows)]
        {
            if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
                return Some(
                    PathBuf::from(local_app_data)
                        .join("DmuxAI")
                        .join("launcher.json"),
                );
            }
        }
        None
    }
}

/// Distro selection result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DistroChoice {
    Auto(String),
    Remembered(String),
    NeedsPicker(Vec<String>),
    Blocked { reason: String, fixit: String },
}

/// Selects a WSL distribution based on remembered preference and available distros.
pub fn choose_distro(remembered: Option<&str>, distros: &[String]) -> DistroChoice {
    if distros.is_empty() {
        return DistroChoice::Blocked {
            reason: "No WSL distributions installed".to_string(),
            fixit: "wsl --install -d Ubuntu".to_string(),
        };
    }

    if let Some(rem) = remembered.filter(|rem| distros.iter().any(|d| d == *rem)) {
        return DistroChoice::Remembered(rem.to_string());
    }

    if distros.len() == 1 {
        DistroChoice::Auto(distros[0].clone())
    } else {
        DistroChoice::NeedsPicker(distros.to_vec())
    }
}

/// Converts a Windows workspace path to a Linux WSL path, rejecting UNC paths.
pub fn windows_to_wsl_path(
    probe: &impl Probe,
    distro: &str,
    win_path: &str,
) -> Result<String, String> {
    let trimmed = win_path.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with(r"\\wsl$")
        || lower.starts_with(r"\\wsl.localhost")
        || lower.starts_with("//wsl$")
        || lower.starts_with("//wsl.localhost")
    {
        return Err(
            "UNC paths are not supported; use a local Windows drive path or canonical Linux path"
                .to_string(),
        );
    }

    match probe.wslpath(distro, trimmed) {
        Ok(converted) => {
            let lower_conv = converted.to_ascii_lowercase();
            if lower_conv.starts_with(r"\\wsl$")
                || lower_conv.starts_with(r"\\wsl.localhost")
                || lower_conv.starts_with("//wsl$")
                || lower_conv.starts_with("//wsl.localhost")
            {
                return Err("UNC paths are not supported; use a local Windows drive path or canonical Linux path".to_string());
            }
            Ok(converted)
        }
        Err(err) => Err(format!(
            "Failed to resolve path '{trimmed}' inside distro '{distro}': {err}"
        )),
    }
}

/// Non-blocking missing agent CLI notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingCli {
    pub name: String,
    pub fixit: String,
}

/// Result of first-run diagnostic checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    Blocked { reason: String, fixit: String },
    Ok { warnings: Vec<MissingCli> },
}

/// Runs first-run checks: WSL status, distro presence, in-distro dmuxai, and agent CLIs.
pub fn run_checks(probe: &impl Probe, distro: Option<&str>) -> CheckResult {
    if !probe.wsl_installed() {
        return CheckResult::Blocked {
            reason: "WSL is not installed or not running".to_string(),
            fixit: "wsl --install".to_string(),
        };
    }

    let distros = match probe.list_distros() {
        Ok(d) if !d.is_empty() => d,
        _ => {
            return CheckResult::Blocked {
                reason: "No WSL distribution found".to_string(),
                fixit: "wsl --install -d Ubuntu".to_string(),
            };
        }
    };

    let target_distro = match distro {
        Some(d) => {
            if !distros.iter().any(|name| name == d) {
                return CheckResult::Blocked {
                    reason: format!("WSL distribution '{d}' is not installed"),
                    fixit: format!("wsl --install -d {d}"),
                };
            }
            d
        }
        None => &distros[0],
    };

    if !probe.has_command(target_distro, "dmuxai") {
        return CheckResult::Blocked {
            reason: format!("dmuxai is not installed inside WSL distro '{target_distro}'"),
            fixit: "install dmuxai inside WSL: git clone https://github.com/donggrri/dmuxai && cd dmuxai && cargo install --path .".to_string(),
        };
    }

    let agent_clis = [
        ("agy", "see the provider install docs (agy)"),
        (
            "cursor-agent",
            "curl https://cursor.com/install -fsS | bash",
        ),
        ("opencode", "curl -fsSL https://opencode.ai/install | bash"),
    ];

    let mut warnings = Vec::new();
    for (cli, fixit) in agent_clis {
        if !probe.has_command(target_distro, cli) {
            warnings.push(MissingCli {
                name: cli.to_string(),
                fixit: fixit.to_string(),
            });
        }
    }

    CheckResult::Ok { warnings }
}

/// Gate check for /mnt/ drive performance warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MntGateResult {
    Allowed,
    WarnAndPrompt { warning: String },
}

pub fn mnt_gate(path: &str, allow_mnt: bool) -> MntGateResult {
    if path.starts_with("/mnt/") {
        if allow_mnt {
            MntGateResult::Allowed
        } else {
            MntGateResult::WarnAndPrompt {
                warning: format!(
                    "Workspace path '{path}' is on a Windows mount (/mnt/...). File permissions and performance are degraded; prefer a Linux path under ~/projects/. Use --allow-mnt to proceed."
                ),
            }
        }
    } else {
        MntGateResult::Allowed
    }
}

/// Version gate check result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionGateResult {
    Ok,
    Stale {
        current: String,
        required: String,
        fixit: String,
    },
    Unverified {
        warning: String,
    },
}

/// Parses simple major.minor.patch semver from a string.
pub fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let trimmed = s.trim();
    for token in trimmed.split_whitespace() {
        let parts: Vec<&str> = token.split('.').collect();
        if let [p0, p1, p2] = parts.as_slice()
            && let (Ok(major), Ok(minor), Ok(patch)) =
                (p0.parse::<u64>(), p1.parse::<u64>(), p2.parse::<u64>())
        {
            return Some((major, minor, patch));
        }
    }
    None
}

/// Checks that dmuxai inside the distro satisfies the MIN_DMUXAI_VERSION floor.
pub fn version_gate(probe: &impl Probe, distro: &str) -> VersionGateResult {
    let min_ver = parse_semver(MIN_DMUXAI_VERSION).unwrap_or((0, 1, 1));

    match probe.dmuxai_version(distro) {
        Ok(version_str) => {
            if let Some(parsed) = parse_semver(&version_str) {
                if parsed < min_ver {
                    VersionGateResult::Stale {
                        current: version_str,
                        required: MIN_DMUXAI_VERSION.to_string(),
                        fixit: "cd dmuxai && git pull && cargo install --path .".to_string(),
                    }
                } else {
                    VersionGateResult::Ok
                }
            } else {
                VersionGateResult::Unverified {
                    warning: format!("Cannot verify version from output: '{version_str}'"),
                }
            }
        }
        Err(err) => VersionGateResult::Unverified {
            warning: format!("Cannot verify dmuxai version: {err}"),
        },
    }
}

/// Launch command arguments and optional fallback notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    pub argv: Vec<String>,
    pub notice: Option<String>,
}

/// Builds the child process argv vector without assembling shell strings.
pub fn build_launch_command(
    probe: &impl Probe,
    distro: &str,
    linux_path: &str,
) -> Result<LaunchCommand, String> {
    if linux_path.contains(';') {
        return Err(format!(
            "Workspace path '{linux_path}' contains ';'. Windows Terminal treats ';' as a command separator. Fix-it: move or rename the workspace directory to a path without semicolons."
        ));
    }

    if probe.has_windows_terminal() {
        Ok(LaunchCommand {
            argv: vec![
                "wt.exe".to_string(),
                "new-tab".to_string(),
                "wsl.exe".to_string(),
                "-d".to_string(),
                distro.to_string(),
                "--cd".to_string(),
                linux_path.to_string(),
                "--exec".to_string(),
                "sh".to_string(),
                "-lc".to_string(),
                "exec dmuxai".to_string(),
            ],
            notice: None,
        })
    } else {
        Ok(LaunchCommand {
            argv: vec![
                "wsl.exe".to_string(),
                "-d".to_string(),
                distro.to_string(),
                "--cd".to_string(),
                linux_path.to_string(),
                "--exec".to_string(),
                "sh".to_string(),
                "-lc".to_string(),
                "exec dmuxai".to_string(),
            ],
            notice: Some("Windows Terminal (wt.exe) not found; falling back to default console. Install Windows Terminal for tabs and full ANSI support.".to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // TC-390: stubbed environment with wt.exe present; distro Ubuntu; workspace C:\src\p -> /home/u/p
    // oracle: argv is exactly wt.exe new-tab wsl.exe -d Ubuntu --cd /home/u/p --exec dmuxai (no shell string)
    #[test]
    fn launcher_builds_windows_terminal_command() {
        let fake = FakeProbe {
            has_wt: true,
            ..Default::default()
        };
        let cmd = build_launch_command(&fake, "Ubuntu", "/home/u/p").unwrap();
        assert_eq!(
            cmd.argv,
            vec![
                "wt.exe",
                "new-tab",
                "wsl.exe",
                "-d",
                "Ubuntu",
                "--cd",
                "/home/u/p",
                "--exec",
                "sh",
                "-lc",
                "exec dmuxai",
            ]
        );
        assert!(cmd.notice.is_none());
    }

    // TC-391: wt.exe absent
    // oracle: argv is wsl.exe -d ... --cd ... --exec dmuxai and a notice naming Windows Terminal install source is recorded
    #[test]
    fn launcher_falls_back_without_windows_terminal() {
        let fake = FakeProbe {
            has_wt: false,
            ..Default::default()
        };
        let cmd = build_launch_command(&fake, "Ubuntu", "/home/u/p").unwrap();
        assert_eq!(
            cmd.argv,
            vec![
                "wsl.exe",
                "-d",
                "Ubuntu",
                "--cd",
                "/home/u/p",
                "--exec",
                "sh",
                "-lc",
                "exec dmuxai",
            ]
        );
        assert!(cmd.notice.is_some());
        let notice = cmd.notice.unwrap();
        assert!(notice.contains("Windows Terminal") || notice.contains("wt.exe"));
    }

    // TC-392: launcher config with remembered distro and last workspace
    // oracle: remembered values are used without prompting; corrupt/unreadable config behaves as not remembered
    #[test]
    fn launcher_remembers_distro_and_workspace() {
        let config_json = r#"{"distro":"Ubuntu","last_workspace":"/home/u/p","allow_mnt":true}"#;
        let config = LauncherConfig::load_from_str(config_json);
        assert_eq!(config.distro.as_deref(), Some("Ubuntu"));
        assert_eq!(config.last_workspace.as_deref(), Some("/home/u/p"));
        assert!(config.allow_mnt);

        let distros = vec!["Ubuntu".to_string(), "Debian".to_string()];
        let choice = choose_distro(config.distro.as_deref(), &distros);
        assert_eq!(choice, DistroChoice::Remembered("Ubuntu".to_string()));

        // Corrupt / unreadable config behaves as default
        let corrupt_config = LauncherConfig::load_from_str("invalid json {{{");
        assert_eq!(corrupt_config, LauncherConfig::default());
        assert!(corrupt_config.distro.is_none());
        assert!(corrupt_config.last_workspace.is_none());
        assert!(!corrupt_config.allow_mnt);

        let choice_corrupt = choose_distro(corrupt_config.distro.as_deref(), &distros);
        assert_eq!(choice_corrupt, DistroChoice::NeedsPicker(distros));
    }

    // TC-393: two distros listed, none remembered
    // oracle: picker lists both distros; the selected one is persisted and reported
    #[test]
    fn launcher_prompts_when_distro_ambiguous() {
        let distros = vec!["Ubuntu".to_string(), "Debian".to_string()];
        let choice = choose_distro(None, &distros);
        assert_eq!(choice, DistroChoice::NeedsPicker(distros.clone()));

        // Persisted distro is remembered
        let config = LauncherConfig {
            distro: Some("Debian".to_string()),
            ..Default::default()
        };
        let choice_persisted = choose_distro(config.distro.as_deref(), &distros);
        assert_eq!(
            choice_persisted,
            DistroChoice::Remembered("Debian".to_string())
        );

        // Empty distro list behaves like missing distro
        let empty: Vec<String> = vec![];
        let choice_empty = choose_distro(None, &empty);
        assert!(matches!(choice_empty, DistroChoice::Blocked { .. }));
    }

    // TC-394: input C:\src\my project and a fake wslpath
    // oracle: result is /mnt/c/src/my project round-tripped through fake; a \\wsl$ input raises never-reemit error path
    #[test]
    fn windows_path_maps_to_wsl_path() {
        let mut fake = FakeProbe::default();
        fake.wslpaths.insert(
            ("Ubuntu".to_string(), r"C:\src\my project".to_string()),
            Ok("/mnt/c/src/my project".to_string()),
        );
        let converted = windows_to_wsl_path(&fake, "Ubuntu", r"C:\src\my project").unwrap();
        assert_eq!(converted, "/mnt/c/src/my project");

        // UNC \\wsl$ path rejected without reemitting \\wsl$ or \\wsl.localhost
        let err = windows_to_wsl_path(&fake, "Ubuntu", r"\\wsl$\Ubuntu\home\u\p").unwrap_err();
        assert!(!err.contains(r"\\wsl$"));
        assert!(!err.contains(r"\\wsl.localhost"));

        let err2 =
            windows_to_wsl_path(&fake, "Ubuntu", r"\\wsl.localhost\Ubuntu\home\u\p").unwrap_err();
        assert!(!err2.contains(r"\\wsl$"));
        assert!(!err2.contains(r"\\wsl.localhost"));
    }

    // TC-395: wsl.exe --status reports not installed
    // oracle: blocked with fix-it naming wsl --install, exit code non-zero, TUI not started
    #[test]
    fn launcher_reports_missing_wsl() {
        let fake = FakeProbe {
            wsl_installed_result: false,
            ..Default::default()
        };
        let res = run_checks(&fake, None);
        match res {
            CheckResult::Blocked { reason, fixit } => {
                assert!(reason.contains("WSL"));
                assert_eq!(fixit, "wsl --install");
            }
            _ => panic!("Expected CheckResult::Blocked"),
        }
    }

    // TC-396: distro present, dmuxai probe returns not-found
    // oracle: blocked with fix-it naming the in-distro install command
    #[test]
    fn launcher_reports_missing_dmuxai_in_distro() {
        let fake = FakeProbe {
            wsl_installed_result: true,
            distros: vec!["Ubuntu".to_string()],
            ..Default::default()
        };
        let res = run_checks(&fake, Some("Ubuntu"));
        match res {
            CheckResult::Blocked { reason, fixit } => {
                assert!(reason.contains("dmuxai"));
                assert_eq!(
                    fixit,
                    "install dmuxai inside WSL: git clone https://github.com/donggrri/dmuxai && cd dmuxai && cargo install --path ."
                );
            }
            _ => panic!("Expected CheckResult::Blocked"),
        }
    }

    // TC-397: agy and cursor-agent present, opencode missing
    // oracle: one non-blocking notice listing opencode with its fix-it command; launch proceeds
    #[test]
    fn launcher_reports_missing_agent_clis_non_blocking() {
        let mut fake = FakeProbe {
            wsl_installed_result: true,
            distros: vec!["Ubuntu".to_string()],
            ..Default::default()
        };
        fake.commands
            .insert(("Ubuntu".to_string(), "dmuxai".to_string()));
        fake.commands
            .insert(("Ubuntu".to_string(), "agy".to_string()));
        fake.commands
            .insert(("Ubuntu".to_string(), "cursor-agent".to_string()));
        // opencode is missing

        let res = run_checks(&fake, Some("Ubuntu"));
        match res {
            CheckResult::Ok { warnings } => {
                assert_eq!(warnings.len(), 1);
                assert_eq!(warnings[0].name, "opencode");
                assert_eq!(
                    warnings[0].fixit,
                    "curl -fsSL https://opencode.ai/install | bash"
                );
            }
            _ => panic!("Expected CheckResult::Ok with warnings"),
        }
    }

    // TC-398: converted path /mnt/c/src/p
    // oracle: performance warning shown; launch denied without confirm; --allow-mnt remembered confirm suppresses repeats
    #[test]
    fn mnt_path_shows_performance_warning() {
        let mnt_path = "/mnt/c/src/p";
        let non_mnt_path = "/home/u/p";

        // Without allow_mnt, warns and prompts
        let gate_warn = mnt_gate(mnt_path, false);
        match gate_warn {
            MntGateResult::WarnAndPrompt { warning } => {
                assert!(warning.contains("/mnt/"));
            }
            _ => panic!("Expected WarnAndPrompt"),
        }

        // With allow_mnt, allowed
        assert_eq!(mnt_gate(mnt_path, true), MntGateResult::Allowed);

        // Non-mnt path allowed without allow_mnt
        assert_eq!(mnt_gate(non_mnt_path, false), MntGateResult::Allowed);
    }

    // TC-402: distro dmuxai --version output older than the launcher floor
    // oracle: update fix-it message with exact command; if version probe fails, "cannot verify version" notice is used and launch continues
    #[test]
    fn launcher_detects_stale_wsl_dmuxai_version() {
        let mut fake = FakeProbe {
            wsl_installed_result: true,
            distros: vec!["Ubuntu".to_string()],
            ..Default::default()
        };

        // Stale version (0.1.0 vs required MIN_DMUXAI_VERSION 0.1.1)
        fake.versions
            .insert("Ubuntu".to_string(), Ok("dmuxai 0.1.0".to_string()));
        let res_stale = version_gate(&fake, "Ubuntu");
        match res_stale {
            VersionGateResult::Stale {
                current,
                required,
                fixit,
            } => {
                assert!(current.contains("0.1.0"));
                assert_eq!(required, MIN_DMUXAI_VERSION);
                assert_eq!(fixit, "cd dmuxai && git pull && cargo install --path .");
            }
            _ => panic!("Expected VersionGateResult::Stale"),
        }

        // Compliant version
        fake.versions
            .insert("Ubuntu".to_string(), Ok("dmuxai 0.1.1".to_string()));
        assert_eq!(version_gate(&fake, "Ubuntu"), VersionGateResult::Ok);

        // Failed probe produces unverified warning
        fake.versions
            .insert("Ubuntu".to_string(), Err("probe failed".to_string()));
        let res_failed = version_gate(&fake, "Ubuntu");
        assert!(matches!(res_failed, VersionGateResult::Unverified { .. }));

        // Unparseable version output produces unverified warning
        fake.versions.insert(
            "Ubuntu".to_string(),
            Ok("nightly-build-unknown".to_string()),
        );
        let res_unparseable = version_gate(&fake, "Ubuntu");
        assert!(matches!(
            res_unparseable,
            VersionGateResult::Unverified { .. }
        ));
    }

    // TC-403: workspace text containing "& ( ) and spaces
    // oracle: each command is one argv list; hostile text stays inside a single argument and never reaches cmd.exe/PowerShell
    #[test]
    fn launcher_never_assembles_shell_command_string() {
        let fake = FakeProbe {
            has_wt: true,
            ..Default::default()
        };
        let hostile_path = r#"/mnt/c/src/my & hostile "quotes" (test) path"#;
        let cmd = build_launch_command(&fake, "Ubuntu", hostile_path).unwrap();

        assert_eq!(cmd.argv.len(), 11);
        assert_eq!(cmd.argv[0], "wt.exe");
        assert_eq!(cmd.argv[1], "new-tab");
        assert_eq!(cmd.argv[2], "wsl.exe");
        assert_eq!(cmd.argv[3], "-d");
        assert_eq!(cmd.argv[4], "Ubuntu");
        assert_eq!(cmd.argv[5], "--cd");
        assert_eq!(cmd.argv[6], hostile_path);
        assert_eq!(cmd.argv[7], "--exec");
        assert_eq!(cmd.argv[8..], ["sh", "-lc", "exec dmuxai"]);
    }

    #[test]
    fn probe_argv_builders_use_exec_and_keep_hostile_args_separate() {
        let hostile = "a&b;c";
        let distro = "Ubuntu";

        let wslpath_argv = probe_wslpath_argv(distro, hostile);
        assert!(wslpath_argv.contains(&"--exec".to_string()));
        assert_eq!(wslpath_argv.last().unwrap(), hostile);

        let version_argv = probe_version_argv(distro);
        assert!(version_argv.contains(&"--exec".to_string()));

        let has_cmd_argv = probe_has_command_argv(distro, hostile);
        assert!(has_cmd_argv.contains(&"--exec".to_string()));
        assert_eq!(has_cmd_argv.last().unwrap(), hostile);
        assert_eq!(
            has_cmd_argv,
            vec![
                "-d",
                distro,
                "--exec",
                "sh",
                "-lc",
                "command -v \"$1\" >/dev/null",
                "sh",
                hostile,
            ]
        );
    }

    #[test]
    fn launcher_rejects_semicolon_in_path() {
        let fake = FakeProbe {
            has_wt: true,
            ..Default::default()
        };
        let path_with_semicolon = "/home/u/project;malicious";
        let err = build_launch_command(&fake, "Ubuntu", path_with_semicolon).unwrap_err();
        assert!(err.contains(';'));
        assert!(err.contains("Fix-it"));
    }

    #[test]
    fn launcher_config_save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("launcher.json");
        let config = LauncherConfig {
            distro: Some("Ubuntu".to_string()),
            last_workspace: Some("C:\\projects\\demo".to_string()),
            allow_mnt: true,
        };
        config.save(&file).unwrap();
        let loaded = LauncherConfig::load(&file);
        assert_eq!(config, loaded);
    }
}
