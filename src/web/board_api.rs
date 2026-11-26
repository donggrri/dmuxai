//! Decision board HTTP API (SPEC-decision-board FR-3120, FR-3121).

use std::convert::Infallible;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::board::BoardStore;
use crate::board::{
    BoardCaller, BoardError, BoardItem, BoardRules, DecideRequest, Lane, ListFilter, RulesStore,
    SayRequest, Verdict, default_actor_by, default_rules, edit_plan_from_json, resolve_actor,
    valid_item_id,
};
use crate::graph::{PlanStep, PlanTask};
use crate::web::server::{error_json, json_ok};

#[derive(Clone)]
pub struct BoardApiState {
    pub board_base: PathBuf,
    pub state_root: PathBuf,
}

pub fn state_root_from_services(services: &crate::web::server::DashboardServices) -> PathBuf {
    if let Some(env) = &services.trust_env
        && let Some(dir) = &env.dmuxai_state_dir
    {
        return dir.clone();
    }
    crate::state_path::state_base().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn board_base_from_services(services: &crate::web::server::DashboardServices) -> PathBuf {
    services
        .board_base
        .clone()
        .unwrap_or_else(|| state_root_from_services(services).join("board"))
}

pub fn router<S>(board_base: PathBuf, state_root: PathBuf) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let state = BoardApiState {
        board_base,
        state_root,
    };
    Router::new()
        .route("/api/board", get(board_list))
        .route("/api/board/events", get(board_events_sse))
        .route(
            "/api/board/rules",
            get(board_rules_get).put(board_rules_put),
        )
        .route("/api/board/{id}", get(board_get))
        .route("/api/board/{id}/decision", post(board_decision))
        .route("/api/board/{id}/messages", post(board_messages))
        .route("/api/board/{id}/plan", post(board_plan))
        .route("/api/board/{id}/lane", post(board_lane))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .with_state(state)
}

fn board_error_response(error: BoardError) -> Response {
    let status = match &error {
        BoardError::Usage(_) => StatusCode::BAD_REQUEST,
        BoardError::NotFound(_) => StatusCode::NOT_FOUND,
        BoardError::Conflict(_) => StatusCode::CONFLICT,
        BoardError::Forbidden(_) => StatusCode::FORBIDDEN,
        BoardError::Failed(_) => StatusCode::UNPROCESSABLE_ENTITY,
        BoardError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_json(status, &error.to_string())
}

fn open_store(base: &Path) -> Result<BoardStore, BoardError> {
    BoardStore::open(base.to_path_buf())
}

async fn with_store<T, F>(base: PathBuf, f: F) -> Result<T, BoardError>
where
    F: FnOnce(&BoardStore) -> Result<T, BoardError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let store = open_store(&base)?;
        f(&store)
    })
    .await
    .map_err(|e| BoardError::Internal(e.to_string()))?
}

#[derive(Deserialize, Default)]
struct ListQuery {
    #[serde(default)]
    lane: Vec<String>,
    workspace: Option<String>,
    #[serde(default)]
    all: Option<String>,
}

fn parse_lanes(raw: &[String]) -> Result<Vec<Lane>, BoardError> {
    let mut lanes = Vec::new();
    for value in raw {
        for part in value.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let lane = Lane::parse(part)
                .ok_or_else(|| BoardError::Usage(format!("invalid lane '{part}'")))?;
            lanes.push(lane);
        }
    }
    Ok(lanes)
}

fn parse_all_flag(raw: Option<&String>) -> bool {
    matches!(
        raw.map(|s| s.as_str()),
        Some("1") | Some("true") | Some("yes")
    )
}

#[derive(Serialize)]
struct BoardListResponse {
    items: Vec<BoardItem>,
}

async fn board_list(
    State(state): State<BoardApiState>,
    Query(query): Query<ListQuery>,
) -> Response {
    let lanes = match parse_lanes(&query.lane) {
        Ok(lanes) => lanes,
        Err(error) => return board_error_response(error),
    };
    let workspace = query.workspace.map(PathBuf::from);
    let include_all = parse_all_flag(query.all.as_ref());
    let filter = ListFilter {
        lanes,
        workspace,
        include_all,
    };
    match with_store(state.board_base.clone(), move |store| store.list(&filter)).await {
        Ok(items) => json_ok(StatusCode::OK, &BoardListResponse { items }),
        Err(error) => board_error_response(error),
    }
}

fn item_response(item: BoardItem) -> serde_json::Value {
    serde_json::to_value(&item).unwrap_or(serde_json::json!({}))
}

async fn board_get(State(state): State<BoardApiState>, AxumPath(id): AxumPath<String>) -> Response {
    if !valid_item_id(&id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid board item id");
    }
    match with_store(state.board_base.clone(), move |store| store.get(&id)).await {
        Ok(item) => json_ok(StatusCode::OK, &item_response(item)),
        Err(error) => board_error_response(error),
    }
}

