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
    unmasked_prompt_from_item_plan, valid_item_id,
};
use crate::graph::spec_review::{
    DecideRequest as SpecDecideRequest, ReviewError, SpecReviewStore, Verdict as SpecVerdict,
};
use crate::graph::{PlanStep, PlanTask};
use crate::web::server::{error_json, json_ok};

#[derive(Clone)]
pub struct BoardApiState {
    pub board_base: PathBuf,
    pub state_root: PathBuf,
    pub launch_allowed: bool,
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

pub fn router<S>(board_base: PathBuf, state_root: PathBuf, launch_allowed: bool) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let state = BoardApiState {
        board_base,
        state_root,
        launch_allowed,
    };
    Router::new()
        .route("/api/board", get(board_list))
        .route("/api/board/pending", get(board_pending))
        .route("/api/board/events", get(board_events_sse))
        .route(
            "/api/board/rules",
            get(board_rules_get).put(board_rules_put),
        )
        .route("/api/board/{id}", get(board_get))
        .route("/api/board/{id}/decision", post(board_decision))
        .route("/api/board/{id}/spec-review", post(board_spec_review))
        .route("/api/board/{id}/check", post(board_check))
        .route("/api/board/{id}/messages", post(board_messages))
        .route("/api/board/{id}/plan", post(board_plan))
        .route("/api/board/{id}/lane", post(board_lane))
        .route("/api/board/{id}/ask", post(board_ask))
        .route("/api/board/{id}/run", post(board_run))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .with_state(state)
}

/// Shared human-only rule (SPEC-decision-board FR-3138, SPEC-dashboard-feature-coverage
/// FR-3762): browser fetch metadata plus `X-Dmuxai-Human: 1`.
pub(crate) fn human_browser_rejection(headers: &axum::http::HeaderMap) -> Option<Response> {
    let sec_site = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .map(str::trim);
    let sec_mode = headers
        .get("sec-fetch-mode")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let human = headers
        .get("x-dmuxai-human")
        .and_then(|v| v.to_str().ok())
        .map(str::trim);
    if sec_site == Some("same-origin") && sec_mode.is_some() && human == Some("1") {
        None
    } else {
        Some(error_json(
            StatusCode::FORBIDDEN,
            "human browser headers required for this action",
        ))
    }
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
    launch_allowed: bool,
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
    let launch_allowed = state.launch_allowed;
    match with_store(state.board_base.clone(), move |store| store.list(&filter)).await {
        Ok(items) => json_ok(
            StatusCode::OK,
            &BoardListResponse {
                items,
                launch_allowed,
            },
        ),
        Err(error) => board_error_response(error),
    }
}

#[derive(Deserialize, Default)]
struct PendingQuery {
    workspace: Option<String>,
}

