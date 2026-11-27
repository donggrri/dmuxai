//! Global read-only projects, conversations, and runs API (`GET /api/global/*`,
//! SPEC-web-all-projects FR-2981..FR-2986, NFR-2980).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Serialize;

use crate::conversation::{
    list_repo_db_conversations, read_repo_conversation_snapshot, summarize_repo_db,
};
use crate::run_journal::{JournalStore, data_base, is_symlink, mask_json_value, valid_run_id};
use crate::state_path::{WorkspaceMetadata, repo_key_for_canonical, state_base};
use crate::web::runs_api::{
    RunsListQuery, detail_response, is_run_active, list_manifests, list_response,
    open_regular_nofollow, parse_runs_limit, redact_token_in_value, stream_response,
};
use crate::web::server::{AppState, error_json, json_ok};

const WORKSPACE_JSON_READ_CAP: u64 = 64 * 1024;
const PROJECT_CACHE_TTL: Duration = Duration::from_secs(5);
const GLOBAL_CONVERSATIONS_LIMIT: usize = 200;

/// Validated filesystem locations for an enumerated `repo_key` (OQ-2980, FR-2986).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoLocations {
    pub repo_key: String,
    pub state_dir: Option<PathBuf>,
    pub runs_dir: Option<PathBuf>,
}

fn is_single_safe_dir_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

/// Enumerates every repo directory under `<state base>/repos/<key>/` and every
/// runs root under `<data base>/repos/<key>/runs` (OQ-2980).
pub fn enumerate_repo_locations() -> Vec<RepoLocations> {
    let mut map: BTreeMap<String, RepoLocations> = BTreeMap::new();

    if let Ok(base) = state_base() {
        let repos_dir = base.join("repos");
        if let Ok(entries) = fs::read_dir(&repos_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if is_symlink(&path).unwrap_or(true) || !path.is_dir() {
                    continue;
                }
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !is_single_safe_dir_name(&name) {
                    continue;
                }
                let loc = map.entry(name.clone()).or_insert_with(|| RepoLocations {
                    repo_key: name,
                    state_dir: None,
                    runs_dir: None,
                });
                loc.state_dir = Some(path);
            }
        }
    }

    if let Ok(base) = data_base() {
        let repos_dir = base.join("repos");
        if let Ok(entries) = fs::read_dir(&repos_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if is_symlink(&path).unwrap_or(true) || !path.is_dir() {
                    continue;
                }
                let runs_path = path.join("runs");
                if is_symlink(&runs_path).unwrap_or(true) || !runs_path.is_dir() {
                    continue;
                }
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !is_single_safe_dir_name(&name) {
                    continue;
                }
                let loc = map.entry(name.clone()).or_insert_with(|| RepoLocations {
                    repo_key: name,
                    state_dir: None,
                    runs_dir: None,
                });
                loc.runs_dir = Some(runs_path);
            }
        }
    }

    map.into_values().collect()
}

/// Validates and resolves `key` by matching it against enumerated repo
/// directories (Terms, FR-2986). Never joins `key` into a filesystem path.
pub fn lookup_repo_key(key: &str) -> Option<RepoLocations> {
    if !is_single_safe_dir_name(key) {
        return None;
    }
    enumerate_repo_locations()
        .into_iter()
        .find(|loc| loc.repo_key == key)
}

fn read_workspace_metadata(state_dir: &Path, expected_key: &str) -> Option<WorkspaceMetadata> {
    let path = state_dir.join("workspace.json");
    let mut file = open_regular_nofollow(&path)?;
    let mut bytes = Vec::new();
    if (&mut file)
        .take(WORKSPACE_JSON_READ_CAP)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return None;
    }
    let meta: WorkspaceMetadata = serde_json::from_slice(&bytes).ok()?;
    if repo_key_for_canonical(&meta.canonical_path) != expected_key {
        return None;
    }
    Some(meta)
}

#[derive(Debug, Clone, Serialize)]
pub struct GlobalProjectItem {
    pub repo_key: String,
    pub canonical_path: Option<String>,
    pub missing: bool,
    pub conversation_count: usize,
    pub run_count: usize,
    pub active_run_count: usize,
    pub last_activity_ms: u64,
    pub open_ws: Option<String>,
}

struct CachedProjects {
    state_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    cached_at: Instant,
    items: Vec<GlobalProjectItem>,
}