#[derive(Deserialize)]
struct DecisionBody {
    verdict: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    option: Option<usize>,
    #[serde(default)]
    plan_hash: Option<String>,
}

async fn board_decision(
    State(state): State<BoardApiState>,
    AxumPath(id): AxumPath<String>,
    axum::Json(body): axum::Json<DecisionBody>,
) -> Response {
    if !valid_item_id(&id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid board item id");
    }
    let verdict = match Verdict::parse(&body.verdict) {
        Some(v) => v,
        None => return error_json(StatusCode::BAD_REQUEST, "invalid verdict"),
    };
    let by = match resolve_actor(None, BoardCaller::Dashboard) {
        Ok(actor) => actor,
        Err(error) => return board_error_response(error),
    };
    let req = DecideRequest {
        verdict,
        text: body.text,
        option: body.option,
        plan_hash: body.plan_hash,
        by,
    };
    match with_store(state.board_base.clone(), move |store| {
        store.decide(&id, req)
    })
    .await
    {
        Ok(item) => json_ok(StatusCode::OK, &item_response(item)),
        Err(error) => board_error_response(error),
    }
}

#[derive(Deserialize)]
struct MessageBody {
    text: String,
}

async fn board_messages(
    State(state): State<BoardApiState>,
    AxumPath(id): AxumPath<String>,
    axum::Json(body): axum::Json<MessageBody>,
) -> Response {
    if !valid_item_id(&id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid board item id");
    }
    let by = default_actor_by(BoardCaller::Dashboard);
    let req = SayRequest {
        text: body.text,
        from: by,
    };
    match with_store(state.board_base.clone(), move |store| store.say(&id, req)).await {
        Ok(item) => json_ok(StatusCode::OK, &item_response(item)),
        Err(error) => board_error_response(error),
    }
}

#[derive(Deserialize)]
struct PlanBody {
    steps: Vec<PlanStep>,
    #[serde(default)]
    tasks: Option<Vec<PlanTask>>,
}

async fn board_plan(
    State(state): State<BoardApiState>,
    AxumPath(id): AxumPath<String>,
    axum::Json(body): axum::Json<PlanBody>,
) -> Response {
    if !valid_item_id(&id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid board item id");
    }
    let state_root = state.state_root.clone();
    let board_base = state.board_base.clone();
    let item_id = id.clone();
    match with_store(board_base.clone(), move |store| store.get(&item_id)).await {
        Ok(item) => {
            let workspace = item
                .workspace
                .path
                .as_ref()
                .map(PathBuf::from)
                .filter(|p| p.is_dir())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            let prompt = item
                .plan
                .as_ref()
                .map(|p| p.prompt.clone())
                .unwrap_or_default();
            let mut doc = serde_json::json!({ "steps": body.steps });
            if let Some(tasks) = body.tasks {
                doc["tasks"] = serde_json::to_value(tasks).unwrap_or(serde_json::json!([]));
            }
            let json_text = match serde_json::to_string(&doc) {
                Ok(s) => s,
                Err(error) => {
                    return board_error_response(BoardError::Internal(error.to_string()));
                }
            };
            let edit = edit_plan_from_json(&workspace, &state_root, &prompt, &json_text);
            match edit {
                Ok((_pending, hash)) => {
                    let edit_id = id.clone();
                    match with_store(board_base, move |store| {
                        store.edit_plan(&edit_id, &hash, &workspace, &state_root)
                    })
                    .await
                    {
                        Ok(item) => json_ok(StatusCode::OK, &item_response(item)),
                        Err(error) => board_error_response(error),
                    }
                }
                Err(error) => board_error_response(error),
            }
        }
        Err(error) => board_error_response(error),
    }
}

#[derive(Deserialize)]
struct LaneBody {
    lane: String,
}

async fn board_lane(
    State(state): State<BoardApiState>,
    AxumPath(id): AxumPath<String>,
    axum::Json(body): axum::Json<LaneBody>,
) -> Response {
    if !valid_item_id(&id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid board item id");
    }
    let lane = match Lane::parse(&body.lane) {
        Some(lane) => lane,
        None => return error_json(StatusCode::BAD_REQUEST, "invalid lane"),
    };
    let by = default_actor_by(BoardCaller::Dashboard);
    match with_store(state.board_base.clone(), move |store| {
        store.move_lane(&id, lane, &by)
    })
    .await
    {
        Ok(item) => json_ok(StatusCode::OK, &item_response(item)),
        Err(error) => board_error_response(error),
    }
}

async fn board_rules_get(State(state): State<BoardApiState>) -> Response {
    let base = state.board_base.clone();
    let rules = tokio::task::spawn_blocking(move || RulesStore::new(&base).load())
        .await
        .unwrap_or_else(|e| {
            eprintln!("board rules load: {e}");
            default_rules()
        });
    json_ok(StatusCode::OK, &rules)
}

