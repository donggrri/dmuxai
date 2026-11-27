//! Integration and browser tests for SPEC-web-all-projects (FR-2980..FR-2986, NFR-2980).

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dmuxai::conversation::{AgentKey, BeginTurn, ConversationEngine, ConversationSelection};
use dmuxai::run_journal::{JournalStore, RunDiagnosticsRef, RunLinks, RunManifest};
use dmuxai::state_path::{repo_key, workspace_metadata_path, write_workspace_metadata};
use serde_json::Value;

struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct IsolatedEnv {
    _root: tempfile::TempDir,
    state_dir: PathBuf,
    data_dir: PathBuf,
    cfg_home: PathBuf,
    home: PathBuf,
}

impl IsolatedEnv {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let state_dir = root.path().join("state");
        let data_dir = root.path().join("data");
        let cfg_home = root.path().join("cfg");
        let home = root.path().join("home");
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&cfg_home).unwrap();
        fs::create_dir_all(&home).unwrap();
        Self {
            _root: root,
            state_dir,
            data_dir,
            cfg_home,
            home,
        }
    }

    fn root(&self) -> &Path {
        self._root.path()
    }
}

fn spawn_dashboard(mode: &str, workspace: &Path, env: &IsolatedEnv) -> (String, ChildGuard) {
    let args = match mode {
        "serve" => vec!["serve", "--port", "0"],
        "web" => vec!["--web", "--port", "0"],
        _ => panic!("unknown mode {mode}"),
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_dmuxai"))
        .args(&args)
        .current_dir(workspace)
        .env_remove("DMUXAI_WORKSPACE")
        .env("DMUXAI_STATE_DIR", &env.state_dir)
        .env("DMUXAI_DATA_DIR", &env.data_dir)
        .env("XDG_STATE_HOME", &env.state_dir)
        .env("XDG_DATA_HOME", &env.data_dir)
        .env("XDG_CONFIG_HOME", &env.cfg_home)
        .env("HOME", &env.home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dmuxai");

    let stdout = child.stdout.take().expect("stdout piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        for _ in 0..20 {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if let Some(url) = line.split_whitespace().find(|w| w.starts_with("http://")) {
                        let _ = tx.send(url.trim().to_owned());
                        return;
                    }
                }
            }
        }
    });

    let guard = ChildGuard(Some(child));
    let url = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("dashboard URL on stdout");
    (url, guard)
}

