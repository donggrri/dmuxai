//! MCP tool handlers for the decision board (FR-3113).

use std::path::PathBuf;

use serde_json::{Value, json};

use super::edit_plan_from_json;
use super::{
    AddItemRequest, BoardCaller, Grade, Kind, Lane, ListFilter, SayRequest, TEXT_MAX,
    UpdateItemRequest, WaitFilter, WaitFor, WaitOutcome, resolve_actor, validate_item_id_early,
    validate_message, validate_new_text, validate_options, validate_ref_url, validate_text_limit,
    wait_ready_json, wait_ready_json_any,
};
use crate::agent_api::{ApiError, exit, open_board_store};

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn usage(message: impl Into<String>) -> ApiError {
    ApiError::new(exit::USAGE, message.into())
}

fn parse_optional_grade(args: &Value) -> Result<Option<Grade>, ApiError> {
    match str_arg(args, "grade") {
        None | Some("") => Ok(None),
        Some(g) => Grade::parse(g)
            .map(Some)
            .ok_or_else(|| usage(format!("invalid grade '{g}'"))),
    }
}

fn parse_optional_kind(args: &Value) -> Result<Option<Kind>, ApiError> {
    match str_arg(args, "kind") {
        None => Ok(None),
        Some(k) => Kind::parse(k)
            .map(Some)
            .ok_or_else(|| usage(format!("invalid kind '{k}'"))),
    }
}

fn parse_optional_lane(args: &Value) -> Result<Option<Lane>, ApiError> {
    match str_arg(args, "lane") {
        None => Ok(None),
        Some(l) => Lane::parse(l)
            .map(Some)
            .ok_or_else(|| usage(format!("invalid lane '{l}'"))),
    }
}

fn parse_string_array(args: &Value, key: &str) -> Result<Option<Vec<String>>, ApiError> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    let items = value
        .as_array()
        .ok_or_else(|| usage(format!("{key} must be an array of strings")))?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let text = item
            .as_str()
            .ok_or_else(|| usage(format!("{key} must be an array of strings")))?;
        out.push(text.to_owned());
    }
    Ok(Some(out))
}

fn validate_plan_hash_arg(hash: &str) -> Result<(), ApiError> {
    if crate::state_path::valid_pending_plan_hash(hash) {
        Ok(())
    } else {
        Err(usage(
            "plan hash must be 16 lowercase hex characters".to_owned(),
        ))
    }
}

fn board_err_json(error: ApiError) -> Value {
    let mut value = error.to_json();
    value["ok"] = Value::Bool(false);
    value
}

fn board_ok_json(value: Value) -> Value {
    let mut out = json!({ "ok": true, "exit_code": exit::OK });
    if value.is_object() {
        if let Some(map) = value.as_object() {
            for (key, val) in map {
                out[key] = val.clone();
            }
        }
    } else {
        out["result"] = value;
    }
    out
}