async fn board_rules_put(
    State(state): State<BoardApiState>,
    axum::Json(body): axum::Json<BoardRules>,
) -> Response {
    if let Err(error) = body.validate() {
        return board_error_response(error);
    }
    let base = state.board_base.clone();
    let saved = body.clone();
    match tokio::task::spawn_blocking(move || RulesStore::new(&base).save(&body)).await {
        Ok(Ok(())) => json_ok(StatusCode::OK, &saved),
        Ok(Err(error)) => board_error_response(error),
        Err(error) => board_error_response(BoardError::Internal(error.to_string())),
    }
}

#[derive(Deserialize, Default)]
struct EventsQuery {
    since: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BoardEventLine {
    seq: u64,
    item_id: String,
    kind: String,
    version: u32,
    by: String,
}

fn events_path(base: &Path) -> PathBuf {
    base.join("events.jsonl")
}

fn read_events_since(path: &Path, since: u64) -> (Vec<BoardEventLine>, u64) {
    if !path.exists() {
        return (Vec::new(), 0);
    }
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return (Vec::new(), 0),
    };
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    let mut max_seq = since;
    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<BoardEventLine>(line)
            && event.seq > since
        {
            max_seq = max_seq.max(event.seq);
            out.push(event);
        }
    }
    (out, max_seq)
}

fn file_offset_end(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn read_new_events_from_offset(
    path: &Path,
    offset: &mut u64,
    cursor: &mut u64,
) -> Vec<BoardEventLine> {
    let mut out = Vec::new();
    let Ok(meta) = fs::metadata(path) else {
        return out;
    };
    let len = meta.len();
    if *offset > len {
        *offset = len;
        return out;
    }
    let Ok(mut file) = File::open(path) else {
        return out;
    };
    if file.seek(SeekFrom::Start(*offset)).is_err() {
        return out;
    }
    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<BoardEventLine>(line)
            && event.seq > *cursor
        {
            *cursor = (*cursor).max(event.seq);
            out.push(event);
        }
    }
    *offset = len;
    out
}

