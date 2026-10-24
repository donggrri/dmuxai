//! Headless CLI contract tests (SPEC-agent-api FR-760..FR-764).
//!
//! Every test runs the real binary against a temporary workspace, state dir
//! and config dir, so the user's own dmuxai config is never read.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    workspace: PathBuf,
}

impl Sandbox {
    fn new(config: &str) -> Self {
        let sandbox = Self::bare();
        std::fs::write(sandbox.workspace.join("dmuxai.toml"), config).unwrap();
        sandbox
    }

    /// A fresh install: empty HOME and config dir, no `dmuxai.toml` anywhere.
    fn bare() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let workspace = root.join("ws");
        for sub in ["ws", "state", "cfg", "home"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        Self {
            _dir: dir,
            root,
            workspace,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with_stdin(args, None)
    }

    fn run_with_stdin(&self, args: &[&str], stdin: Option<&str>) -> Output {
        self.run_in_env(args, stdin, &[])
    }

    /// Runs the binary with `env` overrides (for example a custom `PATH`).
    fn run_in_env(&self, args: &[&str], stdin: Option<&str>, env: &[(&str, OsString)]) -> Output {
        use std::io::Write;
        let mut child = self
            .command(args, env)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("execute dmuxai");
        if let Some(text) = stdin {
            let mut pipe = child.stdin.take().unwrap();
            pipe.write_all(text.as_bytes()).unwrap();
        }
        child.wait_with_output().expect("wait for dmuxai")
    }

    /// Common Command setup shared by `run_in_env` and `spawn_in_env`.
    fn command(&self, args: &[&str], env: &[(&str, OsString)]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dmuxai"));
        cmd.args(args)
            .current_dir(&self.workspace)
            .env("DMUXAI_WORKSPACE", &self.workspace)
            .env("DMUXAI_STATE_DIR", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("APPDATA", self.root.join("cfg"))
            .env("HOME", self.root.join("home"))
            .env_remove("DMUXAI_WEB_TOKEN")
            .env_remove("DMUXAI_OPENCODE_BIN")
            .env_remove("DMUXAI_GROK_BIN");
        cmd.envs(env.iter().map(|(key, value)| (*key, value.clone())));
        cmd
    }

    /// Spawns the binary with environment overrides, returning the child process.
    fn spawn_in_env(&self, args: &[&str], env: &[(&str, OsString)]) -> std::process::Child {
        self.command(args, env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("execute dmuxai")
    }
}

fn stdout_json(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout is not one JSON document ({e}): {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// Writes a fake `opencode` script into `<root>/bin` and returns that directory.
/// `opencode models` lists one free and one paid model.
#[cfg(unix)]
fn fake_opencode(root: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("opencode");
    std::fs::write(
        &script,
        "#!/bin/sh\nif [ \"$1\" = models ]; then echo opencode/glm-5-free; echo opencode/paid; fi\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// `PATH` with `dir` first, so the fake opencode wins the lookup.
#[cfg(unix)]
fn path_with(dir: &Path) -> OsString {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).unwrap()
}

fn fake_agent_config(behavior: &str, allowed: Option<&str>) -> String {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-agent.sh");
    let allowed = allowed
        .map(|list| format!("allowed_runners = [{list}]\n"))
        .unwrap_or_default();
    format!(
        r#"[graph]
enabled = false
{allowed}
[[agents.custom]]
name = "fake"
command = ["sh", "{fixture}", "{behavior}", "{{prompt}}"]
prompt_via = "arg"
output = "jsonl"
text_path = "/part/text"
text_when = {{ path = "/type", equals = "text" }}
error_path = "/error/message"
timeout_secs = 30
"#,
        fixture = fixture.display().to_string().replace('\\', "/"),
    )
}

// TC-761 / FR-761, NFR-760: a headless run prints exactly one JSON object
// with the agent text and exits 0; a failing agent exits 1 with ok=false.
#[cfg(unix)]
#[test]
fn headless_run_json_success_and_failure() {
    let ok = Sandbox::new(&fake_agent_config("jsonl", None));
    let output = ok.run(&["run", "hello", "world", "--agent", "fake", "--json"]);
    let json = stdout_json(&output);
    assert_eq!(output.status.code(), Some(0), "{json}");
    assert_eq!(json["ok"], true);
    assert_eq!(json["exit_code"], 0);
    assert_eq!(json["agent"], "fake");
    assert!(
        json["text"].as_str().unwrap().contains("echo:hello world"),
        "{json}"
    );
    assert!(
        json["events"]["last_seq"].as_u64().unwrap()
            >= json["events"]["first_seq"].as_u64().unwrap()
    );

    // Plain mode prints the text only.
    let plain = ok.run(&["run", "again", "--agent", "fake"]);
    assert_eq!(plain.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&plain.stdout).contains("echo:again"));

    // `run -` reads the prompt from stdin; empty stdin is a usage error.
    let piped = ok.run_with_stdin(
        &["run", "-", "--agent", "fake", "--json"],
        Some("from stdin\n"),
    );
    let json = stdout_json(&piped);
    assert_eq!(piped.status.code(), Some(0), "{json}");
    assert!(
        json["text"].as_str().unwrap().contains("echo:from stdin"),
        "{json}"
    );
    let empty = ok.run_with_stdin(&["run", "-", "--agent", "fake", "--json"], Some(""));
    assert_eq!(empty.status.code(), Some(2));

    let failing = Sandbox::new(&fake_agent_config("error", None));
    let output = failing.run(&["run", "x", "--agent", "fake", "--json"]);
    let json = stdout_json(&output);
    assert_eq!(output.status.code(), Some(1), "{json}");
    assert_eq!(json["ok"], false);
    assert!(
        json["error"].as_str().is_some_and(|e| !e.is_empty()),
        "{json}"
    );
}

// TC-762 / FR-761, NFR-762: allowed_runners is enforced and unknown agents
// are reported with their own exit codes.
#[test]
fn headless_run_respects_allowed_runners() {
    let sandbox = Sandbox::new(&fake_agent_config("jsonl", Some(r#""fake""#)));
    let output = sandbox.run(&["run", "x", "--agent", "agy", "--json"]);
    let json = stdout_json(&output);
    assert_eq!(output.status.code(), Some(5), "{json}");
    assert_eq!(json["ok"], false);
    assert!(json["error"].as_str().unwrap().contains("allowed_runners"));

    let output = sandbox.run(&["run", "x", "--agent", "nobody", "--json"]);
    let json = stdout_json(&output);
    assert_eq!(output.status.code(), Some(3), "{json}");

    let usage = sandbox.run(&["run", "x", "--agent", "fake", "--graph"]);
    assert_eq!(usage.status.code(), Some(2));
}

// TC-1768 / FR-1768: explicit --agent CLI run emits only agent events, no graph or node events.
#[cfg(unix)]
#[test]
fn tc_1768_cli_run_agent_emits_no_graph_events() {
    let sandbox = Sandbox::new(&fake_agent_config("jsonl", None));
    let output = sandbox.run(&["run", "hi", "--agent", "fake", "--json"]);
    let json = stdout_json(&output);
    assert_eq!(output.status.code(), Some(0), "{json}");
    assert!(
        json["graph_id"].is_null(),
        "graph_id must be null for direct agent run, got: {json}"
    );
    assert_eq!(json["ok"], true);

    let events_output = sandbox.run(&["events", "--json"]);
    assert_eq!(events_output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&events_output.stdout);
    let mut saw_agent_event = false;
    for line in stdout.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            let kind = v["kind"].as_str().unwrap_or_default();
            assert_ne!(kind, "graph", "unexpected graph event: {line}");
            assert_ne!(kind, "node", "unexpected node event: {line}");
            if kind == "agent" {
                saw_agent_event = true;
            }
        }
    }
    assert!(
        saw_agent_event,
        "expected at least one agent event in: {stdout}"
    );
}

// TC-1777 / FR-1777: dmuxai run hi --agent fake --json --progress -> stderr has
// at least a started and a completed progress JSON line, stdout is exactly one
// JSON object; without --progress stderr has no progress lines.
#[cfg(unix)]
#[test]
fn tc_1777_cli_progress_lines_before_final_json() {
    let sandbox = Sandbox::new(&fake_agent_config("jsonl", None));
    let output = sandbox.run(&["run", "hi", "--agent", "fake", "--json", "--progress"]);
    assert_eq!(output.status.code(), Some(0));

    // stdout is exactly one JSON object
    let json = stdout_json(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["agent"], "fake");

    // stderr has at least started and completed progress lines
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut saw_started = false;
    let mut saw_completed = false;
    for line in stderr.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
            && v["type"] == "progress"
        {
            if v["kind"] == "started" {
                saw_started = true;
            }
            if v["kind"] == "completed" {
                saw_completed = true;
            }
        }
    }
    assert!(
        saw_started,
        "expected started progress line in stderr: {stderr}"
    );
    assert!(
        saw_completed,
        "expected completed progress line in stderr: {stderr}"
    );

    // Without --progress stderr has no progress lines
    let plain_output = sandbox.run(&["run", "hi", "--agent", "fake", "--json"]);
    assert_eq!(plain_output.status.code(), Some(0));
    let plain_stderr = String::from_utf8_lossy(&plain_output.stderr);
    for line in plain_stderr.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            assert_ne!(
                v["type"], "progress",
                "unexpected progress line without --progress: {line}"
            );
        }
    }
}

// TC-763 / FR-762: todo add, list and run without a TUI.
// The fixture is a POSIX shell script (same as src/agent/custom.rs tests).
#[cfg(unix)]
#[test]
fn todo_cli_add_list_run() {
    let sandbox = Sandbox::new(&fake_agent_config("jsonl", None));
    let added = sandbox.run(&[
        "todo",
        "add",
        "Write",
        "the",
        "docs",
        "--body",
        "API section",
        "--json",
    ]);
    assert_eq!(added.status.code(), Some(0));
    let todo = stdout_json(&added);
    assert_eq!(todo["title"], "Write the docs");
    assert_eq!(todo["body"], "API section");
    assert_eq!(todo["status"], "open");

    // The fake agent echoes its prompt inside a JSON string, so the to-do it
    // runs has a single-line prompt (no body).
    let runnable = stdout_json(&sandbox.run(&["todo", "add", "Echo", "me", "--json"]));
    let id = runnable["id"].as_str().unwrap().to_owned();

    let listed = sandbox.run(&["todo", "list", "--status", "open", "--json"]);
    assert_eq!(listed.status.code(), Some(0));
    let list = stdout_json(&listed);
    assert_eq!(list.as_array().unwrap().len(), 2);

    let missing = sandbox.run(&["todo", "run", "no-such-id", "--agent", "fake", "--json"]);
    assert_eq!(
        missing.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&missing.stdout)
    );

    let empty = sandbox.run(&["todo", "add", "--json"]);
    assert_eq!(empty.status.code(), Some(2));

    if cfg!(unix) {
        let ran = sandbox.run(&["todo", "run", &id, "--agent", "fake", "--json"]);
        let json = stdout_json(&ran);
        assert_eq!(ran.status.code(), Some(0), "{json}");
        assert_eq!(json["todo"]["status"], "done", "{json}");
        let done = stdout_json(&sandbox.run(&["todo", "list", "--status", "done", "--json"]));
        assert_eq!(done.as_array().unwrap().len(), 1);
    }
}

// TC-764 / FR-763: status JSON carries every contract key.
#[test]
fn status_json_has_contract_keys() {
    let sandbox = Sandbox::new(&fake_agent_config("jsonl", Some(r#""fake", "agy""#)));
    let output = sandbox.run(&["status", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let json = stdout_json(&output);
    for key in [
        "workspace",
        "version",
        "agents",
        "allowed_runners",
        "graph_enabled",
        "todos",
        "last_graph",
        "events",
        "dashboard",
        "warnings",
    ] {
        assert!(json.get(key).is_some(), "missing {key}: {json}");
    }
    let agents = json["agents"].as_array().unwrap();
    let fake = agents.iter().find(|a| a["id"] == "fake").unwrap();
    assert_eq!(fake["allowed"], true);
    let cursor = agents.iter().find(|a| a["id"] == "cursor").unwrap();
    assert_eq!(cursor["allowed"], false);
    assert_eq!(json["events"]["last_seq"], 0);
    assert!(json["dashboard"].is_null());
}

// TC-765 / FR-764, NFR-760: events --since prints later events as JSON lines
// and skips malformed lines.
#[test]
fn events_cli_since_json() {
    let sandbox = Sandbox::new("[graph]\nenabled = false\n");
    let dir = sandbox.workspace.join(".dmuxai");
    std::fs::create_dir_all(&dir).unwrap();
    let line = |seq: u64, to: &str| {
        format!(
            r#"{{"v":1,"seq":{seq},"ts":"2026-10-17T00:00:00.000Z","ts_ms":0,"workspace":"/w","kind":"agent","task":null,"todo":null,"node":null,"runner":"fake","model":null,"from_state":null,"to_state":"{to}","message":null}}"#
        )
    };
    let contents = format!(
        "{}\n{}\nnot json\n{}\n",
        line(1, "pending"),
        line(2, "started"),
        line(3, "completed")
    );
    std::fs::write(dir.join("events.jsonl"), contents).unwrap();

    let output = sandbox.run(&["events", "--since", "1", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let seqs: Vec<u64> = stdout
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["seq"]
                .as_u64()
                .unwrap()
        })
        .collect();
    assert_eq!(seqs, [2, 3]);

    let limited = sandbox.run(&["events", "--limit", "1", "--json"]);
    assert_eq!(String::from_utf8_lossy(&limited.stdout).lines().count(), 1);

    let status = stdout_json(&sandbox.run(&["status", "--json"]));
    assert_eq!(status["events"]["last_seq"], 3);
}

// TC-811 / FR-810, FR-813: a fresh install (no dmuxai.toml) with opencode on
// PATH lists the built-in opencode runner, and it is not in the graph runner
// set while allowed_runners is unset. Without opencode it is not listed.
#[cfg(unix)]
#[test]
fn fresh_install_lists_opencode() {
    let sandbox = Sandbox::bare();
    let bin = fake_opencode(&sandbox.root);
    let with_opencode = [("PATH", path_with(&bin))];
    let output = sandbox.run_in_env(&["status", "--json"], None, &with_opencode);
    assert_eq!(output.status.code(), Some(0));
    let json = stdout_json(&output);
    assert_eq!(json["allowed_runners"], Value::Null, "{json}");
    let agents = json["agents"].as_array().unwrap();
    let opencode = agents
        .iter()
        .find(|agent| agent["id"] == "opencode")
        .unwrap_or_else(|| panic!("opencode missing: {json}"));
    assert_eq!(opencode["allowed"], false, "{json}");
    let cursor = agents.iter().find(|agent| agent["id"] == "cursor").unwrap();
    assert_eq!(cursor["allowed"], true);

    let empty_path = [("PATH", OsString::new())];
    let json = stdout_json(&sandbox.run_in_env(&["status", "--json"], None, &empty_path));
    let agents = json["agents"].as_array().unwrap();
    assert!(
        agents.iter().all(|agent| agent["id"] != "opencode"),
        "{json}"
    );
}

// TC-891 / FR-890, FR-899: a fresh install with grok on PATH lists it and marks
// it not graph-allowed. An empty PATH and a home without ~/.grok/bin/grok drops it.
#[cfg(unix)]
#[test]
fn fresh_install_lists_grok() {
    let sandbox = Sandbox::bare();
    let bin = fake_grok(&sandbox.root);
    let with_grok = [("PATH", path_with(&bin))];
    let output = sandbox.run_in_env(&["status", "--json"], None, &with_grok);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = stdout_json(&output);
    let agents = json["agents"].as_array().unwrap();
    let grok = agents
        .iter()
        .find(|agent| agent["id"] == "grok")
        .unwrap_or_else(|| panic!("grok missing: {json}"));
    assert_eq!(grok["allowed"], false, "{json}");
    let cursor = agents.iter().find(|agent| agent["id"] == "cursor").unwrap();
    assert_eq!(cursor["allowed"], true);

    let empty_path = [("PATH", OsString::new())];
    let json = stdout_json(&sandbox.run_in_env(&["status", "--json"], None, &empty_path));
    let agents = json["agents"].as_array().unwrap();
    assert!(agents.iter().all(|agent| agent["id"] != "grok"), "{json}");
}

/// Writes a fake `grok` executable. Registration only checks that the file exists.
#[cfg(unix)]
fn fake_grok(root: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("grok");
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

// TC-813 / FR-814: `init` writes the starter file, the file parses cleanly, a
// second run is refused without --force, and --workspace writes the workspace.
#[cfg(unix)]
#[test]
fn init_writes_starter_config() {
    let sandbox = Sandbox::bare();
    let bin = fake_opencode(&sandbox.root);
    let env = [("PATH", path_with(&bin))];
    let user_config = sandbox.root.join("cfg").join("dmuxai").join("dmuxai.toml");

    let first = sandbox.run_in_env(&["init"], None, &env);
    let stderr = String::from_utf8_lossy(&first.stderr).into_owned();
    assert_eq!(first.status.code(), Some(0), "{stderr}");
    let stdout = String::from_utf8_lossy(&first.stdout).into_owned();
    assert!(
        stdout.contains(&user_config.display().to_string()),
        "{stdout}"
    );
    assert!(stdout.contains("opencode"), "detected runners: {stdout}");

    let text = std::fs::read_to_string(&user_config).unwrap();
    for key in [
        "[theme]",
        "[routing]",
        "[graph]",
        "[cursor]",
        "[agents]",
        "builtin_opencode",
        "# [[agents.custom]]",
    ] {
        assert!(text.contains(key), "missing {key}");
    }
    // The starter parses cleanly: no config warnings, and the built-in runner
    // is still registered from PATH.
    let status = stdout_json(&sandbox.run_in_env(&["status", "--json"], None, &env));
    assert_eq!(status["warnings"], serde_json::json!([]), "{status}");
    let listed = status["agents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|agent| agent["id"] == "opencode");
    assert!(listed, "{status}");

    let again = sandbox.run_in_env(&["init"], None, &env);
    assert_ne!(again.status.code(), Some(0));
    let refusal = String::from_utf8_lossy(&again.stderr);
    assert!(refusal.contains("--force"), "{refusal}");

    let forced = sandbox.run_in_env(&["init", "--force"], None, &env);
    assert_eq!(forced.status.code(), Some(0));

    let workspace = sandbox.run_in_env(&["init", "--workspace"], None, &env);
    assert_eq!(workspace.status.code(), Some(0));
    assert!(sandbox.workspace.join("dmuxai.toml").is_file());
}

// TC-1675 / FR-1675: --config and DMUXAI_CONFIG select the config file.
#[cfg(unix)]
#[test]
fn tc_1675_config_flag_and_env_select_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let flag_cfg = root.join("flag.toml");
    let env_cfg = root.join("env.toml");
    std::fs::write(
        &flag_cfg,
        r#"[[agents.custom]]
name = "fakeflag"
command = ["sh", "-c", "cat >/dev/null; echo ok"]
prompt_via = "stdin"
"#,
    )
    .unwrap();
    std::fs::write(
        &env_cfg,
        r#"[[agents.custom]]
name = "fakeenv"
command = ["sh", "-c", "cat >/dev/null; echo ok"]
prompt_via = "stdin"
"#,
    )
    .unwrap();

    // Each sub-case uses a fresh sandbox (workspace and state dir).
    // Reusing state across configs that define different agents triggers a known bug
    // (#145: restoring conversation referencing a no-longer-configured custom agent panics).

    // Sub-case 1: --config flag selects the file (XDG_CONFIG_HOME stays empty).
    let sandbox_flag = Sandbox::bare();
    let out = sandbox_flag.run_in_env(
        &[
            "--config",
            flag_cfg.to_str().unwrap(),
            "run",
            "--agent",
            "fakeflag",
            "--json",
            "hi",
        ],
        None,
        &[("DMUXAI_CONFIG", std::ffi::OsString::new())],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let json = stdout_json(&out);
    assert_eq!(json["agent"], "fakeflag");
    assert!(json["text"].as_str().unwrap().contains("ok"));

    // Sub-case 2: DMUXAI_CONFIG env var alone selects the file.
    let sandbox_env = Sandbox::bare();
    let out = sandbox_env.run_in_env(
        &["run", "--agent", "fakeenv", "--json", "hi"],
        None,
        &[("DMUXAI_CONFIG", env_cfg.clone().into_os_string())],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let json = stdout_json(&out);
    assert_eq!(json["agent"], "fakeenv");
    assert!(json["text"].as_str().unwrap().contains("ok"));

    // Sub-case 3: --config wins over DMUXAI_CONFIG when both are set.
    let sandbox_both = Sandbox::bare();
    let out = sandbox_both.run_in_env(
        &[
            "--config",
            flag_cfg.to_str().unwrap(),
            "run",
            "--agent",
            "fakeflag",
            "--json",
            "hi",
        ],
        None,
        &[("DMUXAI_CONFIG", env_cfg.into_os_string())],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let json = stdout_json(&out);
    assert_eq!(json["agent"], "fakeflag");
}

// TC-1676 / FR-1676: missing config file exits 2.
#[cfg(unix)]
#[test]
fn tc_1676_missing_config_file_exits_2() {
    let sandbox = Sandbox::bare();

    let out = sandbox.run_in_env(
        &["--config", "/nonexistent/x.toml", "status"],
        None,
        &[("DMUXAI_CONFIG", std::ffi::OsString::new())],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("config file not found"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = sandbox.run_in_env(
        &["status"],
        None,
        &[(
            "DMUXAI_CONFIG",
            std::ffi::OsString::from("/nonexistent/x.toml"),
        )],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("config file not found"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// TC-1677 / FR-1677: child env policy snapshot.
#[cfg(unix)]
#[test]
fn tc_1677_child_env_policy_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cfg = root.join("env.toml");
    std::fs::write(
        &cfg,
        r#"[[agents.custom]]
name = "envprobe"
command = ["sh", "-c", "env"]
"#,
    )
    .unwrap();
    let xdg = root.join("xdg");
    let home = root.join("home");
    std::fs::create_dir_all(&xdg).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    let sandbox = Sandbox::bare();
    let out = sandbox.run_in_env(
        &[
            "--config",
            cfg.to_str().unwrap(),
            "run",
            "--agent",
            "envprobe",
            "--json",
            "hi",
        ],
        None,
        &[
            ("DMUXAI_CONFIG", std::ffi::OsString::new()),
            ("DMUXAI_WEB_TOKEN", std::ffi::OsString::from("secret-x")),
            (
                "DMUXAI_MCP_CONFIG",
                std::ffi::OsString::from("/tmp/leak.json"),
            ),
            ("XDG_CONFIG_HOME", xdg.clone().into_os_string()),
            ("HOME", home.clone().into_os_string()),
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json = stdout_json(&out);
    let text = json["text"].as_str().unwrap();
    assert!(!text.contains("DMUXAI_WEB_TOKEN"), "{text}");
    assert!(!text.contains("DMUXAI_MCP_CONFIG"), "{text}");
    assert!(!text.contains("DMUXAI_CONFIG"), "{text}");
    assert!(
        text.contains(&format!("XDG_CONFIG_HOME={}", xdg.display())),
        "{text}"
    );
    assert!(text.contains(&format!("HOME={}", home.display())), "{text}");
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    let alive = std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !alive {
        return false;
    }
    #[cfg(target_os = "linux")]
    if let Ok(content) = std::fs::read_to_string(format!("/proc/{pid}/status"))
        && content.lines().any(|l| l.starts_with("State:\tZ"))
    {
        return false;
    }
    true
}

#[cfg(unix)]
fn assert_stop_signal_cancels_run(signal: &str) {
    use std::io::Read;
    use std::time::{Duration, Instant};

    let sandbox = Sandbox::new(&fake_agent_config("pidsleep", None));
    let pidfile = sandbox.root.join("pids.txt");
    let env_refs: Vec<(&str, OsString)> =
        vec![("FAKE_AGENT_PIDFILE", pidfile.as_os_str().to_os_string())];
    let mut child = sandbox.spawn_in_env(&["run", "hello", "--agent", "fake", "--json"], &env_refs);
    let stdout_pipe = child.stdout.take().unwrap();
    let dmuxai_pid = child.id();

    // Poll until pidfile has 2 pids.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut pids: Vec<u32> = Vec::new();
    while Instant::now() < deadline {
        if let Ok(content) = std::fs::read_to_string(&pidfile) {
            let parsed: Vec<u32> = content
                .lines()
                .filter_map(|l| l.trim().parse::<u32>().ok())
                .collect();
            if parsed.len() >= 2 {
                pids = parsed;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(pids.len() >= 2, "pidfile did not contain 2 pids: {pids:?}");

    // Send signal.
    let _ = std::process::Command::new("kill")
        .arg(signal)
        .arg(dmuxai_pid.to_string())
        .status();

    // Wait with 30s deadline.
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() < Duration::from_secs(30) => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("dmuxai did not exit within 30s");
            }
            Err(e) => panic!("wait error: {e}"),
        }
    };
    assert_eq!(status.code(), Some(130));

    // Read stdout.
    let mut buf = String::new();
    let mut stdout_pipe = stdout_pipe;
    let _ = stdout_pipe.read_to_string(&mut buf);
    let json: Value = serde_json::from_str(buf.trim())
        .unwrap_or_else(|e| panic!("stdout not one JSON object ({e}): {buf}"));
    assert_eq!(json["exit_code"], 130);

    // events.jsonl contains cancelled.
    let events_path = sandbox.workspace.join(".dmuxai").join("events.jsonl");
    let mut saw_cancelled = false;
    if let Ok(content) = std::fs::read_to_string(&events_path) {
        for line in content.lines() {
            if let Ok(v) = serde_json::from_str::<Value>(line)
                && (v["to_state"] == "cancelled" || v["kind"] == "cancelled")
            {
                saw_cancelled = true;
            }
        }
    }
    assert!(saw_cancelled, "no cancelled event in events.jsonl");

    // Both pids gone within 5s.
    for pid in pids {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !pid_alive(pid) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!pid_alive(pid), "pid {pid} still alive");
    }
}

#[cfg(unix)]
#[test]
fn tc_1790_sigterm_cancels_headless_run_and_reaps_runner() {
    assert_stop_signal_cancels_run("-TERM");
}

#[cfg(unix)]
#[test]
fn tc_1791_sighup_cancels_headless_run_and_reaps_runner() {
    assert_stop_signal_cancels_run("-HUP");
}
