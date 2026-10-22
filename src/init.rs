//! `dmuxai init`: writes a commented starter `dmuxai.toml` (SPEC-first-run FR-814).

use std::path::{Path, PathBuf};

use crate::settings::{default_user_config_path, find_grok_bin, find_on_path, find_opencode_bin};

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
# Graph orchestration splits multi-step prompts across runners (always on).
# max_parallel = 2
# max_retries = 2
# When true, write a markdown task checklist at
# <workspace>/.dmuxai/tasks/TASKS-<run id>.md on plan approval and when a
# task status changes (default false).
# tasks_file = false
# Unset: every runner may take graph nodes, except built-in presets (opencode
# and grok). A built-in preset is never graph-routed unless it is listed here.
# allowed_runners = ["cursor", "agy", "codex"]

# [cursor]
# Pre-warm the next cursor-agent process in the background (TUI / web only).
# prewarm = true
# Idle expiry for the pre-warmed process in seconds.
# prewarm_idle_secs = 300

# [agents]
# The built-in opencode runner is registered automatically when `opencode` is on
# PATH (or DMUXAI_OPENCODE_BIN is set) and no [[agents.custom]] entry is named
# "opencode". It uses the free-model policy: only opencode/*-free models.
# Set to false to hide it.
# builtin_opencode = true
# The built-in grok runner is registered when `grok` is on PATH, DMUXAI_GROK_BIN
# names an existing file, or ~/.grok/bin/grok is executable, and no
# [[agents.custom]] entry is named "grok". Models match grok-* and must not
# contain fast. The default is grok-4.7. A logged-out CLI still lists the runner
# and asks you to run `grok login`. Set to false to hide it. Graph nodes need
# allowed_runners to include grok.
# builtin_grok = true
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
# command = ["opencode", "run", "--format", "json", "-m", "{model}", "{prompt}"]
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

/// Commented copy of the built-in grok preset (FR-900). It is only written when
/// grok is detected. Uncommenting it replaces the built-in preset. The example
/// does not set `builtin`.
const GROK_EXAMPLE: &str = r##"# Built-in grok runner, as an editable [[agents.custom]] entry. Uncommenting
# it replaces the built-in preset. The command is `grok -p` with streaming-json.
# [[agents.custom]]
# name = "grok"
# label = "grok"
# color = "#6366f1"
# command = ["grok", "-p", "{prompt}", "-m", "{model}", "--cwd", "{workspace}", "--output-format", "streaming-json", "--no-wait-for-background"]
# prompt_via = "arg"
# models_command = ["grok", "models"]
# allowed_models = "(?i)^grok-[a-z0-9][a-z0-9._-]*$"
# output = "jsonl"
# text_path = "/data"
# text_when = { path = "/type", equals = "text" }
# session_id_path = "/sessionId"
# resume_arg = ["-r", "{session_id}"]
# timeout_secs = 900
# done_when = { path = "/type", equals = "end" }
# text_break_when = { path = "/type", equals = "tool_call" }
# [agents.custom.env]
# GROK_AUTO_WAKE = "0"
# [agents.custom.modes]
# ask = ["--permission-mode", "plan"]
# plan = ["--permission-mode", "plan"]
# implement = ["--always-approve"]
"##;

/// Runner executables reported by `dmuxai init`, in report order.
const RUNNER_BINARIES: [&str; 5] = ["cursor-agent", "agy", "codex", "opencode", "grok"];

/// The starter config text. Each example is added only when that runner is detected.
pub fn starter_config(opencode_detected: bool, grok_detected: bool) -> String {
    let mut text = STARTER_CONFIG.to_owned();
    if opencode_detected {
        text.push('\n');
        text.push_str(OPENCODE_EXAMPLE);
    }
    if grok_detected {
        text.push('\n');
        text.push_str(GROK_EXAMPLE);
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
    grok_detected: bool,
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
    std::fs::write(path, starter_config(opencode_detected, grok_detected))
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

/// Runners whose executables are found on `PATH` (opencode honours
/// `DMUXAI_OPENCODE_BIN`; grok honours `DMUXAI_GROK_BIN` and `~/.grok/bin`).
pub fn detected_runners() -> Vec<&'static str> {
    RUNNER_BINARIES
        .into_iter()
        .filter(|name| match *name {
            "opencode" => find_opencode_bin().is_some(),
            "grok" => find_grok_bin().is_some(),
            _ => find_on_path(name).is_some(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_agent::{CustomAgentSpec, grok_preset, opencode_preset};

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
        assert!(!starter_config(false, false).contains(example));
        assert!(starter_config(true, false).contains(example));
    }

    // TC-906 / FR-900
    #[test]
    fn starter_config_mentions_builtin_grok() {
        let detected = starter_config(false, true);
        let undetected = starter_config(true, false);
        for text in [&detected, &undetected] {
            assert!(text.contains("# builtin_grok = true"), "{text}");
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("dmuxai.toml");
            std::fs::write(&path, text).unwrap();
            let settings = crate::settings::DmuxSettings::load_with_runners(
                dir.path(),
                Some(&path),
                None,
                None,
                None,
            );
            assert_eq!(settings, crate::settings::DmuxSettings::default());
        }
        assert!(detected.contains("# name = \"grok\""));
        assert!(
            !undetected.contains("name = \"grok\""),
            "undetected starter must not include a grok custom example"
        );

        let uncommented: String = GROK_EXAMPLE
            .lines()
            .filter_map(|line| line.strip_prefix('#')?.strip_prefix(' '))
            .filter(|line| {
                line.starts_with('[')
                    || line
                        .split_once(" = ")
                        .is_some_and(|(key, _)| !key.contains(' '))
            })
            .map(|line| format!("{line}\n"))
            .collect();
        let doc: Doc = toml::from_str(&uncommented).expect("grok example parses");
        let [example] = doc.agents.custom.as_slice() else {
            panic!("one grok example entry");
        };
        let mut expected = grok_preset("grok");
        expected.builtin = false;
        assert_eq!(example, &expected);

        let docs = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/docs/agents/custom-agents.md"
        ))
        .unwrap();
        assert!(
            docs.contains("grok agent stdio"),
            "custom-agents.md must name grok agent stdio"
        );
        assert!(
            docs.contains("not the built-in path"),
            "custom-agents.md must say grok agent stdio is not the built-in path"
        );
    }

    #[test]
    fn write_refuses_overwrite_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("dmuxai.toml");
        write_starter_config(&path, false, false, false).unwrap();
        assert!(path.is_file());
        let error = write_starter_config(&path, false, false, false).unwrap_err();
        assert!(error.contains("--force"), "{error}");
        write_starter_config(&path, true, false, true).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("[[agents.custom]]")
        );
    }
}