static PROJECT_LIST_CACHE: LazyLock<Mutex<Option<CachedProjects>>> =
    LazyLock::new(|| Mutex::new(None));

fn build_project_item(loc: &RepoLocations) -> GlobalProjectItem {
    let ws_meta = loc
        .state_dir
        .as_deref()
        .and_then(|dir| read_workspace_metadata(dir, &loc.repo_key));

    let db_summary = loc.state_dir.as_deref().and_then(|dir| {
        let db_path = dir.join("conversations.sqlite3");
        summarize_repo_db(&db_path, &loc.repo_key)
    });

    let manifests = loc
        .runs_dir
        .as_deref()
        .map(|dir| list_manifests(&JournalStore::new(dir.to_path_buf())))
        .unwrap_or_default();

    let run_count = manifests.len();
    let active_run_count = manifests
        .iter()
        .filter(|m| is_run_active(&m.status))
        .count();
    let runs_latest_ms = manifests
        .iter()
        .map(|m| m.completed_at_ms.unwrap_or(m.created_at_ms))
        .max()
        .unwrap_or(0);
    let manifest_root = manifests.iter().find_map(|m| {
        m.workspace_root
            .as_ref()
            .filter(|s| !s.is_empty() && repo_key_for_canonical(s) == loc.repo_key)
            .cloned()
    });

    let canonical_path = ws_meta
        .as_ref()
        .map(|m| m.canonical_path.clone())
        .or(manifest_root)
        .or_else(|| db_summary.as_ref().and_then(|s| s.workspace_root.clone()));

    let missing = canonical_path
        .as_deref()
        .map(|p| !Path::new(p).is_dir())
        .unwrap_or(true);

    let conversation_count = db_summary.as_ref().map(|s| s.count).unwrap_or(0);
    let conv_latest_ms = db_summary
        .as_ref()
        .and_then(|s| s.latest_updated_at_ms)
        .unwrap_or(0);
    let meta_seen_ms = ws_meta.as_ref().map(|m| m.last_seen_ms).unwrap_or(0);

    let last_activity_ms = conv_latest_ms.max(runs_latest_ms).max(meta_seen_ms);

    GlobalProjectItem {
        repo_key: loc.repo_key.clone(),
        canonical_path,
        missing,
        conversation_count,
        run_count,
        active_run_count,
        last_activity_ms,
        open_ws: None,
    }
}

fn list_projects_cached() -> Vec<GlobalProjectItem> {
    let cur_state = state_base().ok();
    let cur_data = data_base().ok();

    {
        let guard = PROJECT_LIST_CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = guard.as_ref()
            && cached.state_dir == cur_state
            && cached.data_dir == cur_data
            && cached.cached_at.elapsed() < PROJECT_CACHE_TTL
        {
            return cached.items.clone();
        }
    }

    let mut items: Vec<GlobalProjectItem> = enumerate_repo_locations()
        .iter()
        .map(build_project_item)
        .collect();

    items.sort_by(|a, b| {
        b.last_activity_ms
            .cmp(&a.last_activity_ms)
            .then_with(|| a.repo_key.cmp(&b.repo_key))
    });

    let mut guard = PROJECT_LIST_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(CachedProjects {
        state_dir: cur_state,
        data_dir: cur_data,
        cached_at: Instant::now(),
        items: items.clone(),
    });
    items
}

fn resolve_workspace_root(loc: &RepoLocations) -> Option<PathBuf> {
    if let Some(dir) = loc.state_dir.as_deref()
        && let Some(meta) = read_workspace_metadata(dir, &loc.repo_key)
    {
        return Some(PathBuf::from(meta.canonical_path));
    }
    if let Some(runs_dir) = loc.runs_dir.as_deref() {
        let store = JournalStore::new(runs_dir.to_path_buf());
        if let Some(root) = list_manifests(&store).into_iter().find_map(|m| {
            m.workspace_root
                .filter(|s| !s.is_empty() && repo_key_for_canonical(s) == loc.repo_key)
        }) {
            return Some(PathBuf::from(root));
        }
    }
    if let Some(dir) = loc.state_dir.as_deref()
        && let Some(summary) = summarize_repo_db(&dir.join("conversations.sqlite3"), &loc.repo_key)
        && let Some(root) = summary.workspace_root
    {
        return Some(PathBuf::from(root));
    }
    None
}

