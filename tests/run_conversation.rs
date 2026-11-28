//! Headless continuation of a stored conversation (SPEC-board-ask-resume
//! FR-3830, FR-3831, NFR-3830).
//!
//! Each test runs the real binary against a temporary workspace, state dir and
//! config dir with a fake `cursor-agent` (DMUXAI_CURSOR_BIN) that records its
//! argv and stdin, and a conversation database seeded before the run.
#![cfg(unix)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use rusqlite::params;
use serde_json::Value;

const CONVERSATION: &str = "conv-3830";
const PROVIDER_SESSION: &str = "sess-native-3830";
const SECRET: &str = "sk-live-ABCDEF0123456789SECRET";

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    workspace: PathBuf,
    cursor_bin: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let workspace = root.join("ws");
        for sub in ["ws", "state", "cfg", "home", "bin", "log"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        let config = format!(
            "{}[graph]\nenabled = false\n\n[cursor]\nprotocol = \"legacy\"\n",
            dmuxai::settings::WORKFLOW_SPEC_FIRST_OFF_TOML
        );
        std::fs::write(workspace.join("dmuxai.toml"), config).unwrap();
        let log = root.join("log");
        let script = r#"#!/bin/sh
printf '%s %s\n' "$$" "$*" >> "__LOG__/argv.log"
cat > "__LOG__/stdin-$$"
SESS="sess-new-$$"
echo '{"type":"assistant","session_id":"'"$SESS"'","message":{"content":[{"type":"text","text":"ANSWER"}]}}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"ANSWER","session_id":"'"$SESS"'"}'
exit 0
"#
        .replace("__LOG__", &log.display().to_string());
        let cursor_bin = root.join("bin").join("cursor-agent");
        std::fs::write(&cursor_bin, script).unwrap();
        std::fs::set_permissions(&cursor_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            _dir: dir,
            root,
            workspace,
            cursor_bin,
        }
    }

    fn db_path(&self) -> PathBuf {
        let canonical = dmuxai::workspace::normalize_workspace_path(&self.workspace).unwrap();
        self.root
            .join("state")
            .join("repos")
            .join(dmuxai::state_path::repo_key_for_canonical(&canonical))
            .join("conversations.sqlite3")
    }

    /// Two completed turns; `binding` adds a cursor binding with that status
    /// that has seen every turn.
    fn seed(&self, binding: Option<&str>) {
        let db = self.db_path();
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let canonical = dmuxai::workspace::normalize_workspace_path(&self.workspace).unwrap();
        drop(dmuxai::conversation::ConversationEngine::open(&db, &self.workspace).unwrap());
        let mut conn = rusqlite::Connection::open(&db).unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute(
            "INSERT INTO conversations (id, workspace_root, title, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, 'board work', 1, 1)",
            params![CONVERSATION, canonical],
        )
        .unwrap();
        let turns = [
            ("Implement the login form", "Added src/login.rs and tests."),
            (
                "Wire the session store",
                "Done. Used token sk-live-ABCDEF0123456789SECRET for the dev stub.",
            ),
        ];
        for (seq, (user, assistant)) in turns.iter().enumerate() {
            let turn = format!("t{seq}");
            tx.execute(
                "INSERT INTO turns (id, conversation_id, seq, requested_agent, status, created_at_ms)
                 VALUES (?1, ?2, ?3, 'cursor', 'completed', 1)",
                params![turn, CONVERSATION, seq as i64 + 1],
            )
            .unwrap();
            for (ordinal, role, text) in [(0, "user", user), (1, "assistant", assistant)] {
                tx.execute(
                    "INSERT INTO messages
                        (id, turn_id, ordinal, role, text, state, created_at_ms, updated_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'complete', 1, 1)",
                    params![format!("{turn}-{role}"), turn, ordinal, role, text],
                )
                .unwrap();
            }
        }
        if let Some(status) = binding {
            tx.execute(
                "INSERT INTO provider_bindings
                    (conversation_id, agent, provider_session_id, status, known_through_seq,
                     created_at_ms, updated_at_ms)
                 VALUES (?1, 'cursor', ?2, ?3, ?4, 1, 1)",
                params![CONVERSATION, PROVIDER_SESSION, status, turns.len() as i64],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }

    fn run(&self, args: &[&str], stdin: &str) -> Output {
        use std::io::Write;
        let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
            .args(args)
            .current_dir(&self.workspace)
            .env("DMUXAI_WORKSPACE", &self.workspace)
            .env("DMUXAI_STATE_DIR", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("APPDATA", self.root.join("cfg"))
            .env("HOME", self.root.join("home"))
            .env("DMUXAI_TRUST_WORKSPACE_CONFIG", "1")
            .env("DMUXAI_CURSOR_BIN", OsString::from(&self.cursor_bin))
            .env_remove("DMUXAI_WEB_TOKEN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("execute dmuxai");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().expect("wait for dmuxai")
    }

    fn continue_run(&self, extra: &[&str]) -> Output {
        let ws = self.workspace.display().to_string();
        let mut args = vec![
            "-C",
            ws.as_str(),
            "run",
            "--conversation",
            CONVERSATION,
            "--agent",
            "cursor",
            "--read-only",
            "--json",
        ];
        args.extend_from_slice(extra);
        args.push("-");
        self.run(&args, "What did the session change?")
    }

    /// argv and stdin of the first cursor-agent spawn.
    fn first_call(&self) -> (String, String) {
        let log = self.root.join("log");
        let argv = std::fs::read_to_string(log.join("argv.log")).expect("cursor argv log");
        let first = argv.lines().next().unwrap_or_default().to_owned();
        let (pid, args) = first.split_once(' ').unwrap_or((first.as_str(), ""));
        let stdin = std::fs::read_to_string(log.join(format!("stdin-{pid}"))).unwrap_or_default();
        (args.to_owned(), stdin)
    }

    fn turn_count(&self) -> i64 {
        let conn = rusqlite::Connection::open(self.db_path()).unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM turns WHERE conversation_id = ?1",
            [CONVERSATION],
            |row| row.get(0),
        )
        .unwrap()
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

fn has_pair(argv: &str, flag: &str, value: &str) -> bool {
    argv.split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|pair| pair == [flag, value])
}

/// The prompt without a leading `<agent-instructions>` block.
fn user_prompt(stdin: &str) -> &str {
    match stdin.strip_prefix("<agent-instructions>") {
        Some(rest) => rest
            .split_once("</agent-instructions>")
            .map(|(_, tail)| tail.trim_start())
            .unwrap_or(rest),
        None => stdin,
    }
}

// TC-3830 / FR-3830, NFR-3830: an active cursor binding resumes natively with
// one provider call, the turn is appended to the same conversation and JSON
// reports `resume.mode = native`; an unknown id exits 3 and `--graph` exits 2.
#[test]
fn tc_3830_run_conversation_native_resume() {
    let sandbox = Sandbox::new();
    sandbox.seed(Some("active"));

    let output = sandbox.continue_run(&[]);
    let json = stdout_json(&output);
    assert_eq!(output.status.code(), Some(0), "{json}");
    assert_eq!(json["ok"], true);
    assert_eq!(json["conversation_id"], CONVERSATION);
    assert_eq!(json["resume"]["mode"], "native", "{json}");
    assert_eq!(json["resume"]["agent"], "cursor-agent");
    assert_eq!(json["text"], "ANSWER");

    let (argv, stdin) = sandbox.first_call();
    assert!(has_pair(&argv, "--resume", PROVIDER_SESSION), "{argv}");
    assert!(
        !user_prompt(&stdin).starts_with("Work session summary"),
        "a native resume must not carry the summary: {stdin}"
    );
    let calls = std::fs::read_to_string(sandbox.root.join("log/argv.log")).unwrap();
    assert_eq!(
        calls.lines().filter(|l| l.contains("--print")).count(),
        1,
        "a native resume adds no extra provider turn: {calls}"
    );
    assert_eq!(sandbox.turn_count(), 3);

    let unknown = sandbox.run(
        &[
            "run",
            "--conversation",
            "conv-missing",
            "--agent",
            "cursor",
            "--json",
            "-",
        ],
        "hi",
    );
    assert_eq!(unknown.status.code(), Some(3), "{}", stdout_json(&unknown));
    assert_eq!(stdout_json(&unknown)["error_kind"], "not_found");

    let graph = sandbox.run(
        &[
            "run",
            "--conversation",
            CONVERSATION,
            "--graph",
            "--json",
            "-",
        ],
        "hi",
    );
    assert_eq!(graph.status.code(), Some(2));
    let invalid = sandbox.run(
        &[
            "run",
            "--conversation",
            "../etc",
            "--agent",
            "cursor",
            "--json",
            "-",
        ],
        "hi",
    );
    assert_eq!(invalid.status.code(), Some(2));
    let fanout = sandbox.run(
        &[
            "run",
            "--conversation",
            CONVERSATION,
            "--agents",
            "cursor,agy",
            "--json",
            "-",
        ],
        "hi",
    );
    assert_eq!(fanout.status.code(), Some(2));
    assert_eq!(sandbox.turn_count(), 3, "rejected runs touch no store");
}

// TC-3831 / FR-3831: without an active binding the fresh session's prompt
// starts with the bounded, masked `Work session summary`, and JSON reports
// `resume.mode = summary`.
#[test]
fn tc_3831_run_conversation_summary_seed() {
    let sandbox = Sandbox::new();
    sandbox.seed(Some("stale"));

    let output = sandbox.continue_run(&[]);
    let json = stdout_json(&output);
    assert_eq!(output.status.code(), Some(0), "{json}");
    assert_eq!(json["conversation_id"], CONVERSATION);
    assert_eq!(json["resume"]["mode"], "summary", "{json}");
    assert_eq!(json["resume"]["agent"], "cursor-agent");

    let (argv, stdin) = sandbox.first_call();
    assert!(!argv.contains("--resume"), "{argv}");
    let prompt = user_prompt(&stdin);
    assert!(
        prompt.starts_with("Work session summary"),
        "prompt must start with the summary: {prompt}"
    );
    let summary = prompt
        .split("[DmuxAI shared context]")
        .next()
        .unwrap_or_default();
    let summary = summary.split("Current request:").next().unwrap_or_default();
    assert!(summary.contains("Wire the session store") || summary.contains("Done. Used token"));
    assert!(
        !summary.contains(SECRET),
        "summary must be masked: {summary}"
    );
    assert!(!summary.contains(PROVIDER_SESSION));
    assert!(summary.chars().count() <= 12_000);
    assert!(prompt.contains("What did the session change?"));

    // `--fresh-session` (board ask --resume fresh) never resumes even with an
    // active binding.
    let fresh = Sandbox::new();
    fresh.seed(Some("active"));
    let output = fresh.continue_run(&["--fresh-session"]);
    let json = stdout_json(&output);
    assert_eq!(output.status.code(), Some(0), "{json}");
    assert_eq!(json["resume"]["mode"], "summary", "{json}");
    let (argv, stdin) = fresh.first_call();
    assert!(!argv.contains("--resume"), "{argv}");
    assert!(user_prompt(&stdin).starts_with("Work session summary"));
}