async fn board_pending(
    State(state): State<BoardApiState>,
    Query(query): Query<PendingQuery>,
) -> Response {
    let workspace = query.workspace.map(PathBuf::from);
    match with_store(state.board_base.clone(), move |store| {
        store.pending(workspace.as_deref())
    })
    .await
    {
        Ok(pending) => json_ok(StatusCode::OK, &pending),
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
    headers: axum::http::HeaderMap,
    AxumPath(id): AxumPath<String>,
    axum::Json(body): axum::Json<DecisionBody>,
) -> Response {
    if let Some(resp) = human_browser_rejection(&headers) {
        return resp;
    }
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
    let launch_allowed = state.launch_allowed;
    let state_root = state.state_root.clone();
    match with_store(state.board_base.clone(), move |store| {
        let mut item = store.decide_without_notify(&id, req)?;
        let auto_run = if verdict == Verdict::Approve
            && item.decision.as_ref().is_some_and(|d| d.by == "owner")
        {
            let outcome = crate::board::runner::auto_run_after_owner_approve(
                store,
                &item,
                &state_root,
                launch_allowed,
            );
            if (outcome.started || outcome.reason.is_some())
                && let Ok(updated) = store.get(&id)
            {
                item = updated;
            }
            outcome
        } else {
            crate::board::runner::AutoRunOutcome::not_started()
        };
        let actor = item
            .decision
            .as_ref()
            .map(|d| d.by.as_str())
            .unwrap_or("owner");
        crate::board::notify::dispatch_decision_event(store.base(), &item, actor, auto_run.started);
        Ok((item, auto_run))
    })
    .await
    {
        Ok((item, auto_run)) => {
            let mut value = item_response(item);
            if let Some(map) = value.as_object_mut() {
                map.insert(
                    "auto_run".to_owned(),
                    serde_json::to_value(&auto_run)
                        .unwrap_or_else(|_| serde_json::json!({ "started": false })),
                );
            }
            json_ok(StatusCode::OK, &value)
        }
        Err(error) => board_error_response(error),
    }
}

#[derive(Deserialize)]
struct SpecReviewBody {
    verdict: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    spec_digest: Option<String>,
}

fn spec_review_error_response(error: ReviewError) -> Response {
    let status = match &error {
        ReviewError::Usage(_) => StatusCode::BAD_REQUEST,
        ReviewError::NotFound(_) => StatusCode::NOT_FOUND,
        ReviewError::Conflict(_) => StatusCode::CONFLICT,
        ReviewError::Forbidden(_) => StatusCode::FORBIDDEN,
        ReviewError::Corrupt(_) | ReviewError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_json(status, &error.to_string())
}

/// Decides the pending spec review of a board item's run
/// (SPEC-spec-review-gate FR-3707, FR-3708, FR-3713).
async fn board_spec_review(
    State(state): State<BoardApiState>,
    headers: axum::http::HeaderMap,
    AxumPath(id): AxumPath<String>,
    axum::Json(body): axum::Json<SpecReviewBody>,
) -> Response {
    if let Some(resp) = human_browser_rejection(&headers) {
        return resp;
    }
    if !valid_item_id(&id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid board item id");
    }
    let Some(verdict) = SpecVerdict::parse(&body.verdict) else {
        return error_json(StatusCode::BAD_REQUEST, "invalid verdict");
    };
    let text = body
        .text
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());
    if verdict == SpecVerdict::RequestChanges && text.is_none() {
        return error_json(
            StatusCode::BAD_REQUEST,
            "request changes needs non-empty feedback",
        );
    }
    let by = match resolve_actor(None, BoardCaller::Dashboard) {
        Ok(actor) => actor,
        Err(error) => return board_error_response(error),
    };
    let reviews = SpecReviewStore::new(&state.state_root);
    let result = tokio::task::spawn_blocking(move || -> Result<BoardItem, Box<Response>> {
        let store = open_store(&state.board_base).map_err(|e| Box::new(board_error_response(e)))?;
        let item = store
            .get(&id)
            .map_err(|e| Box::new(board_error_response(e)))?;
        let Some(review) = item.spec_review else {
            return Err(Box::new(error_json(
                StatusCode::NOT_FOUND,
                "item has no spec review",
            )));
        };
        let record = reviews
            .decide(
                &review.review_id,
                &SpecDecideRequest {
                    verdict,
                    text: text.as_deref(),
                    spec_digest: body.spec_digest.as_deref(),
                    by: &by,
                },
            )
            .map_err(|e| Box::new(spec_review_error_response(e)))?;
        let updated = crate::board::BoardSpecReview {
            status: record.status,
            ..review
        };
        store
            .set_spec_review(&id, updated, None)
            .map_err(|e| Box::new(board_error_response(e)))
    })
    .await;
    match result {
        Ok(Ok(item)) => json_ok(StatusCode::OK, &item_response(item)),
        Ok(Err(resp)) => *resp,
        Err(error) => error_json(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

#[derive(Deserialize, Default)]
struct CheckBody {
    #[serde(default)]
    text: Option<String>,
}

async fn board_check(
    State(state): State<BoardApiState>,
    headers: axum::http::HeaderMap,
    AxumPath(id): AxumPath<String>,
    body: Option<axum::Json<CheckBody>>,
) -> Response {
    if let Some(resp) = human_browser_rejection(&headers) {
        return resp;
    }
    if !valid_item_id(&id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid board item id");
    }
    let by = match resolve_actor(None, BoardCaller::Dashboard) {
        Ok(actor) => actor,
        Err(error) => return board_error_response(error),
    };
    let text = body.and_then(|axum::Json(b)| b.text);
    match with_store(state.board_base.clone(), move |store| {
        store.check(&id, text, &by)
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
    let by = default_actor_by(BoardCaller::Dashboard);
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
            let stored_prompt = match unmasked_prompt_from_item_plan(
                &state_root,
                &board_base,
                item.plan.as_ref(),
            ) {
                Ok(Some(prompt)) => prompt,
                Ok(None) => String::new(),
                Err(error) => return board_error_response(error),
            };
            if stored_prompt.trim().is_empty() {
                return board_error_response(BoardError::Usage(
                    "plan edit requires an existing plan or prompt".to_owned(),
                ));
            }
            let inherit = item.plan.as_ref().map(|p| p.tasks.clone());
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
            let edit =
                edit_plan_from_json(&workspace, &state_root, &stored_prompt, &json_text, inherit);
            match edit {
                Ok((_pending, hash)) => {
                    let edit_id = id.clone();
                    let by = by.clone();
                    match with_store(board_base, move |store| {
                        store.edit_plan(&edit_id, &hash, &workspace, &state_root, &by)
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
    headers: axum::http::HeaderMap,
    AxumPath(id): AxumPath<String>,
    axum::Json(body): axum::Json<LaneBody>,
) -> Response {
    if let Some(resp) = human_browser_rejection(&headers) {
        return resp;
    }
    if !valid_item_id(&id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid board item id");
    }
    let lane = match Lane::parse(&body.lane) {
        Some(lane) => lane,
        None => return error_json(StatusCode::BAD_REQUEST, "invalid lane"),
    };
    let by = default_actor_by(BoardCaller::Dashboard);
    match with_store(state.board_base.clone(), move |store| {
        store.move_lane(&id, lane, &by, None)
    })
    .await
    {
        Ok(item) => json_ok(StatusCode::OK, &item_response(item)),
        Err(error) => board_error_response(error),
    }
}

#[derive(Deserialize)]
struct AskBody {
    text: String,
    #[serde(default)]
    revise: bool,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

async fn board_ask(
    State(state): State<BoardApiState>,
    AxumPath(id): AxumPath<String>,
    axum::Json(body): axum::Json<AskBody>,
) -> Response {
    if !state.launch_allowed {
        return error_json(
            StatusCode::FORBIDDEN,
            "view-only dashboard cannot run board asks",
        );
    }
    if !valid_item_id(&id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid board item id");
    }
    let by = match resolve_actor(None, BoardCaller::Dashboard) {
        Ok(actor) => actor,
        Err(error) => return board_error_response(error),
    };
    let board_base = state.board_base.clone();
    let state_root = state.state_root.clone();
    let params = crate::board::runner::AskParams {
        id,
        text: body.text,
        revise: body.revise,
        agent: body.agent,
        model: body.model,
        by,
        state_root,
    };
    match with_store(board_base.clone(), move |store| {
        crate::board::runner::ask_item_prepare(store, params)
    })
    .await
    {
        Ok(prepared) => {
            let item = prepared.item.clone();
            std::thread::spawn(move || {
                if let Ok(bg_store) = open_store(&board_base) {
                    let _ = crate::board::runner::ask_item_execute(&bg_store, prepared);
                }
            });
            json_ok(StatusCode::ACCEPTED, &item_response(item))
        }
        Err(error) => board_error_response(error),
    }
}

#[derive(Deserialize, Default)]
struct RunBody {
    #[serde(default)]
    plan_hash: Option<String>,
}

async fn board_run(
    State(state): State<BoardApiState>,
    AxumPath(id): AxumPath<String>,
    body: Option<axum::Json<RunBody>>,
) -> Response {
    if !state.launch_allowed {
        return error_json(
            StatusCode::FORBIDDEN,
            "view-only dashboard cannot start board runs",
        );
    }
    if !valid_item_id(&id) {
        return error_json(StatusCode::BAD_REQUEST, "invalid board item id");
    }
    let by = match resolve_actor(None, BoardCaller::Dashboard) {
        Ok(actor) => actor,
        Err(error) => return board_error_response(error),
    };
    let expected_plan_hash = body.and_then(|axum::Json(b)| b.plan_hash);
    let board_base = state.board_base.clone();
    let state_root = state.state_root.clone();
    let params = crate::board::runner::RunParams {
        id,
        expected_plan_hash,
        by,
        state_root,
    };
    match with_store(board_base.clone(), move |store| {
        crate::board::runner::run_item_prepare(store, params)
    })
    .await
    {
        Ok(prepared) => {
            let item = prepared.item.clone();
            std::thread::spawn(move || {
                if let Ok(bg_store) = open_store(&board_base) {
                    let _ = crate::board::runner::run_item_execute(&bg_store, prepared);
                }
            });
            json_ok(StatusCode::ACCEPTED, &item_response(item))
        }
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
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<BoardRules>,
) -> Response {
    if let Some(resp) = human_browser_rejection(&headers) {
        return resp;
    }
    if let Err(error) = body.validate() {
        return board_error_response(error);
    }
    let saved = body.clone();
    match with_store(state.board_base.clone(), move |store| {
        store.with_rules_lock(|| RulesStore::new(store.base()).save(&body))
    })
    .await
    {
        Ok(()) => json_ok(StatusCode::OK, &saved),
        Err(error) => board_error_response(error),
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
                key: None,
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
            .header("Sec-Fetch-Site", "same-origin")
            .header("Sec-Fetch-Mode", "cors")
            .header("X-Dmuxai-Human", "1")
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
            .header("Sec-Fetch-Site", "same-origin")
            .header("Sec-Fetch-Mode", "cors")
            .header("X-Dmuxai-Human", "1")
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
            .header("Sec-Fetch-Site", "same-origin")
            .header("Sec-Fetch-Mode", "cors")
            .header("X-Dmuxai-Human", "1")
            .body(axum::body::Body::from(
                r#"{"default_lane":"nope","rules":[]}"#,
            ))
            .unwrap();
        let resp = router.oneshot(bad).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // TC-3756 / FR-3756: a scripted PUT without the human browser headers is
    // refused with 403 and changes nothing; GET shows the saved rules.
    #[tokio::test]
    async fn tc_3756_rules_put_requires_human_headers() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let _ = BoardStore::open(board.clone()).unwrap();
        let token = "boardtesttoken";
        let router = test_router(board, state, token);
        let rules_get = |router: axum::Router| async move {
            let req = axum::http::Request::builder()
                .uri("/api/board/rules")
                .header("Host", "localhost:7420")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap();
            let resp = router.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let bytes = http_body_util::BodyExt::collect(resp.into_body())
                .await
                .unwrap()
                .to_bytes();
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
        };
        let put = |headers: &[(&str, &str)], body: &str| {
            let mut b = axum::http::Request::builder()
                .method("PUT")
                .uri("/api/board/rules")
                .header("Host", "localhost:7420")
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json");
            for (name, value) in headers {
                b = b.header(*name, *value);
            }
            b.body(axum::body::Body::from(body.to_owned())).unwrap()
        };
        let human = [
            ("Sec-Fetch-Site", "same-origin"),
            ("Sec-Fetch-Mode", "cors"),
            ("X-Dmuxai-Human", "1"),
        ];
        let body = r#"{"default_lane":"needs_decision","rules":[{"kind":"debug","lane":"auto"}]}"#;
        let before = rules_get(router.clone()).await;

        // Failure: scripted, cross-site, no mode, wrong marker.
        let refused: [&[(&str, &str)]; 5] = [
            &[],
            &[("X-Dmuxai-Human", "1")],
            &[
                ("Sec-Fetch-Site", "same-origin"),
                ("Sec-Fetch-Mode", "cors"),
            ],
            &[
                ("Sec-Fetch-Site", "cross-site"),
                ("Sec-Fetch-Mode", "cors"),
                ("X-Dmuxai-Human", "1"),
            ],
            &[
                ("Sec-Fetch-Site", "same-origin"),
                ("Sec-Fetch-Mode", "cors"),
                ("X-Dmuxai-Human", "yes"),
            ],
        ];
        for headers in refused {
            let resp = router.clone().oneshot(put(headers, body)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{headers:?}");
        }
        assert_eq!(rules_get(router.clone()).await, before, "403 saves nothing");

        // Success: the human headers save and GET returns the new rules.
        let resp = router.clone().oneshot(put(&human, body)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let after = rules_get(router.clone()).await;
        assert_eq!(after["rules"][0]["kind"], "debug");
        assert_eq!(after["rules"][0]["lane"], "auto");

        // Boundary: invalid rule with human headers is 400 and keeps the saved rules.
        let resp = router
            .clone()
            .oneshot(put(&human, r#"{"default_lane":"nope","rules":[]}"#))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(rules_get(router).await, after);
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
                    key: None,
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

    // FR-3132, FR-3133: POST /api/board/{id}/ask and /run return 403 in view-only
    // `serve`, and 202 Accepted when launch_allowed (`--web`).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn board_http_ask_and_run_view_only_vs_web() {
        let _guard = crate::state_path::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let store = BoardStore::open(board.clone()).unwrap();
        let item = store
            .add(AddItemRequest {
                title: "http ask/run".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: Some("bot".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(ws.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let plan_json = r#"{"tasks":[{"id":"task-1","title":"do it","done_check":"done","size":"small","depends_on":[]}],"steps":[{"id":"step-1","role":"implement","job":"edit","runner":"cursor","depends_on":[],"task":"task-1"}]}"#;
        let (_, hash) = edit_plan_from_json(&ws, &state, "ship it", plan_json, None).unwrap();
        store
            .attach_plan(&item.id, &hash, &state, "bot:test")
            .unwrap();
        store
            .decide(
                &item.id,
                DecideRequest {
                    verdict: Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: Some(hash.clone()),
                    by: "owner".to_owned(),
                },
            )
            .unwrap();

        let token = "boardtesttoken";
        // View-only (`web_attached: false`) -> 403
        let view_only = test_router(board.clone(), state.clone(), token);
        let resp = view_only
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/board/{}/ask", item.id))
                    .header("Host", "localhost:7420")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(r#"{"text":"why?"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let resp = view_only
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/board/{}/run", item.id))
                    .header("Host", "localhost:7420")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // `--web` (`web_attached: true`) -> 202
        let fake_bin = dir.path().join("fake.sh");
        std::fs::write(
            &fake_bin,
            "#!/bin/sh\ncat > /dev/null\nsleep 0.1\nprintf '{\"ok\":true,\"exit_code\":0,\"run_id\":\"fedcba9876543210\",\"text\":\"ok\"}\\n'\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_bin).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_bin, perms).unwrap();
        }
        unsafe { std::env::set_var("DMUXAI_BOARD_CHILD_BIN", &fake_bin) };

        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        let services = DashboardServices {
            board_base: Some(board),
            web_attached: true,
            trust_env: Some(crate::runtime_env::RuntimeEnv {
                dmuxai_state_dir: Some(state),
                ..crate::runtime_env::RuntimeEnv::from_process()
            }),
            ..DashboardServices::default()
        };
        let web_router = build_router_with(
            hub,
            token.to_string(),
            Some(vec!["localhost:7420".to_string()]),
            services,
        );

        let resp = web_router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/board/{}/ask", item.id))
                    .header("Host", "localhost:7420")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(r#"{"text":"why?"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let ask_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let current = store.get(&item.id).unwrap();
            if current.thread.iter().any(|m| m.from == "planner") {
                break;
            }
            assert!(
                std::time::Instant::now() < ask_deadline,
                "timed out waiting for background ask"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let resp = web_router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/board/{}/run", item.id))
                    .header("Host", "localhost:7420")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(format!(
                        r#"{{"plan_hash":"{hash}"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        // Second concurrent run while in_progress -> 409 Conflict.
        let resp_conflict = web_router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/board/{}/run", item.id))
                    .header("Host", "localhost:7420")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(format!(
                        r#"{{"plan_hash":"{hash}"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_conflict.status(), StatusCode::CONFLICT);

        let run_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let current = store.get(&item.id).unwrap();
            if current.lane == Lane::Done {
                assert_eq!(current.run_ids, vec!["fedcba9876543210".to_owned()]);
                break;
            }
            assert!(
                std::time::Instant::now() < run_deadline,
                "timed out waiting for background run"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        unsafe { std::env::remove_var("DMUXAI_BOARD_CHILD_BIN") };
    }

    // TC-3138 / FR-3138, OQ-3104: human-only enforcement across store, CLI, MCP, HTTP, and run.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn tc_3138_human_only_decisions_cli_mcp_http_and_run_never_approves() {
        let _guard = crate::state_path::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let store = BoardStore::open(board.clone()).unwrap();

        let plan_json = r#"{"tasks":[{"id":"task-1","title":"do it","done_check":"done","size":"small","depends_on":[]}],"steps":[{"id":"step-1","role":"implement","job":"edit","runner":"cursor","depends_on":[],"task":"task-1"}]}"#;
        let (_, hash) = edit_plan_from_json(&ws, &state, "ship it", plan_json, None).unwrap();

        let auto_item = store
            .add(AddItemRequest {
                title: "auto item".to_owned(),
                summary: String::new(),
                kind: Kind::Debug,
                grade: None,
                lane: None,
                source_bot: Some("coord".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(ws.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: Some(hash.clone()),
                key: None,
                by: "bot:coord".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        assert_eq!(auto_item.lane, Lane::Auto);

        // 1. Bot decide in auto lane -> exit 5 (Forbidden).
        let bot_decide = store
            .decide(
                &auto_item.id,
                DecideRequest {
                    verdict: Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: Some(hash.clone()),
                    by: "bot:coord".to_owned(),
                },
            )
            .unwrap_err();
        assert_eq!(bot_decide.exit_code(), crate::agent_api::exit::FORBIDDEN);

        // 2. Move into approved/rejected is refused even for owner.
        let move_approved = store
            .move_lane(&auto_item.id, Lane::Approved, "owner", None)
            .unwrap_err();
        assert_eq!(move_approved.exit_code(), crate::agent_api::exit::FORBIDDEN);
        let move_rejected = store
            .move_lane(&auto_item.id, Lane::Rejected, "owner", None)
            .unwrap_err();
        assert_eq!(move_rejected.exit_code(), crate::agent_api::exit::FORBIDDEN);

        // 3. CLI confirmation mismatch -> refused (exit 5); non-TTY owner -> exit 5.
        let mismatch =
            crate::board::confirm_owner_action(&auto_item.id, "approve", "auto item", |_| {
                Ok("wrong-id".to_owned())
            })
            .unwrap_err();
        assert_eq!(mismatch.exit_code(), crate::agent_api::exit::FORBIDDEN);
        assert!(
            crate::board::confirm_owner_action(&auto_item.id, "approve", "auto item", |_| {
                Ok(auto_item.id.clone())
            })
            .is_ok()
        );
        let non_tty_owner = resolve_actor(Some("owner"), BoardCaller::CliNonTty).unwrap_err();
        assert_eq!(non_tty_owner.exit_code(), crate::agent_api::exit::FORBIDDEN);

        // 4. MCP board_decide is unknown; board_pending is available and read-only.
        unsafe { std::env::set_var("XDG_STATE_HOME", &state) };
        let mcp_decide = crate::board::mcp_invoke(
            &ws,
            "board_decide",
            &serde_json::json!({ "id": auto_item.id, "verdict": "approve" }),
        );
        assert_eq!(mcp_decide["ok"], false);
        assert!(
            mcp_decide["error"]
                .as_str()
                .unwrap_or("")
                .contains("unknown board tool")
        );

        // 5. board run on auto lane leaves decision == None and checked == None.
        let fake_bin = dir.path().join("fake-run.sh");
        std::fs::write(
            &fake_bin,
            "#!/bin/sh\ncat > /dev/null\nprintf '{\"ok\":true,\"exit_code\":0,\"run_id\":\"0123456789abcdef\",\"text\":\"done\"}\\n'\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_bin).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_bin, perms).unwrap();
        }
        unsafe { std::env::set_var("DMUXAI_BOARD_CHILD_BIN", &fake_bin) };
        let prep = crate::board::runner::run_item_prepare(
            &store,
            crate::board::runner::RunParams {
                id: auto_item.id.clone(),
                expected_plan_hash: Some(hash.clone()),
                by: "bot:coord".to_owned(),
                state_root: state.clone(),
            },
        )
        .unwrap();
        let finished = crate::board::runner::run_item_execute(&store, prep).unwrap();
        unsafe {
            std::env::remove_var("DMUXAI_BOARD_CHILD_BIN");
            std::env::remove_var("XDG_STATE_HOME");
        }
        assert_eq!(finished.lane, Lane::Done);
        assert!(finished.decision.is_none());
        assert!(finished.checked.is_none());

        // 6. HTTP: decision/check/lane without human headers -> 403; with headers -> 200.
        let need_item = seed_item(&store, &state);
        let token = "boardtesttoken";
        let router = test_router(board, state, token);

        let no_hdr_decide = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/board/{need_item}/decision"))
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(r#"{"verdict":"approve"}"#))
            .unwrap();
        assert_eq!(
            router
                .clone()
                .oneshot(no_hdr_decide)
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );

        let no_hdr_check = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/board/{}/check", finished.id))
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from("{}"))
            .unwrap();
        assert_eq!(
            router.clone().oneshot(no_hdr_check).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );

        let with_hdr_decide = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/board/{need_item}/decision"))
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("Sec-Fetch-Site", "same-origin")
            .header("Sec-Fetch-Mode", "cors")
            .header("X-Dmuxai-Human", "1")
            .body(axum::body::Body::from(r#"{"verdict":"approve"}"#))
            .unwrap();
        assert_eq!(
            router
                .clone()
                .oneshot(with_hdr_decide)
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        let with_hdr_check = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/board/{}/check", finished.id))
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("Sec-Fetch-Site", "same-origin")
            .header("Sec-Fetch-Mode", "cors")
            .header("X-Dmuxai-Human", "1")
            .body(axum::body::Body::from(r#"{"text":"verified"}"#))
            .unwrap();
        assert_eq!(
            router
                .clone()
                .oneshot(with_hdr_check)
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        let no_hdr_rules = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/board/rules")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                r#"{"rules":[],"default_lane":"needs_decision"}"#,
            ))
            .unwrap();
        assert_eq!(
            router.clone().oneshot(no_hdr_rules).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );

        let with_hdr_rules = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/board/rules")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("Sec-Fetch-Site", "same-origin")
            .header("Sec-Fetch-Mode", "cors")
            .header("X-Dmuxai-Human", "1")
            .body(axum::body::Body::from(
                r#"{"rules":[],"default_lane":"needs_decision"}"#,
            ))
            .unwrap();
        assert_eq!(
            router
                .clone()
                .oneshot(with_hdr_rules)
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        let pending_req = axum::http::Request::builder()
            .uri("/api/board/pending")
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let pending_resp = router.oneshot(pending_req).await.unwrap();
        assert_eq!(pending_resp.status(), StatusCode::OK);
        let pending_body = body_string(pending_resp).await;
        assert!(pending_body.contains("\"count\":0"));
    }

    // TC-3232 / FR-3232, FR-3236: owner approve with matching plan hash in `--web`
    // starts a detached board run, moves the item to in_progress, returns auto_run.started = true
    // without blocking, and dispatches decision notify with bot/verdict/auto_run:true.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn tc_3232_owner_approve_with_matching_hash_starts_detached_run() {
        let _guard = crate::state_path::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let ws = dir.path().join("ws");
        let xdg_cfg = dir.path().join("xdg_cfg");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(xdg_cfg.join("dmuxai")).unwrap();

        let notify_out = dir.path().join("notify.jsonl");
        let notify_script = dir.path().join("notify.sh");
        std::fs::write(
            &notify_script,
            format!(
                "#!/bin/sh\nbody=$(cat)\nprintf '%s\\n' \"$body\" >> \"{}\"\n",
                notify_out.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&notify_script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&notify_script, perms).unwrap();
        }
        std::fs::write(
            xdg_cfg.join("dmuxai").join("dmuxai.toml"),
            format!(
                "[board]\nnotify_command = [\"{}\"]\n",
                notify_script.display()
            ),
        )
        .unwrap();

        let self_argv = dir.path().join("self-argv.txt");
        let fake_self = dir.path().join("fake-self.sh");
        std::fs::write(
            &fake_self,
            format!(
                "#!/bin/sh\nsleep 0.2\nprintf '%s\\n' \"$@\" > \"{}\"\nexit 0\n",
                self_argv.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_self).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_self, perms).unwrap();
        }

        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_self = std::env::var_os("DMUXAI_BOARD_SELF_BIN");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &xdg_cfg);
            std::env::set_var("DMUXAI_BOARD_SELF_BIN", &fake_self);
        }

        let store = BoardStore::open(board.clone()).unwrap();
        let plan_json = r#"{"tasks":[{"id":"task-1","title":"do it","done_check":"done","size":"small","depends_on":[]}],"steps":[{"id":"step-1","role":"implement","job":"edit","runner":"cursor","depends_on":[],"task":"task-1"}]}"#;
        let (_, hash) = edit_plan_from_json(&ws, &state, "ship it", plan_json, None).unwrap();
        let item = store
            .add(AddItemRequest {
                title: "Auto run on approve".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: Some("coordinator".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(ws),
                recommendation: None,
                options: vec![],
                plan_hash: Some(hash.clone()),
                key: None,
                by: "bot:coordinator".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();

        let token = "boardtesttoken";
        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        let services = DashboardServices {
            board_base: Some(board.clone()),
            web_attached: true,
            trust_env: Some(crate::runtime_env::RuntimeEnv {
                dmuxai_state_dir: Some(state),
                ..crate::runtime_env::RuntimeEnv::from_process()
            }),
            ..DashboardServices::default()
        };
        let router = build_router_with(
            hub,
            token.to_string(),
            Some(vec!["localhost:7420".to_string()]),
            services,
        );

        let started_at = std::time::Instant::now();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/board/{}/decision", item.id))
                    .header("Host", "localhost:7420")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .header("Sec-Fetch-Site", "same-origin")
                    .header("Sec-Fetch-Mode", "cors")
                    .header("X-Dmuxai-Human", "1")
                    .body(axum::body::Body::from(format!(
                        r#"{{"verdict":"approve","plan_hash":"{hash}"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            started_at.elapsed() < std::time::Duration::from_millis(500),
            "decision endpoint must not block on detached run"
        );
        let body: serde_json::Value =
            serde_json::from_str(&body_string(resp).await).expect("json response");
        assert_eq!(body["lane"], "in_progress");
        assert_eq!(body["auto_run"]["started"], true);
        assert!(body["auto_run"].get("reason").is_none());
        assert_eq!(store.get(&item.id).unwrap().lane, Lane::InProgress);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !self_argv.exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let argv_text = std::fs::read_to_string(&self_argv).expect("detached child should run");
        assert!(argv_text.contains("--exec-prepared"), "argv: {argv_text}");
        assert!(argv_text.contains(&hash), "argv: {argv_text}");
        assert!(argv_text.contains("owner"), "argv: {argv_text}");

        crate::board::notify::wait_for_pending(std::time::Duration::from_secs(5));
        let notify_raw = std::fs::read_to_string(&notify_out).expect("notify output");
        let decision_ev: serde_json::Value = notify_raw
            .lines()
            .filter_map(|l| serde_json::from_str(l.trim()).ok())
            .find(|v: &serde_json::Value| v["event"] == "decision" && v["item_id"] == item.id)
            .expect("decision notify event");
        assert_eq!(decision_ev["bot"], "coordinator");
        assert_eq!(decision_ev["verdict"], "approve");
        assert_eq!(decision_ev["auto_run"], true);

        unsafe {
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match prev_self {
                Some(v) => std::env::set_var("DMUXAI_BOARD_SELF_BIN", v),
                None => std::env::remove_var("DMUXAI_BOARD_SELF_BIN"),
            }
        }
    }

    // TC-3235 / FR-3235, NFR-3230: view-only `serve` adds `auto-run skipped: view-only dashboard`
    // and never starts a run; `[board] auto_run_on_approve = false` never auto-runs and adds no thread msg.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn tc_3235_view_only_and_opt_out_never_auto_run() {
        let _guard = crate::state_path::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let ws = dir.path().join("ws");
        let xdg_cfg = dir.path().join("xdg_cfg");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(xdg_cfg.join("dmuxai")).unwrap();

        let self_marker = dir.path().join("self-invoked.txt");
        let fake_self = dir.path().join("fake-self.sh");
        std::fs::write(
            &fake_self,
            format!("#!/bin/sh\ntouch \"{}\"\nexit 0\n", self_marker.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_self).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_self, perms).unwrap();
        }

        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_self = std::env::var_os("DMUXAI_BOARD_SELF_BIN");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &xdg_cfg);
            std::env::set_var("DMUXAI_BOARD_SELF_BIN", &fake_self);
        }

        let store = BoardStore::open(board.clone()).unwrap();
        let plan_json = r#"{"tasks":[{"id":"task-1","title":"do it","done_check":"done","size":"small","depends_on":[]}],"steps":[{"id":"step-1","role":"implement","job":"edit","runner":"cursor","depends_on":[],"task":"task-1"}]}"#;
        let (_, hash) = edit_plan_from_json(&ws, &state, "ship it", plan_json, None).unwrap();

        // 1. View-only `serve` (launch_allowed = false) skips auto-run and adds `run` message.
        let view_item = store
            .add(AddItemRequest {
                title: "view-only approve".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: Some("coord".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(ws.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: Some(hash.clone()),
                key: None,
                by: "bot:coord".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let token = "boardtesttoken";
        let view_router = test_router(board.clone(), state.clone(), token);
        let resp = view_router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/board/{}/decision", view_item.id))
                    .header("Host", "localhost:7420")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .header("Sec-Fetch-Site", "same-origin")
                    .header("Sec-Fetch-Mode", "cors")
                    .header("X-Dmuxai-Human", "1")
                    .body(axum::body::Body::from(format!(
                        r#"{{"verdict":"approve","plan_hash":"{hash}"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(body["lane"], "approved");
        assert_eq!(body["auto_run"]["started"], false);
        assert_eq!(body["auto_run"]["reason"], "view-only dashboard");
        let saved_view = store.get(&view_item.id).unwrap();
        assert_eq!(saved_view.lane, Lane::Approved);
        assert_eq!(saved_view.thread.len(), 1);
        assert_eq!(saved_view.thread[0].from, "run");
        assert_eq!(
            saved_view.thread[0].text,
            "auto-run skipped: view-only dashboard"
        );
        assert!(!self_marker.exists());

        // 2. `[board] auto_run_on_approve = false` in user config (even on `--web`) never auto-runs
        //    and starts no thread / adds no thread message (NFR-3230).
        std::fs::write(
            xdg_cfg.join("dmuxai").join("dmuxai.toml"),
            "[board]\nauto_run_on_approve = false\n",
        )
        .unwrap();
        let opt_out_item = store
            .add(AddItemRequest {
                title: "opt-out approve".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: Some(Lane::NeedsDecision),
                source_bot: Some("coord".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(ws),
                recommendation: None,
                options: vec![],
                plan_hash: Some(hash.clone()),
                key: None,
                by: "bot:coord".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let hub = DashboardHub::new(SnapshotSource::Tui, "/workspace".to_string());
        let services = DashboardServices {
            board_base: Some(board),
            web_attached: true,
            trust_env: Some(crate::runtime_env::RuntimeEnv {
                dmuxai_state_dir: Some(state),
                ..crate::runtime_env::RuntimeEnv::from_process()
            }),
            ..DashboardServices::default()
        };
        let web_router = build_router_with(
            hub,
            token.to_string(),
            Some(vec!["localhost:7420".to_string()]),
            services,
        );
        let resp = web_router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/board/{}/decision", opt_out_item.id))
                    .header("Host", "localhost:7420")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .header("Sec-Fetch-Site", "same-origin")
                    .header("Sec-Fetch-Mode", "cors")
                    .header("X-Dmuxai-Human", "1")
                    .body(axum::body::Body::from(format!(
                        r#"{{"verdict":"approve","plan_hash":"{hash}"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let opt_body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(opt_body["lane"], "approved");
        assert_eq!(opt_body["auto_run"]["started"], false);
        let saved_opt = store.get(&opt_out_item.id).unwrap();
        assert_eq!(saved_opt.lane, Lane::Approved);
        assert!(saved_opt.thread.is_empty());
        assert!(!self_marker.exists());

        unsafe {
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match prev_self {
                Some(v) => std::env::set_var("DMUXAI_BOARD_SELF_BIN", v),
                None => std::env::remove_var("DMUXAI_BOARD_SELF_BIN"),
            }
        }
    }

    /// A board item whose run waits on a pending spec review record.
    fn seed_spec_review(store: &BoardStore, state: &Path, review_id: &str) -> String {
        use crate::graph::spec_review::{ReviewStatus, SpecReviewRecord};
        let id = seed_item(store, state);
        let record = SpecReviewRecord {
            review_id: review_id.to_owned(),
            graph_id: review_id.to_owned(),
            run_id: "run-1".to_owned(),
            workspace: "/workspace".to_owned(),
            prompt_summary: "ship".to_owned(),
            spec_paths: vec!["docs/specs/SPEC-x.md".to_owned()],
            spec_digest: "digest-1".to_owned(),
            round: 1,
            max_rounds: 5,
            status: ReviewStatus::Pending,
            created_ms: 1,
            updated_ms: 1,
            board_item_id: Some(id.clone()),
            verdict: None,
            by: None,
            feedback: None,
            decided_ms: None,
        };
        SpecReviewStore::new(state).create(&record).unwrap();
        store
            .set_spec_review(
                &id,
                crate::board::BoardSpecReview {
                    review_id: review_id.to_owned(),
                    status: ReviewStatus::Pending,
                    round: 1,
                    spec_paths: record.spec_paths.clone(),
                    spec_digest: record.spec_digest.clone(),
                },
                None,
            )
            .unwrap();
        id
    }

    fn spec_review_post(
        id: &str,
        token: &str,
        body: &str,
        human: bool,
    ) -> axum::http::Request<axum::body::Body> {
        let mut builder = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/board/{id}/spec-review"))
            .header("Host", "localhost:7420")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json");
        if human {
            builder = builder
                .header("Sec-Fetch-Site", "same-origin")
                .header("Sec-Fetch-Mode", "cors")
                .header("X-Dmuxai-Human", "1");
        }
        builder
            .body(axum::body::Body::from(body.to_owned()))
            .unwrap()
    }

    // TC-3708 / FR-3708 (http): a spec review POST without the human browser
    // headers is 403 and leaves the record and the item unchanged.
    #[tokio::test]
    async fn tc_3708_spec_review_post_requires_human_headers() {
        use crate::graph::spec_review::ReviewStatus;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let store = BoardStore::open(board.clone()).unwrap();
        let id = seed_spec_review(&store, &state, "graph-708");
        let token = "boardtesttoken";
        let router = test_router(board, state.clone(), token);
        let before = store.get(&id).unwrap();
        let resp = router
            .oneshot(spec_review_post(
                &id,
                token,
                r#"{"verdict":"approve","spec_digest":"digest-1"}"#,
                false,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let record = SpecReviewStore::new(&state).load("graph-708").unwrap();
        assert_eq!(record.status, ReviewStatus::Pending);
        assert!(record.by.is_none());
        assert_eq!(store.get(&id).unwrap().version, before.version);
    }

    // TC-3713 / FR-3713: verdicts, validation, digest and pending checks,
    // GET item and GET pending include `spec_review`.
    #[tokio::test]
    async fn tc_3713_spec_review_endpoint_codes_and_payloads() {
        use crate::graph::spec_review::ReviewStatus;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let store = BoardStore::open(board.clone()).unwrap();
        let token = "boardtesttoken";
        let router = test_router(board, state.clone(), token);
        let reviews = SpecReviewStore::new(&state);
        let approve_id = seed_spec_review(&store, &state, "graph-a");
        let changes_id = seed_spec_review(&store, &state, "graph-c");
        let reject_id = seed_spec_review(&store, &state, "graph-r");
        let plain_id = seed_item(&store, &state);

        let get = |uri: String| {
            axum::http::Request::builder()
                .uri(uri)
                .header("Host", "localhost:7420")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let resp = router
            .clone()
            .oneshot(get(format!("/api/board/{approve_id}")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let item: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(item["spec_review"]["status"], "pending");
        assert_eq!(item["spec_review"]["round"], 1);
        assert_eq!(item["spec_review"]["spec_digest"], "digest-1");
        assert_eq!(item["spec_review"]["spec_paths"][0], "docs/specs/SPEC-x.md");

        let resp = router
            .clone()
            .oneshot(get("/api/board/pending".to_owned()))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let pending: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        let entry = pending["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == approve_id.as_str())
            .expect("pending list includes the spec review item");
        assert_eq!(entry["reason"], "spec_review");
        assert_eq!(entry["spec_review"]["status"], "pending");

        let post = |id: &str, body: &str| spec_review_post(id, token, body, true);
        let status_of = |resp: &axum::response::Response| resp.status();

        // 400: invalid id, invalid verdict, empty feedback for request_changes.
        let resp = router
            .clone()
            .oneshot(post("not-an-id", r#"{"verdict":"approve"}"#))
            .await
            .unwrap();
        assert_eq!(status_of(&resp), StatusCode::BAD_REQUEST);
        let resp = router
            .clone()
            .oneshot(post(&approve_id, r#"{"verdict":"maybe"}"#))
            .await
            .unwrap();
        assert_eq!(status_of(&resp), StatusCode::BAD_REQUEST);
        let resp = router
            .clone()
            .oneshot(post(
                &changes_id,
                r#"{"verdict":"request_changes","text":"   "}"#,
            ))
            .await
            .unwrap();
        assert_eq!(status_of(&resp), StatusCode::BAD_REQUEST);

        // 404: unknown item, item without a review.
        let resp = router
            .clone()
            .oneshot(post("b000000000001", r#"{"verdict":"approve"}"#))
            .await
            .unwrap();
        assert_eq!(status_of(&resp), StatusCode::NOT_FOUND);
        let resp = router
            .clone()
            .oneshot(post(&plain_id, r#"{"verdict":"approve"}"#))
            .await
            .unwrap();
        assert_eq!(status_of(&resp), StatusCode::NOT_FOUND);

        // 409: stale digest.
        let resp = router
            .clone()
            .oneshot(post(
                &approve_id,
                r#"{"verdict":"approve","spec_digest":"old"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(status_of(&resp), StatusCode::CONFLICT);
        assert!(
            body_string(resp)
                .await
                .contains("spec changed since it was shown; reload")
        );
        assert_eq!(
            reviews.load("graph-a").unwrap().status,
            ReviewStatus::Pending
        );

        // 200: each verdict returns the updated item.
        let resp = router
            .clone()
            .oneshot(post(
                &approve_id,
                r#"{"verdict":"approve","spec_digest":"digest-1"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(status_of(&resp), StatusCode::OK);
        let item: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(item["id"], approve_id.as_str());
        assert_eq!(item["spec_review"]["status"], "approved");
        assert_eq!(item["lane"], "needs_decision");
        let record = reviews.load("graph-a").unwrap();
        assert_eq!(record.status, ReviewStatus::Approved);
        assert_eq!(record.by.as_deref(), Some("owner"));

        let resp = router
            .clone()
            .oneshot(post(
                &changes_id,
                r#"{"verdict":"request_changes","text":"add errors","spec_digest":"digest-1"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(status_of(&resp), StatusCode::OK);
        let item: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(item["spec_review"]["status"], "changes_requested");
        assert_eq!(
            reviews.load("graph-c").unwrap().feedback.as_deref(),
            Some("add errors")
        );

        let resp = router
            .clone()
            .oneshot(post(
                &reject_id,
                r#"{"verdict":"reject","spec_digest":"digest-1"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(status_of(&resp), StatusCode::OK);
        let item: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(item["spec_review"]["status"], "rejected");

        // 409: a decided review is not pending.
        let resp = router
            .clone()
            .oneshot(post(
                &approve_id,
                r#"{"verdict":"reject","spec_digest":"digest-1"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(status_of(&resp), StatusCode::CONFLICT);
        assert!(
            body_string(resp)
                .await
                .contains("spec review is not pending")
        );

        // Decided reviews leave the pending list (the plan decision remains).
        let resp = router
            .clone()
            .oneshot(get("/api/board/pending".to_owned()))
            .await
            .unwrap();
        let pending: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        let entry = pending["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == approve_id.as_str())
            .unwrap();
        assert_eq!(entry["reason"], "decide");
    }

    // TC-3715 / FR-3715, FR-3503 (#374): dashboard Save plan stores the
    // spec-first expanded plan and marks the inserted steps.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn tc_3715_save_plan_stores_spec_first_expanded_plan() {
        let _guard = crate::state_path::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let board = state.join("board");
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let xdg = dir.path().join("xdg");
        std::fs::create_dir_all(xdg.join("dmuxai")).unwrap();
        std::fs::write(
            xdg.join("dmuxai").join("dmuxai.toml"),
            "[workflow]\nspec_first = \"full\"\n",
        )
        .unwrap();
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &xdg) };

        let store = BoardStore::open(board.clone()).unwrap();
        let item = store
            .add(AddItemRequest {
                title: "save plan".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: None,
                source_bot: Some("coord".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(workspace.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:coord".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let research = r#"{"tasks":[{"id":"task-1","title":"do it","done_check":"done","size":"small","depends_on":[]}],"steps":[{"id":"step-1","role":"research","job":"look","runner":"cursor","depends_on":[],"task":"task-1"}]}"#;
        let (_, hash) = edit_plan_from_json(&workspace, &state, "ship it", research, None).unwrap();
        store
            .attach_plan(&item.id, &hash, &state, "bot:coord")
            .unwrap();

        let token = "boardtesttoken";
        let router = test_router(board, state.clone(), token);
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/board/{}/plan", item.id))
                    .header("Host", "localhost:7420")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"steps":[{"id":"step-1","role":"implement","job":"edit","runner":"cursor","model":null,"depends_on":[],"task":"task-1"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let body = body_string(resp).await;
        unsafe {
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
        assert_eq!(status, StatusCode::OK, "{body}");
        let item: serde_json::Value = serde_json::from_str(&body).unwrap();
        let ids: Vec<&str> = item["plan"]["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["spec-1", "step-1", "test-1"]);
        assert_eq!(
            item["plan"]["spec_first_added"],
            serde_json::json!(["spec-1", "test-1"])
        );
        let stored_hash = item["plan"]["hash"].as_str().unwrap();
        let pending = crate::board::read_pending_plan(&state, stored_hash).unwrap();
        assert_eq!(pending.steps.len(), 3);
    }
}
