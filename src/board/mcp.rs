//! MCP tool handlers for the decision board (FR-3113).

use std::path::PathBuf;

use serde_json::{Value, json};

use super::edit_plan_from_json;
use super::{
    AddItemRequest, BoardCaller, DecideRequest, Grade, Kind, Lane, ListFilter, SayRequest,
    UpdateItemRequest, Verdict, WaitFilter, WaitFor, WaitOutcome, resolve_actor, wait_ready_json,
};
use crate::agent_api::{ApiError, exit, open_board_store};

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
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
        "board_decide" => match board_decide(args) {
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
        other => board_err_json(ApiError::new(
            exit::USAGE,
            format!("unknown board tool: {other}"),
        )),
    }
}

fn board_add(workspace_root: &std::path::Path, args: &Value) -> Result<Value, ApiError> {
    let title = str_arg(args, "title")
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ApiError::new(exit::USAGE, "title is required".to_owned()))?;
    let kind = str_arg(args, "kind")
        .and_then(Kind::parse)
        .ok_or_else(|| ApiError::new(exit::USAGE, "kind is required".to_owned()))?;
    let grade = str_arg(args, "grade").and_then(Grade::parse);
    let lane = str_arg(args, "lane").and_then(Lane::parse);
    let options: Vec<String> = args
        .get("options")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
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
            summary: str_arg(args, "summary").unwrap_or("").to_owned(),
            kind,
            grade,
            lane,
            source_bot: str_arg(args, "bot").map(str::to_owned),
            source_channel: str_arg(args, "channel").map(str::to_owned),
            source_ref: str_arg(args, "ref").map(str::to_owned),
            workspace: Some(workspace),
            recommendation: str_arg(args, "recommendation").map(str::to_owned),
            options,
            plan_hash: str_arg(args, "plan_hash").map(str::to_owned),
            by,
            state_root,
        })
        .map_err(ApiError::from)?;
    Ok(serde_json::to_value(item).unwrap_or(Value::Null))
}

fn board_list(args: &Value) -> Result<Value, ApiError> {
    let store = open_board_store()?;
    let lanes: Vec<Lane> = args
        .get("lane")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter_map(Lane::parse)
                .collect()
        })
        .unwrap_or_default();
    let workspace = str_arg(args, "workspace").map(PathBuf::from);
    let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
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
    let id = str_arg(args, "id")
        .ok_or_else(|| ApiError::new(exit::USAGE, "id is required".to_owned()))?;
    let store = open_board_store()?;
    let item = store.get(id).map_err(ApiError::from)?;
    Ok(serde_json::to_value(item).unwrap_or(Value::Null))
}

fn board_update(_workspace_root: &std::path::Path, args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id")
        .ok_or_else(|| ApiError::new(exit::USAGE, "id is required".to_owned()))?;
    let by = resolve_actor(str_arg(args, "by"), BoardCaller::Mcp).map_err(ApiError::from)?;
    let store = open_board_store()?;
    let item = store
        .update(
            id,
            UpdateItemRequest {
                title: str_arg(args, "title").map(str::to_owned),
                summary: str_arg(args, "summary").map(str::to_owned),
                recommendation: str_arg(args, "recommendation").map(str::to_owned),
                options: args.get("options").and_then(|v| {
                    v.as_array().map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                }),
                grade: None,
                kind: str_arg(args, "kind").and_then(Kind::parse),
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
    let id = str_arg(args, "id")
        .ok_or_else(|| ApiError::new(exit::USAGE, "id is required".to_owned()))?;
    let lane = str_arg(args, "lane")
        .and_then(Lane::parse)
        .ok_or_else(|| ApiError::new(exit::USAGE, "lane is required".to_owned()))?;
    let by = resolve_actor(str_arg(args, "by"), BoardCaller::Mcp).map_err(ApiError::from)?;
    let store = open_board_store()?;
    let item = store.move_lane(id, lane, &by).map_err(ApiError::from)?;
    Ok(serde_json::to_value(item).unwrap_or(Value::Null))
}

fn board_plan(workspace_root: &std::path::Path, args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id")
        .ok_or_else(|| ApiError::new(exit::USAGE, "id is required".to_owned()))?;
    let store = open_board_store()?;
    let state_root = crate::state_path::state_base()
        .map_err(|e| ApiError::new(exit::INTERNAL, e.to_string()))?;
    let item = if let Some(hash) = str_arg(args, "hash") {
        store
            .attach_plan(id, hash, &state_root)
            .map_err(ApiError::from)?
    } else if let Some(steps) = args.get("steps") {
        let prompt = store
            .get(id)
            .map_err(ApiError::from)?
            .plan
            .map(|p| p.prompt)
            .unwrap_or_else(|| str_arg(args, "prompt").unwrap_or_default().to_owned());
        let mut doc = json!({ "steps": steps });
        if let Some(tasks) = args.get("tasks") {
            doc["tasks"] = tasks.clone();
        }
        let (_pending, hash) = edit_plan_from_json(
            workspace_root,
            &state_root,
            &prompt,
            &serde_json::to_string(&doc)
                .map_err(|e| ApiError::new(exit::INTERNAL, e.to_string()))?,
        )
        .map_err(ApiError::from)?;
        store
            .edit_plan(id, &hash, workspace_root, &state_root)
            .map_err(ApiError::from)?
    } else {
        return Err(ApiError::new(
            exit::USAGE,
            "board_plan needs hash or steps".to_owned(),
        ));
    };
    Ok(serde_json::to_value(item).unwrap_or(Value::Null))
}

fn board_decide(args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id")
        .ok_or_else(|| ApiError::new(exit::USAGE, "id is required".to_owned()))?;
    let verdict = str_arg(args, "verdict")
        .and_then(Verdict::parse)
        .ok_or_else(|| ApiError::new(exit::USAGE, "verdict is required".to_owned()))?;
    let by = resolve_actor(str_arg(args, "by"), BoardCaller::Mcp).map_err(ApiError::from)?;
    let store = open_board_store()?;
    let item = store
        .decide(
            id,
            DecideRequest {
                verdict,
                text: str_arg(args, "text").map(str::to_owned),
                option: args
                    .get("option")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize),
                plan_hash: str_arg(args, "plan_hash").map(str::to_owned),
                by,
            },
        )
        .map_err(ApiError::from)?;
    Ok(serde_json::to_value(item).unwrap_or(Value::Null))
}

fn board_say(args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id")
        .ok_or_else(|| ApiError::new(exit::USAGE, "id is required".to_owned()))?;
    let text = str_arg(args, "text")
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ApiError::new(exit::USAGE, "text is required".to_owned()))?;
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

fn board_wait(args: &Value) -> Result<Value, ApiError> {
    let id = str_arg(args, "id")
        .ok_or_else(|| ApiError::new(exit::USAGE, "id is required".to_owned()))?;
    let store = open_board_store()?;
    let since = args
        .get("since_version")
        .and_then(Value::as_u64)
        .map(|n| n as u32)
        .unwrap_or_else(|| store.get(id).map(|i| i.version).unwrap_or(0));
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(Value::as_u64)
        .unwrap_or(300)
        .min(300);
    let wait_for = match str_arg(args, "wait_for").unwrap_or("any") {
        "decision" => WaitFor::Decision,
        "message" => WaitFor::Message,
        "any" => WaitFor::Any,
        other => {
            return Err(ApiError::new(
                exit::USAGE,
                format!("invalid wait_for '{other}'"),
            ));
        }
    };
    let waiter = str_arg(args, "waiter")
        .map(str::to_owned)
        .unwrap_or_else(|| "bot:mcp".to_owned());
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