fn valid_conversation_id(id: &str) -> bool {
    let trimmed = id.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 128
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub(crate) async fn list_projects(State(state): State<AppState>) -> Response {
    let mut items = list_projects_cached();
    let mut open_by_key: HashMap<String, String> = HashMap::new();
    for entry in state.workspaces.entries() {
        let key = repo_key_for_canonical(&entry.path);
        open_by_key.entry(key).or_insert_with(|| entry.id.clone());
    }
    for item in &mut items {
        item.open_ws = open_by_key.get(&item.repo_key).cloned();
    }

    let mut payload = serde_json::json!({ "projects": items });
    mask_json_value(&mut payload);
    redact_token_in_value(&mut payload, state.token());
    json_ok(StatusCode::OK, &payload)
}

#[derive(Serialize)]
struct GlobalConversationItem {
    id: String,
    title: String,
    updated_at_ms: i64,
    turn_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_agent: Option<String>,
    spoken_agents: Vec<String>,
    running: bool,
}

pub(crate) async fn list_conversations(
    State(state): State<AppState>,
    AxumPath(repo_key): AxumPath<String>,
) -> Response {
    let Some(loc) = lookup_repo_key(&repo_key) else {
        return error_json(StatusCode::NOT_FOUND, "project not found");
    };
    let rows = loc
        .state_dir
        .as_deref()
        .map(|dir| {
            list_repo_db_conversations(
                &dir.join("conversations.sqlite3"),
                &loc.repo_key,
                GLOBAL_CONVERSATIONS_LIMIT,
            )
        })
        .unwrap_or_default();

    let conversations: Vec<GlobalConversationItem> = rows
        .into_iter()
        .map(|(_, card)| GlobalConversationItem {
            id: card.id.as_str().to_owned(),
            title: card.title,
            updated_at_ms: card.updated_at_ms,
            turn_count: card.turn_count,
            last_agent: card.last_agent.map(|a| a.as_db().to_owned()),
            spoken_agents: card
                .spoken_agents
                .into_iter()
                .map(|a| a.as_db().to_owned())
                .collect(),
            running: card.running,
        })
        .collect();

    let mut payload = serde_json::json!({ "conversations": conversations });
    mask_json_value(&mut payload);
    redact_token_in_value(&mut payload, state.token());
    json_ok(StatusCode::OK, &payload)
}

pub(crate) async fn conversation_detail(
    State(state): State<AppState>,
    AxumPath((repo_key, id)): AxumPath<(String, String)>,
) -> Response {
    let Some(loc) = lookup_repo_key(&repo_key) else {
        return error_json(StatusCode::NOT_FOUND, "project not found");
    };
    if !valid_conversation_id(&id) {
        return error_json(StatusCode::NOT_FOUND, "conversation not found");
    }
    let Some(state_dir) = loc.state_dir.as_deref() else {
        return error_json(StatusCode::NOT_FOUND, "conversation not found");
    };
    let db_path = state_dir.join("conversations.sqlite3");
    let Ok(Some(snapshot)) = read_repo_conversation_snapshot(&db_path, &loc.repo_key, &id) else {
        return error_json(StatusCode::NOT_FOUND, "conversation not found");
    };

    let turns: Vec<serde_json::Value> = snapshot
        .turns
        .into_iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id.as_str(),
                "seq": t.seq,
                "agent": t.agent.as_db(),
                "model_id": t.model_id,
                "status": format!("{:?}", t.status).to_lowercase(),
                "error_text": t.error_text,
            })
        })
        .collect();

    let messages: Vec<serde_json::Value> = snapshot
        .messages
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "turn_id": m.turn_id.as_str(),
                "turn_seq": m.turn_seq,
                "agent": m.agent.as_db(),
                "role": format!("{:?}", m.role).to_lowercase(),
                "text": m.display_text.unwrap_or(m.text),
                "state": format!("{:?}", m.state).to_lowercase(),
            })
        })
        .collect();

    let mut payload = serde_json::json!({
        "conversation": {
            "id": snapshot.id.as_str(),
            "title": snapshot.title,
            "turns": turns,
            "messages": messages,
        }
    });
    mask_json_value(&mut payload);
    redact_token_in_value(&mut payload, state.token());
    json_ok(StatusCode::OK, &payload)
}