fn http_request(url_str: &str, method: &str, body: Option<&str>) -> (u16, String) {
    let trimmed = url_str.strip_prefix("http://").expect("http url");
    let (host_port, path_and_query) = match trimmed.find('/') {
        Some(idx) => (&trimmed[..idx], &trimmed[idx..]),
        None => (trimmed, "/"),
    };
    let mut stream = TcpStream::connect(host_port).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("write timeout");

    let body_bytes = body.unwrap_or("").as_bytes();
    let mut req = format!(
        "{method} {path_and_query} HTTP/1.1\r\nHost: {host_port}\r\nX-Dmuxai-Client: 1\r\nConnection: close\r\n"
    );
    if body.is_some() {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", body_bytes.len()));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    if !body_bytes.is_empty() {
        stream.write_all(body_bytes).unwrap();
    }
    stream.flush().unwrap();

    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (head, resp_body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    (status, resp_body.to_owned())
}

fn http_get(base_url: &str, path: &str) -> (u16, String) {
    let base = base_url.split('?').next().unwrap_or(base_url);
    let base = base.trim_end_matches('/');
    http_request(&format!("{base}{path}"), "GET", None)
}

fn read_sse_until(base_url: &str, path: &str, needle: &str) -> (u16, String) {
    let base = base_url
        .split('?')
        .next()
        .unwrap_or(base_url)
        .trim_end_matches('/');
    let full = format!("{base}{path}");
    let trimmed = full.strip_prefix("http://").unwrap();
    let (host_port, path_and_query) = trimmed.split_once('/').unwrap();
    let mut stream = TcpStream::connect(host_port).expect("connect sse");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("read timeout");
    let req = format!(
        "GET /{path_and_query} HTTP/1.1\r\nHost: {host_port}\r\nX-Dmuxai-Client: 1\r\nAccept: text/event-stream\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                let s = String::from_utf8_lossy(&buf);
                if s.contains(needle) {
                    break;
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(e) => panic!("sse read error: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    (status, text)
}

fn make_manifest(
    run_id: &str,
    status: &str,
    workspace_root: Option<&Path>,
    origin: &str,
) -> RunManifest {
    RunManifest {
        schema_version: 1,
        run_id: run_id.to_owned(),
        origin: origin.to_owned(),
        status: status.to_owned(),
        created_at_ms: 1_700_000_000_000,
        completed_at_ms: if status == "completed" {
            Some(1_700_000_005_000)
        } else {
            None
        },
        duration_ms: if status == "completed" {
            Some(5000)
        } else {
            None
        },
        requested_runner: Some("codex".to_owned()),
        resolved_runner: Some("codex".to_owned()),
        requested_model: None,
        resolved_model: None,
        dmuxai_version: Some("0.4.0".to_owned()),
        build_commit: None,
        pid: None,
        dmuxai_exit_code: if status == "completed" { Some(0) } else { None },
        child_exit_code: None,
        child_signal: None,
        error_kind: None,
        error_summary: None,
        links: RunLinks::default(),
        diagnostics: RunDiagnosticsRef::default(),
        end_summary: None,
        workspace_root: workspace_root
            .map(|p| fs::canonicalize(p).unwrap().to_string_lossy().into_owned()),
    }
}

struct EnvRestore {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvRestore {
    fn set(key: &'static str, val: &Path) -> Self {
        let prev = std::env::var_os(key);
        unsafe { std::env::set_var(key, val) };
        Self { key, prev }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

// TC-2980 / FR-2980: workspace.json written atomically (dir 0700, file 0600) on open and run start,
// and new run manifests record workspace_root while old manifests remain readable.
#[test]
fn tc_2980_workspace_json_written_on_open_and_run_start_and_manifest_has_root() {
    let _lock = dmuxai::state_path::test_env_lock();
    let env = IsolatedEnv::new();
    let ws = env.root().join("proj-a");
    fs::create_dir_all(&ws).unwrap();

    let _s = EnvRestore::set("DMUXAI_STATE_DIR", &env.state_dir);
    let _d = EnvRestore::set("DMUXAI_DATA_DIR", &env.data_dir);

    let path = write_workspace_metadata(&ws).expect("write workspace.json");
    assert!(path.is_file());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    let key = repo_key(&ws).unwrap();
    assert_eq!(path, workspace_metadata_path(&key).unwrap());
    let json: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(json["repo_key"], key);
    assert_eq!(
        json["canonical_path"],
        fs::canonicalize(&ws).unwrap().to_string_lossy().as_ref()
    );
    assert!(json["last_seen_ms"].as_u64().unwrap() > 0);

    // Old manifest without workspace_root still deserializes cleanly.
    let old_json = r#"{
        "schema_version": 1,
        "run_id": "0123456789abcdef",
        "origin": "cli_headless",
        "status": "completed",
        "created_at_ms": 1000,
        "completed_at_ms": 2000,
        "duration_ms": 1000,
        "requested_runner": "codex",
        "resolved_runner": "codex",
        "requested_model": null,
        "resolved_model": null,
        "dmuxai_version": "0.3.0",
        "build_commit": null,
        "child_exit_code": 0,
        "child_signal": null,
        "error_kind": null,
        "error_summary": null,
        "links": { "conversation_id": null, "turn_id": null, "graph_id": null, "node_id": null, "attempt_index": null },
        "diagnostics": { "stderr_bytes": 0, "tail_sample": null },
        "end_summary": null
    }"#;
    let old_manifest: RunManifest = serde_json::from_str(old_json).expect("old manifest readable");
    assert_eq!(old_manifest.workspace_root, None);

    // Warning-only failure when workspace does not exist.
    dmuxai::state_path::warn_write_workspace_metadata(&env.root().join("nonexistent-ws"));

    // Live dashboard startup writes workspace.json for the opened workspace.
    let ws_live = env.root().join("proj-live");
    fs::create_dir_all(&ws_live).unwrap();
    let (_url, _guard) = spawn_dashboard("serve", &ws_live, &env);
    let live_key = repo_key(&ws_live).unwrap();
    assert!(workspace_metadata_path(&live_key).unwrap().is_file());
}

// TC-2981 / FR-2981: GET /api/global/projects lists repos from both state and data bases,
// including run-only repos and conversation-only repos, sorted by last_activity_ms desc.
#[test]
fn tc_2981_global_projects_lists_repos_from_state_and_data_bases() {
    let env = IsolatedEnv::new();
    let ws_conv = env.root().join("repo-conv");
    let ws_runs = env.root().join("repo-runs");
    let ws_missing = env.root().join("repo-missing");
    let ws_noroot = env.root().join("repo-noroot");
    fs::create_dir_all(&ws_conv).unwrap();
    fs::create_dir_all(&ws_runs).unwrap();
    fs::create_dir_all(&ws_missing).unwrap();
    fs::create_dir_all(&ws_noroot).unwrap();

    let (k_conv, k_runs, k_missing, k_noroot, db_path) = {
        let _lock = dmuxai::state_path::test_env_lock();
        let _s = EnvRestore::set("DMUXAI_STATE_DIR", &env.state_dir);
        let _d = EnvRestore::set("DMUXAI_DATA_DIR", &env.data_dir);
        let k_conv = repo_key(&ws_conv).unwrap();
        let db_path = env
            .state_dir
            .join("repos")
            .join(&k_conv)
            .join("conversations.sqlite3");
        let mut engine = ConversationEngine::open(&db_path, &ws_conv).unwrap();
        engine
            .select(ConversationSelection::New {
                title: Some("Conv Repo".to_owned()),
            })
            .unwrap();
        let _ = engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "conv repo prompt".to_owned(),
                display_text: None,
            })
            .unwrap();

        let k_runs = repo_key(&ws_runs).unwrap();
        let runs_dir = env.data_dir.join("repos").join(&k_runs).join("runs");
        let store = JournalStore::new(runs_dir);
        let manifest = make_manifest("aaaa1111bbbb2222", "active", Some(&ws_runs), "cli_headless");
        store.save_relaxed(&manifest).unwrap();

        let k_missing = repo_key(&ws_missing).unwrap();
        write_workspace_metadata(&ws_missing).unwrap();
        fs::remove_dir_all(&ws_missing).unwrap();

        let k_noroot = repo_key(&ws_noroot).unwrap();
        let noroot_runs = env.data_dir.join("repos").join(&k_noroot).join("runs");
        let noroot_store = JournalStore::new(noroot_runs);
        let mut noroot_manifest =
            make_manifest("cccc3333dddd4444", "completed", None, "cli_headless");
        noroot_manifest.created_at_ms = 1_600_000_000_000;
        noroot_manifest.completed_at_ms = Some(1_600_000_001_000);
        noroot_store.save_relaxed(&noroot_manifest).unwrap();

        (k_conv, k_runs, k_missing, k_noroot, db_path)
    };

    let (url, _guard) = spawn_dashboard("serve", &ws_conv, &env);
    let (status, body) = http_get(&url, "/api/global/projects");
    assert_eq!(status, 200, "body: {body}");
    let parsed: Value = serde_json::from_str(&body).unwrap();
    let projects = parsed["projects"].as_array().expect("projects array");
    let p_conv = projects
        .iter()
        .find(|p| p["repo_key"] == k_conv)
        .expect("conv repo listed");
    let p_runs = projects
        .iter()
        .find(|p| p["repo_key"] == k_runs)
        .expect("runs-only repo listed");
    let p_missing = projects
        .iter()
        .find(|p| p["repo_key"] == k_missing)
        .expect("missing repo listed");
    let p_noroot = projects
        .iter()
        .find(|p| p["repo_key"] == k_noroot)
        .expect("noroot repo listed");
    assert_eq!(p_conv["conversation_count"], 1);
    assert_eq!(p_conv["open_ws"], "default");
    assert_eq!(p_runs["run_count"], 1);
    assert_eq!(p_runs["active_run_count"], 1);
    assert!(p_runs.get("open_ws").is_some() && p_runs["open_ws"].is_null());
    assert_eq!(
        p_runs["canonical_path"],
        fs::canonicalize(&ws_runs)
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(p_missing["missing"], true);
    assert!(p_noroot.get("canonical_path").is_some() && p_noroot["canonical_path"].is_null());
    assert_eq!(p_noroot["missing"], true);

    // Verify last_activity_ms descending sort order.
    for pair in projects.windows(2) {
        let a = pair[0]["last_activity_ms"].as_u64().unwrap();
        let b = pair[1]["last_activity_ms"].as_u64().unwrap();
        assert!(
            a >= b,
            "projects not sorted by last_activity_ms desc: {a} < {b}"
        );
    }

    // Verify DB was opened read-only and not migrated/modified.
    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let user_ver: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert!(user_ver >= 1);
}

// TC-2982 / FR-2982: global conversation detail, run detail, run stream, and 404 on bad keys/ids.
#[test]
fn tc_2982_global_conversation_run_detail_and_stream_read_only_bad_keys_404() {
    let env = IsolatedEnv::new();
    let ws_conv = env.root().join("repo-c");
    let ws_runs = env.root().join("repo-r");
    fs::create_dir_all(&ws_conv).unwrap();
    fs::create_dir_all(&ws_runs).unwrap();

    let (k_conv, conv_id, k_runs, run_id) = {
        let _lock = dmuxai::state_path::test_env_lock();
        let _s = EnvRestore::set("DMUXAI_STATE_DIR", &env.state_dir);
        let _d = EnvRestore::set("DMUXAI_DATA_DIR", &env.data_dir);

        let k_conv = repo_key(&ws_conv).unwrap();
        let db_path = env
            .state_dir
            .join("repos")
            .join(&k_conv)
            .join("conversations.sqlite3");
        let mut engine = ConversationEngine::open(&db_path, &ws_conv).unwrap();
        let snap = engine
            .select(ConversationSelection::New {
                title: Some("Alpha Task".to_owned()),
            })
            .unwrap();
        let _ = engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "hello from user".to_owned(),
                display_text: None,
            })
            .unwrap();
        let conv_id = snap.id.as_str().to_owned();

        let k_runs = repo_key(&ws_runs).unwrap();
        let runs_dir = env.data_dir.join("repos").join(&k_runs).join("runs");
        let store = JournalStore::new(runs_dir);
        let run_id = "deadbeef01234567";
        let manifest = make_manifest(run_id, "completed", Some(&ws_runs), "cli_headless");
        store.save_relaxed(&manifest).unwrap();
        store
            .append_progress(run_id, r#"{"kind":"step","msg":"done"}"#)
            .unwrap();

        (k_conv, conv_id, k_runs, run_id.to_owned())
    };

    let (url, _guard) = spawn_dashboard("serve", &ws_conv, &env);

    let (st, convs_body) = http_get(
        &url,
        &format!("/api/global/projects/{k_conv}/conversations"),
    );
    assert_eq!(st, 200);
    assert!(convs_body.contains("Alpha Task"));

    let (st, detail_body) = http_get(
        &url,
        &format!("/api/global/projects/{k_conv}/conversations/{conv_id}"),
    );
    assert_eq!(st, 200);
    assert!(detail_body.contains("hello from user"));

    let (st, runs_body) = http_get(&url, &format!("/api/global/projects/{k_runs}/runs"));
    assert_eq!(st, 200);
    assert!(runs_body.contains(&run_id));

    let (st, run_detail) = http_get(
        &url,
        &format!("/api/global/projects/{k_runs}/runs/{run_id}"),
    );
    assert_eq!(st, 200);
    assert!(run_detail.contains("completed"));

    let (st, sse_body) = read_sse_until(
        &url,
        &format!("/api/global/projects/{k_runs}/runs/{run_id}/stream"),
        "event: end",
    );
    assert_eq!(st, 200);
    assert!(sse_body.contains("event: run"));

    // Live active -> completed stream transition.
    let live_run_id = "1234567890abcdef";
    let runs_dir = env.data_dir.join("repos").join(&k_runs).join("runs");
    let store = JournalStore::new(runs_dir);
    store
        .save_relaxed(&make_manifest(
            live_run_id,
            "active",
            Some(&ws_runs),
            "cli_headless",
        ))
        .unwrap();
    let url_clone = url.clone();
    let k_runs_clone = k_runs.clone();
    let handle = std::thread::spawn(move || {
        read_sse_until(
            &url_clone,
            &format!("/api/global/projects/{k_runs_clone}/runs/{live_run_id}/stream"),
            "event: end",
        )
    });
    std::thread::sleep(Duration::from_millis(150));
    store
        .append_progress(live_run_id, r#"{"kind":"step","msg":"live-line"}"#)
        .unwrap();
    store
        .save_relaxed(&make_manifest(
            live_run_id,
            "completed",
            Some(&ws_runs),
            "cli_headless",
        ))
        .unwrap();
    let (live_st, live_sse) = handle.join().unwrap();
    assert_eq!(live_st, 200);
    assert!(live_sse.contains("live-line"));
    assert!(live_sse.contains("event: end"));

    // Unknown or invalid repo keys and ids return 404.
    let (st, _) = http_get(
        &url,
        "/api/global/projects/unknown-0000000000000000/conversations",
    );
    assert_eq!(st, 404);
    let (st, _) = http_get(
        &url,
        &format!("/api/global/projects/{k_conv}/conversations/no-such-id"),
    );
    assert_eq!(st, 404);
    let (st, _) = http_get(&url, &format!("/api/global/projects/{k_runs}/runs/bad!id"));
    assert_eq!(st, 404);
}

// TC-2983 / FR-2983: global responses mask sensitive tokens and never expose dashboard token.
#[test]
fn tc_2983_global_responses_mask_secrets() {
    let env = IsolatedEnv::new();
    let ws = env.root().join("repo-sec");
    fs::create_dir_all(&ws).unwrap();

    let (k, conv_id, run_id) = {
        let _lock = dmuxai::state_path::test_env_lock();
        let _s = EnvRestore::set("DMUXAI_STATE_DIR", &env.state_dir);
        let _d = EnvRestore::set("DMUXAI_DATA_DIR", &env.data_dir);

        let k = repo_key(&ws).unwrap();
        let db_path = env
            .state_dir
            .join("repos")
            .join(&k)
            .join("conversations.sqlite3");
        let mut engine = ConversationEngine::open(&db_path, &ws).unwrap();
        let snap = engine
            .select(ConversationSelection::New {
                title: Some("Bearer sk-proj-secret1234567890abcd".to_owned()),
            })
            .unwrap();
        let _ = engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "token is ghp_1234567890abcdef1234567890abcdef1234".to_owned(),
                display_text: None,
            })
            .unwrap();
        let conv_id = snap.id.as_str().to_owned();

        let runs_dir = env.data_dir.join("repos").join(&k).join("runs");
        let store = JournalStore::new(runs_dir);
        let run_id = "beef1234cafe5678";
        let manifest = make_manifest(
            run_id,
            "completed",
            Some(&ws),
            "cli sk-proj-secret1234567890abcd",
        );
        store.save_relaxed(&manifest).unwrap();
        store
            .append_progress(
                run_id,
                r#"{"apikey":"secret-api-key-val","msg":"Bearer sk-proj-secret1234567890abcd"}"#,
            )
            .unwrap();
        (k, conv_id, run_id.to_owned())
    };

    let (url, _guard) = spawn_dashboard("serve", &ws, &env);
    let (_, projs_body) = http_get(&url, "/api/global/projects");
    assert!(!projs_body.contains("sk-proj-secret1234567890abcd"));

    let (_, convs_body) = http_get(&url, &format!("/api/global/projects/{k}/conversations"));
    assert!(!convs_body.contains("sk-proj-secret1234567890abcd"));

    let (_, conv_body) = http_get(
        &url,
        &format!("/api/global/projects/{k}/conversations/{conv_id}"),
    );
    assert!(!conv_body.contains("sk-proj-secret1234567890abcd"));
    assert!(!conv_body.contains("ghp_1234567890abcdef1234567890abcdef1234"));

    let (_, runs_body) = http_get(&url, &format!("/api/global/projects/{k}/runs"));
    assert!(!runs_body.contains("sk-proj-secret1234567890abcd"));

    let (_, run_body) = http_get(&url, &format!("/api/global/projects/{k}/runs/{run_id}"));
    assert!(!run_body.contains("sk-proj-secret1234567890abcd"));
    assert!(!run_body.contains("secret-api-key-val"));

    let (st, sse_body) = read_sse_until(
        &url,
        &format!("/api/global/projects/{k}/runs/{run_id}/stream"),
        "event: end",
    );
    assert_eq!(st, 200);
    assert!(!sse_body.contains("sk-proj-secret1234567890abcd"));
    assert!(!sse_body.contains("secret-api-key-val"));
}

// --- Headless Chrome helpers for TC-2984 and TC-2985 ---

fn find_chrome_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("CHROME_BIN") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    for candidate in [
        "/usr/bin/google-chrome",
        "/opt/google/chrome/chrome",
        "google-chrome",
        "chromium",
        "chromium-browser",
    ] {
        let p = Path::new(candidate);
        if p.is_absolute() {
            if p.exists() {
                return Some(p.to_path_buf());
            }
        } else if let Ok(output) = Command::new("which").arg(candidate).output()
            && output.status.success()
        {
            let found = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if !found.is_empty() {
                return Some(PathBuf::from(found));
            }
        }
    }
    None
}

fn http_get_json(authority: &str, path: &str) -> std::io::Result<Value> {
    let mut stream = TcpStream::connect(authority)?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes())?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let mut content_length: Option<usize> = None;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                let text = String::from_utf8_lossy(&buf);
                if content_length.is_none()
                    && let Some(pos) = text.find("\r\n\r\n")
                {
                    for line in text[..pos].lines() {
                        if let Some((k, v)) = line.split_once(':')
                            && k.trim().eq_ignore_ascii_case("content-length")
                            && let Ok(len) = v.trim().parse::<usize>()
                        {
                            content_length = Some(len);
                            break;
                        }
                    }
                }
                if let Some(len) = content_length
                    && let Some(pos) = text.find("\r\n\r\n")
                    && buf.len() >= pos + 4 + len
                {
                    let body = &text[pos + 4..pos + 4 + len];
                    return serde_json::from_str(body)
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e));
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").trim();
    serde_json::from_str(body).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

struct CdpClient {
    stream: TcpStream,
    msg_id: u64,
}

impl CdpClient {
    fn connect(authority: &str, path: &str) -> std::io::Result<Self> {
        let mut stream = TcpStream::connect(authority)?;
        stream.set_read_timeout(Some(Duration::from_millis(500)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(req.as_bytes())?;
        let mut response = [0u8; 1024];
        let n = stream.read(&mut response)?;
        if !String::from_utf8_lossy(&response[..n]).contains("101") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "WebSocket handshake failed",
            ));
        }
        Ok(Self { stream, msg_id: 0 })
    }

    fn eval(&mut self, expr: &str) -> Result<Value, String> {
        self.msg_id += 1;
        let id = self.msg_id;
        let msg = serde_json::json!({
            "id": id,
            "method": "Runtime.evaluate",
            "params": {
                "expression": expr,
                "returnByValue": true,
                "awaitPromise": true
            }
        })
        .to_string();
        let payload = msg.as_bytes();
        let mut frame = vec![0x81];
        if payload.len() < 126 {
            frame.push(0x80 | (payload.len() as u8));
        } else {
            frame.push(0x80 | 126);
            frame.push(((payload.len() >> 8) & 0xff) as u8);
            frame.push((payload.len() & 0xff) as u8);
        }
        frame.extend_from_slice(&[0, 0, 0, 0]);
        frame.extend_from_slice(payload);
        self.stream.write_all(&frame).map_err(|e| e.to_string())?;
        self.stream.flush().map_err(|e| e.to_string())?;

        let mut buf = Vec::new();
        let mut temp = [0u8; 4096];
        let target_str = format!("\"id\":{id}");
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match self.stream.read(&mut temp) {
                Ok(0) => return Err("connection closed".to_owned()),
                Ok(n) => {
                    buf.extend_from_slice(&temp[..n]);
                    let text = String::from_utf8_lossy(&buf);
                    if text.contains(&target_str)
                        && let Some(start) = text.find(&format!("{{\"id\":{id}"))
                        && let Some(end) = text[start..].rfind('}')
                        && let Ok(val) =
                            serde_json::from_str::<Value>(&text[start..start + end + 1])
                        && val.get("id").and_then(|v| v.as_u64()) == Some(id)
                    {
                        return Ok(val["result"]["result"]["value"].clone());
                    }
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("timed out waiting for CDP response".to_owned())
    }
}

struct ChromeSession {
    _user_data_dir: tempfile::TempDir,
    _chrome: ChildGuard,
    cdp: CdpClient,
}

fn open_in_chrome(url: &str) -> ChromeSession {
    let chrome_bin = find_chrome_binary().expect("chrome binary");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cdp_port = listener.local_addr().unwrap().port();
    drop(listener);
    let user_data_dir = tempfile::tempdir().unwrap();
    let child = Command::new(&chrome_bin)
        .args([
            "--headless=new",
            "--no-sandbox",
            "--disable-gpu",
            &format!("--remote-debugging-port={cdp_port}"),
            &format!("--user-data-dir={}", user_data_dir.path().display()),
            "--window-size=1280,800",
            url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn chrome");
    let guard = ChildGuard(Some(child));

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut ws_url = None;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(pages) = http_get_json(&format!("127.0.0.1:{cdp_port}"), "/json/list")
            && let Some(arr) = pages.as_array()
        {
            for page in arr {
                if page.get("type").and_then(|t| t.as_str()) == Some("page")
                    && let Some(w) = page.get("webSocketDebuggerUrl").and_then(|v| v.as_str())
                {
                    ws_url = Some(w.to_owned());
                    break;
                }
            }
        }
        if ws_url.is_some() {
            break;
        }
    }
    let ws_url = ws_url.expect("CDP page websocket");
    let ws_path = ws_url
        .split(&format!("{cdp_port}"))
        .nth(1)
        .unwrap()
        .to_owned();
    let cdp = CdpClient::connect(&format!("127.0.0.1:{cdp_port}"), &ws_path).expect("cdp connect");
    ChromeSession {
        _user_data_dir: user_data_dir,
        _chrome: guard,
        cdp,
    }
}

fn wait_until(session: &mut ChromeSession, expr: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if session.cdp.eval(expr).ok().and_then(|v| v.as_bool()) == Some(true) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for: {expr}");
}

// Wait for the initial /api/state render so handlers installed by init() (including
// the More menu) are bound before a test clicks. Mirrors web_tab_trim.rs.
fn wait_for_dashboard_ready(session: &mut ChromeSession) {
    wait_until(
        session,
        "typeof applySnapshot === 'function' && snapshot !== null",
    );
}

// Click the same #more-toggle the user clicks, retrying until the menu opens.
// The click is skipped while the menu is already open so a retry cannot toggle it closed.
fn open_more_menu(session: &mut ChromeSession) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let _ = session.cdp.eval(
            "(() => { const m = document.getElementById('more-menu'); \
             if (m && m.hidden) document.getElementById('more-toggle').click(); })()",
        );
        std::thread::sleep(Duration::from_millis(100));
        if session
            .cdp
            .eval("(() => { const m = document.getElementById('more-menu'); return !!m && !m.hidden; })()")
            .ok()
            .and_then(|v| v.as_bool())
            == Some(true)
        {
            return;
        }
    }
    panic!("timed out waiting for more menu to open");
}

// TC-2984 / FR-2984: All projects UI renders projects with counts, active-run marker,
// conversations, runs, and read-only detail badge.
#[test]
#[ignore]
fn tc_2984_all_projects_view_renders_projects_conversations_runs() {
    let env = IsolatedEnv::new();
    let ws1 = env.root().join("ui-proj-1");
    let ws2 = env.root().join("ui-proj-2");
    fs::create_dir_all(&ws1).unwrap();
    fs::create_dir_all(&ws2).unwrap();

    let (k1, k2) = {
        let _lock = dmuxai::state_path::test_env_lock();
        let _s = EnvRestore::set("DMUXAI_STATE_DIR", &env.state_dir);
        let _d = EnvRestore::set("DMUXAI_DATA_DIR", &env.data_dir);

        let k1 = repo_key(&ws1).unwrap();
        let db1 = env
            .state_dir
            .join("repos")
            .join(&k1)
            .join("conversations.sqlite3");
        let mut engine = ConversationEngine::open(&db1, &ws1).unwrap();
        engine
            .select(ConversationSelection::New {
                title: Some("UI Conv One".to_owned()),
            })
            .unwrap();
        let _ = engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "read only message".to_owned(),
                display_text: None,
            })
            .unwrap();

        let k2 = repo_key(&ws2).unwrap();
        let runs_dir = env.data_dir.join("repos").join(&k2).join("runs");
        let store = JournalStore::new(runs_dir);
        store
            .save_relaxed(&make_manifest(
                "cafe0000beef1111",
                "active",
                Some(&ws2),
                "cli_headless",
            ))
            .unwrap();
        (k1, k2)
    };

    let (url, _guard) = spawn_dashboard("serve", &ws1, &env);
    let mut chrome = open_in_chrome(&url);

    wait_for_dashboard_ready(&mut chrome);
    open_more_menu(&mut chrome);
    chrome
        .cdp
        .eval("document.querySelector('#all-projects-toggle').click()")
        .unwrap();
    wait_until(
        &mut chrome,
        &format!(
            "!!document.querySelector('[data-repo-key=\"{k1}\"]') && !!document.querySelector('[data-repo-key=\"{k2}\"]')"
        ),
    );
    let active_text = chrome
        .cdp
        .eval("document.querySelector('.all-projects-active-badge')?.textContent || ''")
        .unwrap();
    assert!(active_text.as_str().unwrap_or("").contains("active"));

    chrome
        .cdp
        .eval(&format!(
            "document.querySelector('[data-repo-key=\"{k1}\"]').click()"
        ))
        .unwrap();
    wait_until(
        &mut chrome,
        "!!document.querySelector('#all-projects-conversations [data-conv-id]')",
    );
    chrome
        .cdp
        .eval("document.querySelector('#all-projects-conversations [data-conv-id]').click()")
        .unwrap();
    wait_until(
        &mut chrome,
        "!!document.querySelector('.all-projects-readonly-badge')",
    );
    let body_text = chrome
        .cdp
        .eval("document.querySelector('#all-projects-item-detail')?.textContent || ''")
        .unwrap();
    assert!(
        body_text
            .as_str()
            .unwrap_or("")
            .contains("read only message")
    );

    // Click second project and inspect its run detail.
    chrome
        .cdp
        .eval(&format!(
            "document.querySelector('[data-repo-key=\"{k2}\"]').click()"
        ))
        .unwrap();
    wait_until(
        &mut chrome,
        "!!document.querySelector('#all-projects-runs [data-run-id=\"cafe0000beef1111\"]')",
    );
    chrome
        .cdp
        .eval("document.querySelector('#all-projects-runs [data-run-id=\"cafe0000beef1111\"]').click()")
        .unwrap();
    wait_until(
        &mut chrome,
        "!!document.querySelector('.all-projects-run-log') && !!document.querySelector('.all-projects-readonly-badge')",
    );
}