pub fn invoke(workspace_root: &std::path::Path, name: &str, args: &Value) -> Value {
    match name {
        "board_add" => match board_add(workspace_root, args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        "board_list" => match board_list(args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        "board_show" => match board_show(args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        "board_update" => match board_update(workspace_root, args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        "board_move" => match board_move(args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        "board_plan" => match board_plan(workspace_root, args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        "board_pending" => match board_pending(args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        "board_say" => match board_say(args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        "board_wait" => match board_wait(args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        "board_prompt" => match board_prompt(args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        "board_events" => match board_events(args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        "board_ask" => match board_ask(args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        "board_run" => match board_run(args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        "board_session" => match board_session(args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        "board_link" => match board_link(args) {
            Ok(v) => board_ok_json(v),
            Err(e) => board_err_json(e),
        },
        other => board_err_json(ApiError::new(
            exit::USAGE,
            format!("unknown board tool: {other}"),
        )),
    }
}

fn board_add(workspace_root: &std::path::Path, args: &Value) -> Result<Value, ApiError> {
    let title = str_arg(args, "title")
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| usage("title is required"))?;
    let kind = match str_arg(args, "kind") {
        Some(k) => Kind::parse(k).ok_or_else(|| usage(format!("invalid kind '{k}'")))?,
        None => return Err(usage("kind is required")),
    };
    let grade = parse_optional_grade(args)?;
    let lane = parse_optional_lane(args)?;
    let options = parse_string_array(args, "options")?.unwrap_or_default();
    let summary = str_arg(args, "summary").unwrap_or("");
    let recommendation = str_arg(args, "recommendation");
    let ref_url = str_arg(args, "ref");
    let plan_hash = str_arg(args, "plan_hash");
    validate_new_text(title, summary, recommendation).map_err(ApiError::from)?;
    validate_options(&options).map_err(ApiError::from)?;
    validate_ref_url(ref_url).map_err(ApiError::from)?;
    if let Some(hash) = plan_hash {
        validate_plan_hash_arg(hash)?;
    }
    let workspace = str_arg(args, "workspace")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.to_path_buf());
    let state_root = crate::state_path::state_base()
        .map_err(|e| ApiError::new(exit::INTERNAL, e.to_string()))?;
    let by = resolve_actor(str_arg(args, "by"), BoardCaller::Mcp).map_err(ApiError::from)?;
    let store = open_board_store()?;
    let item = store
        .add(AddItemRequest {
            title: title.to_owned(),
            summary: summary.to_owned(),
            kind,
            grade,
            lane,
            source_bot: str_arg(args, "bot").map(str::to_owned),
            source_channel: str_arg(args, "channel").map(str::to_owned),
            source_ref: ref_url.map(str::to_owned),
            workspace: Some(workspace),
            recommendation: recommendation.map(str::to_owned),
            options,
            plan_hash: plan_hash.map(str::to_owned),
            key: str_arg(args, "key").map(str::to_owned),
            by,
            state_root,
        })
        .map_err(ApiError::from)?;
    Ok(serde_json::to_value(item).unwrap_or(Value::Null))
}

fn board_list(args: &Value) -> Result<Value, ApiError> {
    let lanes: Vec<Lane> = match args.get("lane") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                let lane = v.as_str().ok_or_else(|| usage("lane must be a string"))?;
                Lane::parse(lane).ok_or_else(|| usage(format!("invalid lane '{lane}'")))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(usage("lane must be an array of strings")),
        None => Vec::new(),
    };
    let workspace = str_arg(args, "workspace").map(PathBuf::from);
    let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
    let store = open_board_store()?;
    let items = store
        .list(&ListFilter {
            lanes,
            workspace,
            include_all: all,
        })
        .map_err(ApiError::from)?;
    Ok(serde_json::to_value(items).unwrap_or(Value::Null))
}

fn board_show(args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id").ok_or_else(|| usage("id is required"))?;
    validate_item_id_early(id).map_err(ApiError::from)?;
    let store = open_board_store()?;
    let item = store.get(id).map_err(ApiError::from)?;
    Ok(serde_json::to_value(item).unwrap_or(Value::Null))
}

/// Read-only work session of an item (SPEC-board-work-session FR-3823).
fn board_session(args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id").ok_or_else(|| usage("id is required"))?;
    validate_item_id_early(id).map_err(ApiError::from)?;
    let state_root = crate::state_path::state_base()
        .map_err(|e| ApiError::new(exit::INTERNAL, e.to_string()))?;
    let store = open_board_store()?;
    let view = super::session::session_view(&store, id, &state_root).map_err(ApiError::from)?;
    Ok(serde_json::to_value(view).unwrap_or(Value::Null))
}

/// Link an existing run to an item (SPEC-board-work-session FR-3826).
fn board_link(args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id").ok_or_else(|| usage("id is required"))?;
    validate_item_id_early(id).map_err(ApiError::from)?;
    let run_id = str_arg(args, "run_id").ok_or_else(|| usage("run_id is required"))?;
    if !crate::run_journal::valid_run_id(run_id) {
        return Err(usage("run_id must be 16 lowercase hex characters"));
    }
    let by = resolve_actor(str_arg(args, "by"), BoardCaller::Mcp).map_err(ApiError::from)?;
    let store = open_board_store()?;
    let item = super::session::link_run(&store, id, run_id, &by).map_err(ApiError::from)?;
    Ok(serde_json::to_value(item).unwrap_or(Value::Null))
}

fn board_update(_workspace_root: &std::path::Path, args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id").ok_or_else(|| usage("id is required"))?;
    validate_item_id_early(id).map_err(ApiError::from)?;
    let title = str_arg(args, "title");
    let summary = str_arg(args, "summary");
    let recommendation = str_arg(args, "recommendation");
    let options = parse_string_array(args, "options")?;
    let grade = match str_arg(args, "grade") {
        None => None,
        Some(_) => Some(parse_optional_grade(args)?),
    };
    let kind = parse_optional_kind(args)?;
    if let Some(title) = title {
        validate_new_text(title, "", None).map_err(ApiError::from)?;
    }
    if let Some(summary) = summary {
        validate_text_limit("summary", summary, TEXT_MAX).map_err(ApiError::from)?;
    }
    if let Some(recommendation) = recommendation {
        validate_text_limit("recommendation", recommendation, TEXT_MAX).map_err(ApiError::from)?;
    }
    if let Some(options) = &options {
        validate_options(options).map_err(ApiError::from)?;
    }
    let by = resolve_actor(str_arg(args, "by"), BoardCaller::Mcp).map_err(ApiError::from)?;
    let store = open_board_store()?;
    let item = store
        .update(
            id,
            UpdateItemRequest {
                title: title.map(str::to_owned),
                summary: summary.map(str::to_owned),
                recommendation: recommendation.map(str::to_owned),
                options,
                grade,
                kind,
                run_id: str_arg(args, "run_id").map(str::to_owned),
                if_version: args
                    .get("if_version")
                    .and_then(Value::as_u64)
                    .map(|n| n as u32),
                by,
            },
        )
        .map_err(ApiError::from)?;
    Ok(serde_json::to_value(item).unwrap_or(Value::Null))
}

fn board_move(args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id").ok_or_else(|| usage("id is required"))?;
    validate_item_id_early(id).map_err(ApiError::from)?;
    let lane = match str_arg(args, "lane") {
        Some(l) => Lane::parse(l).ok_or_else(|| usage(format!("invalid lane '{l}'")))?,
        None => return Err(usage("lane is required")),
    };
    let by = resolve_actor(str_arg(args, "by"), BoardCaller::Mcp).map_err(ApiError::from)?;
    let store = open_board_store()?;
    let if_version = args
        .get("if_version")
        .and_then(Value::as_u64)
        .map(|n| n as u32);
    let item = store
        .move_lane(id, lane, &by, if_version)
        .map_err(ApiError::from)?;
    Ok(serde_json::to_value(item).unwrap_or(Value::Null))
}

fn board_plan(workspace_root: &std::path::Path, args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id").ok_or_else(|| usage("id is required"))?;
    validate_item_id_early(id).map_err(ApiError::from)?;
    let hash_arg = str_arg(args, "hash");
    if let Some(hash) = hash_arg {
        validate_plan_hash_arg(hash)?;
    } else if args.get("steps").is_none() {
        return Err(usage("board_plan needs hash or steps"));
    }
    let state_root = crate::state_path::state_base()
        .map_err(|e| ApiError::new(exit::INTERNAL, e.to_string()))?;
    let store = open_board_store()?;
    let item = if let Some(hash) = hash_arg {
        let by = resolve_actor(str_arg(args, "by"), BoardCaller::Mcp).map_err(ApiError::from)?;
        store
            .attach_plan(id, hash, &state_root, &by)
            .map_err(ApiError::from)?
    } else if let Some(steps) = args.get("steps") {
        let item = store.get(id).map_err(ApiError::from)?;
        let ws = item
            .workspace
            .path
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace_root.to_path_buf());
        let prompt = if let Some(prompt) = str_arg(args, "prompt") {
            super::read_path_or_stdin(prompt).map_err(ApiError::from)?
        } else {
            super::plan::unmasked_prompt_from_item_plan(
                &state_root,
                store.base(),
                item.plan.as_ref(),
            )
            .map_err(ApiError::from)?
            .unwrap_or_default()
        };
        if prompt.trim().is_empty() {
            return Err(ApiError::new(
                exit::USAGE,
                "board_plan edit needs prompt when the item has no plan".to_owned(),
            ));
        }
        let inherit_tasks = item.plan.as_ref().map(|p| p.tasks.clone());
        let mut doc = json!({ "steps": steps });
        if let Some(tasks) = args.get("tasks") {
            doc["tasks"] = tasks.clone();
        }
        let by = resolve_actor(str_arg(args, "by"), BoardCaller::Mcp).map_err(ApiError::from)?;
        let (_pending, hash) = edit_plan_from_json(
            &ws,
            &state_root,
            &prompt,
            &serde_json::to_string(&doc)
                .map_err(|e| ApiError::new(exit::INTERNAL, e.to_string()))?,
            inherit_tasks,
        )
        .map_err(ApiError::from)?;
        store
            .edit_plan(id, &hash, &ws, &state_root, &by)
            .map_err(ApiError::from)?
    } else {
        return Err(ApiError::new(
            exit::USAGE,
            "board_plan needs hash or steps".to_owned(),
        ));
    };
    Ok(serde_json::to_value(item).unwrap_or(Value::Null))
}

fn board_pending(args: &Value) -> Result<Value, ApiError> {
    let workspace = str_arg(args, "workspace").map(PathBuf::from);
    let store = open_board_store()?;
    let pending = store
        .pending(workspace.as_deref())
        .map_err(ApiError::from)?;
    Ok(serde_json::to_value(pending).unwrap_or(Value::Null))
}

fn board_say(args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id").ok_or_else(|| usage("id is required"))?;
    validate_item_id_early(id).map_err(ApiError::from)?;
    let text = str_arg(args, "text")
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| usage("text is required"))?;
    validate_message(text).map_err(ApiError::from)?;
    let from = resolve_actor(str_arg(args, "from"), BoardCaller::Mcp).map_err(ApiError::from)?;
    let store = open_board_store()?;
    let item = store
        .say(
            id,
            SayRequest {
                text: text.to_owned(),
                from,
            },
        )
        .map_err(ApiError::from)?;
    Ok(serde_json::to_value(item).unwrap_or(Value::Null))
}

fn board_prompt(args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id").ok_or_else(|| usage("id is required"))?;
    validate_item_id_early(id).map_err(ApiError::from)?;
    let hash = str_arg(args, "hash");
    if let Some(hash) = hash {
        validate_plan_hash_arg(hash)?;
    }
    let store = open_board_store()?;
    let prompt = store.unmasked_prompt(id, hash).map_err(ApiError::from)?;
    Ok(json!({ "prompt": prompt }))
}

fn board_events(args: &Value) -> Result<Value, ApiError> {
    let since = args.get("since").and_then(Value::as_u64).unwrap_or(0);
    let store = open_board_store()?;
    let events = store.list_events(since).map_err(ApiError::from)?;
    Ok(serde_json::to_value(events).unwrap_or(Value::Null))
}

fn board_wait(args: &Value) -> Result<Value, ApiError> {
    let any = args.get("any").and_then(Value::as_bool).unwrap_or(false);
    let wait_for = match str_arg(args, "wait_for").unwrap_or("any") {
        "decision" => WaitFor::Decision,
        "message" => WaitFor::Message,
        "any" => WaitFor::Any,
        other => return Err(usage(format!("invalid wait_for '{other}'"))),
    };
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(Value::as_u64)
        .unwrap_or(300)
        .min(300);
    let waiter = str_arg(args, "waiter")
        .map(str::to_owned)
        .unwrap_or_else(|| "bot:mcp".to_owned());
    let since_version = args
        .get("since_version")
        .and_then(Value::as_u64)
        .map(|n| n as u32);
    let id = str_arg(args, "id");
    if !any {
        let id = id.ok_or_else(|| usage("id is required unless any is true"))?;
        validate_item_id_early(id).map_err(ApiError::from)?;
    }
    let store = open_board_store()?;
    if any {
        let since_seq = args.get("since_seq").and_then(Value::as_u64).unwrap_or(0);
        let outcome = store
            .wait_any(
                since_seq,
                WaitFilter {
                    since_version: since_version.unwrap_or(0),
                    timeout_secs,
                    wait_for,
                    waiter,
                },
            )
            .map_err(ApiError::from)?;
        return Ok(match outcome {
            Some((event, item)) => wait_ready_json_any(&event, &item),
            None => {
                json!({ "timed_out": true, "version": since_version.unwrap_or(0) })
            }
        });
    }
    let id = id.expect("validated above");
    let since = since_version.unwrap_or_else(|| store.get(id).map(|i| i.version).unwrap_or(0));
    let outcome = store
        .wait(
            id,
            WaitFilter {
                since_version: since,
                timeout_secs,
                wait_for,
                waiter,
            },
        )
        .map_err(ApiError::from)?;
    Ok(match outcome {
        WaitOutcome::Ready { item, event } => wait_ready_json(&event, &item),
        WaitOutcome::TimedOut { version } => json!({ "timed_out": true, "version": version }),
    })
}

fn board_ask(args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id").ok_or_else(|| usage("id is required"))?;
    validate_item_id_early(id).map_err(ApiError::from)?;
    let text = str_arg(args, "text")
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| usage("text is required"))?;
    validate_message(text).map_err(ApiError::from)?;
    let revise = args.get("revise").and_then(Value::as_bool).unwrap_or(false);
    let agent = str_arg(args, "agent").map(str::to_owned);
    let model = str_arg(args, "model").map(str::to_owned);
    let resume = match str_arg(args, "resume") {
        None => super::resume::ResumeMode::default(),
        Some(raw) => super::resume::ResumeMode::parse(raw)
            .ok_or_else(|| usage("resume must be auto, native, fresh or off"))?,
    };
    let by = resolve_actor(str_arg(args, "by"), BoardCaller::Mcp).map_err(ApiError::from)?;
    let state_root = crate::state_path::state_base()
        .map_err(|e| ApiError::new(exit::INTERNAL, e.to_string()))?;
    let store = open_board_store()?;
    let answer = super::runner::ask_item_report(
        &store,
        super::runner::AskParams {
            id: id.to_owned(),
            text: text.to_owned(),
            revise,
            agent,
            model,
            by,
            state_root,
            resume,
        },
    )
    .map_err(ApiError::from)?;
    Ok(super::resume::ask_answer_json(
        &answer.item,
        answer.resume.as_ref(),
    ))
}

fn board_run(args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id").ok_or_else(|| usage("id is required"))?;
    validate_item_id_early(id).map_err(ApiError::from)?;
    let by = resolve_actor(str_arg(args, "by"), BoardCaller::Mcp).map_err(ApiError::from)?;
    let state_root = crate::state_path::state_base()
        .map_err(|e| ApiError::new(exit::INTERNAL, e.to_string()))?;
    let store = open_board_store()?;
    let prepared = super::runner::run_item_prepare(
        &store,
        super::runner::RunParams {
            id: id.to_owned(),
            expected_plan_hash: None,
            by,
            state_root,
        },
    )
    .map_err(ApiError::from)?;
    let item =
        super::runner::launch_detached_board_run(&store, prepared).map_err(ApiError::from)?;
    Ok(serde_json::to_value(item).unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{SessionKind, SessionStatus, session};

    // TC-3824 / FR-3824, FR-3823, FR-3826: MCP `board_show` includes
    // `sessions`; `board_session` reports the work session read-only;
    // `board_link` appends a run link from a manifest and maps unknown runs
    // to exit 3 and invalid run ids to exit 2.
    #[test]
    fn tc_3824_show_includes_sessions() {
        let _env = crate::state_path::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let data = dir.path().join("data");
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        unsafe {
            std::env::set_var("DMUXAI_STATE_DIR", &state);
            std::env::set_var("DMUXAI_DATA_DIR", &data);
        }
        let added = invoke(
            &ws,
            "board_add",
            &json!({"title": "mcp sessions", "kind": "feature"}),
        );
        let id = added["id"].as_str().unwrap().to_owned();
        let ws_path = added["workspace"]["path"].as_str().unwrap().to_owned();
        let store = open_board_store().unwrap();
        let mut link = session::running_link(SessionKind::Run, std::path::Path::new(&ws_path), 5);
        link.status = SessionStatus::Completed;
        link.conversation_id = Some("conv-mcp".to_owned());
        store.append_session(&id, link, "bot:coord").unwrap();

        let shown = invoke(&ws, "board_show", &json!({"id": id}));
        assert_eq!(shown["ok"], true);
        assert_eq!(shown["sessions"][0]["conversation_id"], "conv-mcp");
        assert_eq!(shown["sessions"][0]["status"], "completed");

        let view = invoke(&ws, "board_session", &json!({"id": id}));
        assert_eq!(view["ok"], true, "{view}");
        assert_eq!(view["work_session"]["conversation_id"], "conv-mcp");
        assert_eq!(view["bindings"], json!([]));
        assert!(view["warning"].is_string());
        session::test_support::seed_conversation_db(&state, &ws_path, "conv-mcp", 3);
        let view = invoke(&ws, "board_session", &json!({"id": id}));
        assert_eq!(
            view["bindings"][1],
            json!({"agent": "cursor", "status": "active"})
        );
        assert!(!view.to_string().contains("SECRET"));

        session::test_support::write_manifest(
            &data,
            &ws_path,
            "abcdef0123456789",
            "completed",
            Some("conv-linked"),
            None,
        );
        let linked = invoke(
            &ws,
            "board_link",
            &json!({"id": id, "run_id": "abcdef0123456789"}),
        );
        assert_eq!(linked["ok"], true, "{linked}");
        assert_eq!(linked["sessions"][1]["run_id"], "abcdef0123456789");
        let unknown = invoke(
            &ws,
            "board_link",
            &json!({"id": id, "run_id": "0000000000000000"}),
        );
        assert_eq!(unknown["exit_code"], exit::NOT_FOUND);
        let invalid = invoke(&ws, "board_link", &json!({"id": id, "run_id": "../x"}));
        assert_eq!(invalid["exit_code"], exit::USAGE);
        let missing = invoke(&ws, "board_session", &json!({"id": "b000000000001"}));
        assert_eq!(missing["exit_code"], exit::NOT_FOUND);
        unsafe {
            std::env::remove_var("DMUXAI_STATE_DIR");
            std::env::remove_var("DMUXAI_DATA_DIR");
        }
    }
}