pub(crate) async fn list_runs(
    State(state): State<AppState>,
    AxumPath(repo_key): AxumPath<String>,
    Query(params): Query<RunsListQuery>,
) -> Response {
    let Some(loc) = lookup_repo_key(&repo_key) else {
        return error_json(StatusCode::NOT_FOUND, "project not found");
    };
    let limit = match parse_runs_limit(params.limit.as_deref()) {
        Ok(limit) => limit,
        Err(()) => return error_json(StatusCode::BAD_REQUEST, "invalid limit"),
    };
    let Some(runs_dir) = loc.runs_dir else {
        return json_ok(StatusCode::OK, &serde_json::json!({ "runs": [] }));
    };
    list_response(&JournalStore::new(runs_dir), limit, state.token())
}

pub(crate) async fn run_detail(
    State(state): State<AppState>,
    AxumPath((repo_key, run_id)): AxumPath<(String, String)>,
) -> Response {
    let Some(loc) = lookup_repo_key(&repo_key) else {
        return error_json(StatusCode::NOT_FOUND, "project not found");
    };
    if !valid_run_id(&run_id) {
        return error_json(StatusCode::NOT_FOUND, "run not found");
    }
    let Some(runs_dir) = loc.runs_dir.clone() else {
        return error_json(StatusCode::NOT_FOUND, "run not found");
    };
    let workspace_root = resolve_workspace_root(&loc);
    detail_response(
        &JournalStore::new(runs_dir),
        workspace_root.as_deref(),
        &run_id,
        state.token(),
    )
}

