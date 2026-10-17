//! `dmuxai init`: writes a commented starter `dmuxai.toml` (SPEC-first-run FR-814).

use std::path::{Path, PathBuf};

use crate::settings::{default_user_config_path, find_on_path, find_opencode_bin};

/// Starter config. Every setting is commented out, so the file parses to
/// `DmuxSettings::default()` (SPEC-first-run TC-814).
const STARTER_CONFIG: &str = r##"# DmuxAI configuration (dmuxai.toml). Everything here is optional: DmuxAI runs
# without this file. Uncomment and edit only what you want to change.

# [theme]
# Colors are names (for example "lightblue") or "#rrggbb".
# background = "#101010"
# foreground = "white"
# border = "dark-gray"
# accent = "#ffa500"
# [theme.agents]
# Per-runner chat colors: cursor, agy, codex or a custom agent name.
# cursor = "yellow"

# [routing]
# Runner used when no runner is named explicitly.
# default_agent = "cursor"

# [graph]
# Graph orchestration splits multi-step prompts across runners (on by default).
# enabled = true
# max_parallel = 2
# max_retries = 2
# Unset: every runner may take graph nodes, except the built-in opencode preset.
# The preset is never graph-routed unless it is listed here.
# allowed_runners = ["cursor", "agy", "codex"]

# [cursor]
# windows_unsandboxed = false

# [agents]
# The built-in opencode runner is registered automatically when `opencode` is on
# PATH (or DMUXAI_OPENCODE_BIN is set) and no [[agents.custom]] entry is named
# "opencode". It uses the free-model policy: only opencode/*-free models.
# Set to false to hide it.
# builtin_opencode = true
"##;

/// Commented copy of the built-in opencode preset (FR-814). It is only written
/// when opencode is detected. Uncommenting it replaces the built-in preset.
const OPENCODE_EXAMPLE: &str = r##"# Built-in opencode runner, as an editable [[agents.custom]] entry. Uncommenting
# it replaces the built-in preset (FR-811). To change the model policy, edit
# allowed_models. default_model is left unset, so the first allowed model in
# `opencode models` is used.
# [[agents.custom]]
# name = "opencode"
# label = "opencode"
# color = "#ffa500"
# command = ["opencode", "run", "--format", "json", "-m", "{model}", "--dir", "{workspace}", "{prompt}"]
# prompt_via = "arg"
# image_arg = ["-f", "{image}"]
# models_command = ["opencode", "models"]
# models_filter = "-free$"
# allowed_models = "^opencode/.*-free$"
# output = "jsonl"
# text_path = "/part/text"
# text_when = { path = "/type", equals = "text" }
# error_path = "/error/message"
# session_id_path = "/sessionID"
# resume_arg = ["--session", "{session_id}"]
# timeout_secs = 900
# failure_stderr = "(?i)permission denied|rejected permission"
# temp_config_template = '{"permission":{"edit":"{edit}","bash":"{edit}","webfetch":"allow"}}'
# env = { OPENCODE_CONFIG = "{temp_config}" }
"##;

/// Runner executables reported by `dmuxai init`, in report order.
const RUNNER_BINARIES: [&str; 4] = ["cursor-agent", "agy", "codex", "opencode"];

/// The starter config text. The opencode example is added when `opencode_detected`.
pub fn starter_config(opencode_detected: bool) -> String {
    let mut text = STARTER_CONFIG.to_owned();
    if opencode_detected {
        text.push('\n');
        text.push_str(OPENCODE_EXAMPLE);
    }
    text
}

/// Where `dmuxai init` writes: `<workspace>/dmuxai.toml` with `--workspace`,
/// otherwise the user config file.
pub fn target_path(workspace: bool, workspace_root: &Path) -> Result<PathBuf, String> {
    if workspace {
        return Ok(workspace_root.join("dmuxai.toml"));
    }
    default_user_config_path().ok_or_else(|| {
        "cannot find the user config directory: set HOME or XDG_CONFIG_HOME".to_owned()
    })
}

/// Writes the starter config to `path`, creating parent directories. An existing
/// file is kept unless `force` is set.
pub fn write_starter_config(
    path: &Path,
    opencode_detected: bool,
    force: bool,
) -> Result<(), String> {
    if path.exists() && !force {
        return Err(format!(
            "{} already exists; edit it, or rerun with --force to overwrite it",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    std::fs::write(path, starter_config(opencode_detected))
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

/// Runners whose executables are found on `PATH` (opencode honours
/// `DMUXAI_OPENCODE_BIN`).
pub fn detected_runners() -> Vec<&'static str> {
    RUNNER_BINARIES
        .into_iter()
        .filter(|name| {
            if *name == "opencode" {
                find_opencode_bin().is_some()
            } else {
                find_on_path(name).is_some()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_agent::{CustomAgentSpec, opencode_preset};

    #[derive(serde::Deserialize)]
    struct Doc {
        agents: Agents,
    }

    #[derive(serde::Deserialize)]
    struct Agents {
        custom: Vec<CustomAgentSpec>,
    }

    // TC-814 / FR-814: the commented opencode example equals the built-in
    // preset (the preset's `builtin` flag is never configurable).
    #[test]
    fn opencode_example_equals_builtin_preset() {
        // Keep only the commented TOML lines: table headers and `key = value`.
        let uncommented: String = OPENCODE_EXAMPLE
            .lines()
            .filter_map(|line| line.strip_prefix('#')?.strip_prefix(' '))
            .filter(|line| {
                line.starts_with("[[")
                    || line
                        .split_once(" = ")
                        .is_some_and(|(key, _)| !key.contains(' '))
            })
            .map(|line| format!("{line}\n"))
            .collect();
        let doc: Doc = toml::from_str(&uncommented).expect("example parses");
        let [example] = doc.agents.custom.as_slice() else {
            panic!("one example entry");
        };
        let mut expected = opencode_preset("opencode");
        expected.builtin = false;
        assert_eq!(example, &expected);
    }

    #[test]
    fn starter_config_only_adds_example_when_detected() {
        let example = "# name = \"opencode\"";
        assert!(!starter_config(false).contains(example));
        assert!(starter_config(true).contains(example));
    }

    #[test]
    fn write_refuses_overwrite_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("dmuxai.toml");
        write_starter_config(&path, false, false).unwrap();
        assert!(path.is_file());
        let error = write_starter_config(&path, false, false).unwrap_err();
        assert!(error.contains("--force"), "{error}");
        write_starter_config(&path, true, true).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("[[agents.custom]]")
        );
    }
}
