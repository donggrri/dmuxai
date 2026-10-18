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
        let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
            .args(args)
            .current_dir(&self.workspace)
            .env("DMUXAI_WORKSPACE", &self.workspace)
            .env("DMUXAI_STATE_DIR", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("APPDATA", self.root.join("cfg"))
            .env("HOME", self.root.join("home"))
            .env_remove("DMUXAI_WEB_TOKEN")
            .env_remove("DMUXAI_OPENCODE_BIN")
            .env_remove("DMUXAI_GROK_BIN")
            .envs(env.iter().map(|(key, value)| (*key, value.clone())))
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