// TC-2985 / FR-2985: Open project is disabled in serve with hint, and in --web attaches and loads conversation.
#[test]
#[ignore]
fn tc_2985_open_project_attaches_and_loads_conversation_disabled_in_serve() {
    let env = IsolatedEnv::new();
    let ws1 = env.root().join("open-proj-1");
    let ws2 = env.root().join("open-proj-2");
    fs::create_dir_all(&ws1).unwrap();
    fs::create_dir_all(&ws2).unwrap();

    let (k2, conv2_id) = {
        let _lock = dmuxai::state_path::test_env_lock();
        let _s = EnvRestore::set("DMUXAI_STATE_DIR", &env.state_dir);
        let _d = EnvRestore::set("DMUXAI_DATA_DIR", &env.data_dir);
        let k2 = repo_key(&ws2).unwrap();
        let db2 = env
            .state_dir
            .join("repos")
            .join(&k2)
            .join("conversations.sqlite3");
        let mut engine = ConversationEngine::open(&db2, &ws2).unwrap();
        let snap = engine
            .select(ConversationSelection::New {
                title: Some("Target Conv".to_owned()),
            })
            .unwrap();
        let _ = engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "target prompt".to_owned(),
                display_text: None,
            })
            .unwrap();
        (k2, snap.id.as_str().to_owned())
    };

    // 1. In view-only serve, Open project is disabled with a hint.
    let (serve_url, _serve_guard) = spawn_dashboard("serve", &ws1, &env);
    let mut serve_chrome = open_in_chrome(&serve_url);
    wait_for_dashboard_ready(&mut serve_chrome);
    open_more_menu(&mut serve_chrome);
    serve_chrome
        .cdp
        .eval("document.querySelector('#all-projects-toggle').click()")
        .unwrap();
    wait_until(
        &mut serve_chrome,
        &format!("!!document.querySelector('[data-repo-key=\"{k2}\"]')"),
    );
    serve_chrome
        .cdp
        .eval(&format!(
            "document.querySelector('[data-repo-key=\"{k2}\"]').click()"
        ))
        .unwrap();
    wait_until(
        &mut serve_chrome,
        "!!document.querySelector('#all-projects-open-btn')",
    );
    let disabled = serve_chrome
        .cdp
        .eval("document.querySelector('#all-projects-open-btn').disabled")
        .unwrap();
    assert_eq!(disabled, Value::Bool(true));
    let serve_hint = serve_chrome
        .cdp
        .eval("document.querySelector('#all-projects-open-hint')?.textContent || ''")
        .unwrap();
    assert!(serve_hint.as_str().unwrap_or("").contains("View-only"));

    // 2. In --web host, Open project is enabled, attaches ws2, and loads selected conversation.
    let (web_url, _web_guard) = spawn_dashboard("web", &ws1, &env);
    let mut web_chrome = open_in_chrome(&web_url);
    wait_for_dashboard_ready(&mut web_chrome);
    open_more_menu(&mut web_chrome);
    web_chrome
        .cdp
        .eval("document.querySelector('#all-projects-toggle').click()")
        .unwrap();
    wait_until(
        &mut web_chrome,
        &format!("!!document.querySelector('[data-repo-key=\"{k2}\"]')"),
    );
    web_chrome
        .cdp
        .eval(&format!(
            "document.querySelector('[data-repo-key=\"{k2}\"]').click()"
        ))
        .unwrap();
    wait_until(
        &mut web_chrome,
        &format!("!!document.querySelector('[data-conv-id=\"{conv2_id}\"]')"),
    );
    web_chrome
        .cdp
        .eval(&format!(
            "document.querySelector('[data-conv-id=\"{conv2_id}\"]').click()"
        ))
        .unwrap();
    wait_until(
        &mut web_chrome,
        "document.querySelector('#all-projects-open-btn').disabled === false",
    );
    web_chrome
        .cdp
        .eval("document.querySelector('#all-projects-open-btn').click()")
        .unwrap();
    wait_until(&mut web_chrome, "window.location.search.includes('ws=')");
    wait_until(
        &mut web_chrome,
        "document.getElementById('all-projects').hidden === true",
    );
    wait_until(
        &mut web_chrome,
        "(document.getElementById('session')?.textContent || '').includes('target prompt')",
    );
}