async fn board_events_sse(
    State(state): State<BoardApiState>,
    Query(params): Query<EventsQuery>,
) -> Response {
    let since = match params.since.as_deref() {
        None | Some("") => 0,
        Some(value) => match value.parse::<u64>() {
            Ok(n) => n,
            Err(_) => return error_json(StatusCode::BAD_REQUEST, "invalid since"),
        },
    };
    let path = events_path(&state.board_base);
    let (tx, rx) = mpsc::unbounded_channel::<Result<Event, Infallible>>();
    tokio::spawn(async move {
        let (replay, mut cursor) = read_events_since(&path, since);
        let mut offset = file_offset_end(&path);
        for event in replay {
            let payload = serde_json::json!({
                "seq": event.seq,
                "item_id": event.item_id,
                "kind": event.kind,
                "version": event.version,
                "by": event.by,
            });
            if tx
                .send(Ok(Event::default()
                    .event("board")
                    .data(payload.to_string())))
                .is_err()
            {
                return;
            }
            cursor = cursor.max(event.seq);
        }
        let mut poll = interval(Duration::from_secs(1));
        poll.tick().await;
        loop {
            if tx.is_closed() {
                return;
            }
            for event in read_new_events_from_offset(&path, &mut offset, &mut cursor) {
                let payload = serde_json::json!({
                    "seq": event.seq,
                    "item_id": event.item_id,
                    "kind": event.kind,
                    "version": event.version,
                    "by": event.by,
                });
                if tx
                    .send(Ok(Event::default()
                        .event("board")
                        .data(payload.to_string())))
                    .is_err()
                {
                    return;
                }
            }
            poll.tick().await;
        }
    });
    Sse::new(UnboundedReceiverStream::new(rx))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::AddItemRequest;
    use crate::board::Kind;
    use crate::web::hub::DashboardHub;
    use crate::web::server::{DashboardServices, build_router_with};
    use crate::web::snapshot::SnapshotSource;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_router(board_base: PathBuf, state_root: PathBuf, token: &str) -> axum::Router {
        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        let services = DashboardServices {
            board_base: Some(board_base),
            trust_env: Some(crate::runtime_env::RuntimeEnv {
                dmuxai_state_dir: Some(state_root.clone()),
                ..crate::runtime_env::RuntimeEnv::from_process()
            }),
            ..DashboardServices::default()
        };
        build_router_with(
            hub,
            token.to_string(),
            Some(vec![
                "localhost:7420".to_string(),
                "127.0.0.1:7420".to_string(),
            ]),
            services,
        )
    }

    async fn body_string(resp: axum::response::Response) -> String {
        String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes()).into_owned()
    }

    async fn first_sse_text(resp: axum::response::Response) -> Option<String> {
        use http_body_util::BodyExt;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut body = resp.into_body();
        let mut buf = Vec::new();
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let frame = match tokio::time::timeout(remaining, body.frame()).await {
                Ok(Some(Ok(frame))) => frame,
                _ => break,
            };
            if let Some(chunk) = frame.data_ref() {
                buf.extend_from_slice(chunk);
                let text = String::from_utf8_lossy(&buf);
                if text.contains("event: board") {
                    return Some(text.into_owned());
                }
            }
        }
        if buf.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(&buf).into_owned())
        }
    }

    fn seed_item(store: &BoardStore, state: &Path) -> String {
        let item = store
            .add(AddItemRequest {
                title: "decide me".to_owned(),
                summary: "summary".to_owned(),
                kind: Kind::Feature,
                grade: None,
                lane: None,
                source_bot: Some("coord".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(PathBuf::from("/workspace")),
                recommendation: Some("ship".to_owned()),
                options: vec!["A".to_owned()],
                plan_hash: None,
                by: "bot:coord".to_owned(),
                state_root: state.to_path_buf(),
            })
            .unwrap();
        item.id
    }

    // TC-3120 / FR-3120: HTTP routes, codes, dashboard owner actor.
    #[tokio::test]
    async fn tc_3120_board_http_routes_status_codes_and_owner_actor() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let store = BoardStore::open(board.clone()).unwrap();
        let id = seed_item(&store, &state);
        let token = "boardtesttoken";
        let router = test_router(board.clone(), state.clone(), token);

        let bad_id = axum::http::Request::builder()
            .uri("/api/board/not-an-id")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(bad_id).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let get = axum::http::Request::builder()
            .uri(format!("/api/board/{id}"))
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(get).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let decide = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/board/{id}/decision"))
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                r#"{"verdict":"approve","plan_hash":"0000000000000000"}"#,
            ))
            .unwrap();
        let resp = router.clone().oneshot(decide).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);

        let missing = axum::http::Request::builder()
            .uri("/api/board/b000000000001")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(missing).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let list = axum::http::Request::builder()
            .uri("/api/board")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(list).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains(&id));
    }

    // TC-3121 / FR-3121: SSE replay and tail.
    #[tokio::test]
    async fn tc_3121_board_sse_replays_since_and_streams_new_events() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let store = BoardStore::open(board.clone()).unwrap();
        let id = seed_item(&store, &state);
        let _ = store
            .say(
                &id,
                SayRequest {
                    text: "hello".to_owned(),
                    from: "owner".to_owned(),
                },
            )
            .unwrap();
        let token = "boardtesttoken";
        let router = test_router(board, state, token);
        let req = axum::http::Request::builder()
            .uri("/api/board/events?since=0")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "text/event-stream")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .contains("text/event-stream")
        );
        let body = first_sse_text(resp)
            .await
            .expect("expected at least one board sse frame");
        assert!(body.contains("event: board"));
        assert!(body.contains(&id));
    }

    // TC-3126 / FR-3126: rules GET/PUT validation.
    #[tokio::test]
    async fn tc_3126_board_rules_get_put_validation() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let _ = BoardStore::open(board.clone()).unwrap();
        let token = "boardtesttoken";
        let router = test_router(board, state, token);

        let get = axum::http::Request::builder()
            .uri("/api/board/rules")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(get).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let put = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/board/rules")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                r#"{"default_lane":"needs_decision","rules":[{"kind":"debug","lane":"auto"}]}"#,
            ))
            .unwrap();
        let resp = router.clone().oneshot(put).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bad = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/board/rules")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                r#"{"default_lane":"nope","rules":[]}"#,
            ))
            .unwrap();
        let resp = router.oneshot(bad).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // TC-3127 / NFR-3120: list 500 items under 300 ms over HTTP.
    #[tokio::test]
    async fn tc_3127_board_http_list_500_under_budget() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let store = BoardStore::open(board.clone()).unwrap();
        for i in 0..500 {
            store
                .add(AddItemRequest {
                    title: format!("item-{i}"),
                    summary: String::new(),
                    kind: Kind::Debug,
                    grade: None,
                    lane: None,
                    source_bot: None,
                    source_channel: None,
                    source_ref: None,
                    workspace: None,
                    recommendation: None,
                    options: vec![],
                    plan_hash: None,
                    by: "bot:test".to_owned(),
                    state_root: state.clone(),
                })
                .unwrap();
        }
        let token = "boardtesttoken";
        let router = test_router(board, state, token);
        let start = std::time::Instant::now();
        let req = axum::http::Request::builder()
            .uri("/api/board")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            start.elapsed().as_millis() < 300,
            "list took {} ms",
            start.elapsed().as_millis()
        );
    }
}