pub(crate) async fn run_stream(
    State(state): State<AppState>,
    AxumPath((repo_key, run_id)): AxumPath<(String, String)>,
) -> Response {
    let Some(loc) = lookup_repo_key(&repo_key) else {
        return error_json(StatusCode::NOT_FOUND, "project not found");
    };
    if !valid_run_id(&run_id) {
        return error_json(StatusCode::NOT_FOUND, "run not found");
    }
    let Some(runs_dir) = loc.runs_dir.clone() else {
        return error_json(StatusCode::NOT_FOUND, "run not found");
    };
    let events_path =
        resolve_workspace_root(&loc).map(|root| root.join(".dmuxai").join("events.jsonl"));
    stream_response(
        &JournalStore::new(runs_dir),
        events_path,
        run_id,
        state.token().to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_path::test_env_lock;

    struct EnvRestore {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl EnvRestore {
        fn set(key: &'static str, val: &Path) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: serialized by `test_env_lock()`.
            unsafe { std::env::set_var(key, val) };
            Self { key, prev }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            // SAFETY: serialized by `test_env_lock()`.
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    // TC-2986 / FR-2986: valid repo key blocks traversal and unknown keys.
    #[test]
    fn tc_2986_valid_repo_key_blocks_traversal() {
        let _lock = test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let state_dir = tmp.path().join("state");
        let data_dir = tmp.path().join("data");
        let valid_key = "demo-0123456789abcdef";
        fs::create_dir_all(state_dir.join("repos").join(valid_key)).unwrap();
        fs::create_dir_all(&data_dir).unwrap();

        let _s = EnvRestore::set("DMUXAI_STATE_DIR", &state_dir);
        let _d = EnvRestore::set("DMUXAI_DATA_DIR", &data_dir);

        assert!(lookup_repo_key("").is_none());
        assert!(lookup_repo_key(".").is_none());
        assert!(lookup_repo_key("..").is_none());
        assert!(lookup_repo_key("../etc").is_none());
        assert!(lookup_repo_key("demo/sub").is_none());
        assert!(lookup_repo_key("demo\\sub").is_none());
        assert!(lookup_repo_key("demo\0sub").is_none());
        assert!(lookup_repo_key("unknown-fedcba9876543210").is_none());

        let found = lookup_repo_key(valid_key).expect("enumerated key must resolve");
        assert_eq!(found.repo_key, valid_key);

        // Symlinked repo directories must be skipped by enumerate_repo_locations / lookup_repo_key.
        #[cfg(unix)]
        {
            let real_dir = tmp.path().join("outside");
            fs::create_dir_all(&real_dir).unwrap();
            let sym_key = "sym-0123456789abcdef";
            std::os::unix::fs::symlink(&real_dir, state_dir.join("repos").join(sym_key)).unwrap();
            assert!(lookup_repo_key(sym_key).is_none());
        }

        // Spoofed manifest.workspace_root hashing to a different repo_key must be ignored.
        let ws_real = tmp.path().join("real-repo");
        let ws_spoof = tmp.path().join("spoof-repo");
        fs::create_dir_all(&ws_real).unwrap();
        fs::create_dir_all(&ws_spoof).unwrap();
        let real_key = crate::state_path::repo_key(&ws_real).unwrap();
        let runs_dir = data_dir.join("repos").join(&real_key).join("runs");
        let store = JournalStore::new(runs_dir);
        let spoof_manifest = crate::run_journal::RunManifest {
            schema_version: 1,
            run_id: "1111222233334444".to_owned(),
            origin: "cli_headless".to_owned(),
            status: "completed".to_owned(),
            created_at_ms: 1000,
            completed_at_ms: Some(2000),
            duration_ms: Some(1000),
            requested_runner: None,
            resolved_runner: None,
            requested_model: None,
            resolved_model: None,
            dmuxai_version: None,
            build_commit: None,
            pid: None,
            dmuxai_exit_code: Some(0),
            child_exit_code: None,
            child_signal: None,
            error_kind: None,
            error_summary: None,
            links: Default::default(),
            diagnostics: Default::default(),
            end_summary: None,
            workspace_root: Some(ws_spoof.to_string_lossy().into_owned()),
        };
        store.save_relaxed(&spoof_manifest).unwrap();
        let real_loc = lookup_repo_key(&real_key).unwrap();
        assert!(resolve_workspace_root(&real_loc).is_none());
        assert!(build_project_item(&real_loc).canonical_path.is_none());
    }

    // TC-2987 / NFR-2980: listing uses bounded queries and caches for 5s.
    #[test]
    fn tc_2987_global_listing_is_cached_and_bounded() {
        let _lock = test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let state_dir = tmp.path().join("state");
        let data_dir = tmp.path().join("data");
        let ws1 = tmp.path().join("proj1");
        fs::create_dir_all(&ws1).unwrap();
        let key1 = crate::state_path::repo_key(&ws1).unwrap();

        let _s = EnvRestore::set("DMUXAI_STATE_DIR", &state_dir);
        let _d = EnvRestore::set("DMUXAI_DATA_DIR", &data_dir);

        let db1 = state_dir
            .join("repos")
            .join(&key1)
            .join("conversations.sqlite3");
        {
            let mut engine = crate::conversation::ConversationEngine::open(&db1, &ws1).unwrap();
            engine
                .select(crate::conversation::ConversationSelection::New {
                    title: Some("First".to_owned()),
                })
                .unwrap();
            let _ = engine
                .begin_turn(crate::conversation::BeginTurn {
                    agent: crate::conversation::AgentKey::Codex,
                    model_id: None,
                    prompt: "hello".to_owned(),
                    display_text: None,
                })
                .unwrap();
        }
        let summary = summarize_repo_db(&db1, &key1).expect("bounded summary");
        assert_eq!(summary.count, 1);

        // Drop `messages` table to prove summarize_repo_db does not scan messages.
        {
            let conn = rusqlite::Connection::open(&db1).unwrap();
            conn.execute("DROP TABLE messages", []).unwrap();
        }
        let summary_no_msgs =
            summarize_repo_db(&db1, &key1).expect("bounded summary without messages table");
        assert_eq!(summary_no_msgs.count, 1);

        let first = list_projects_cached();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].repo_key, key1);

        // Add a second repo on disk; within 5s TTL the cache returns the first snapshot.
        let ws2 = tmp.path().join("proj2");
        fs::create_dir_all(&ws2).unwrap();
        let key2 = crate::state_path::repo_key(&ws2).unwrap();
        fs::create_dir_all(data_dir.join("repos").join(&key2).join("runs")).unwrap();

        let second = list_projects_cached();
        assert_eq!(second.len(), 1, "expected 5s cache hit");

        // Invalidate cache and confirm the second repo appears.
        {
            let mut guard = PROJECT_LIST_CACHE.lock().unwrap();
            *guard = None;
        }
        let third = list_projects_cached();
        assert_eq!(third.len(), 2);
    }
}
