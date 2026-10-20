//! ACP `session/request_permission` policy (SPEC-acp-runners FR-1304).

use serde_json::Value;

/// Inputs for permission decisions (sensitive gates are enforced by the App before
/// tools run; this layer handles workspace boundaries on ACP permission prompts).
#[derive(Debug, Clone, Default)]
pub struct PermissionContext {
    pub workspace: String,
    /// When true, prefer deny/cancel for write/exec style options (graph gate).
    pub sensitive_gate_active: bool,
}

/// Pick an option id to approve, or `None` to cancel/deny.
pub fn choose_permission_option(params: &Value, ctx: &PermissionContext) -> Option<String> {
    let options = params.get("options").and_then(Value::as_array).or_else(|| {
        params
            .pointer("/toolCall/options")
            .and_then(Value::as_array)
    });
    let options = options?;

    if ctx.sensitive_gate_active {
        return None;
    }

    let workspace = ctx.workspace.as_str();
    if path_outside_workspace(params, workspace) {
        return deny_option(options);
    }

    allow_workspace_option(options)
}

fn path_outside_workspace(params: &Value, workspace: &str) -> bool {
    let paths = collect_paths(params);
    if paths.is_empty() {
        return false;
    }
    let workspace = workspace.trim_end_matches('/');
    paths.iter().any(|path| {
        let path = path.trim();
        if path.is_empty() {
            return false;
        }
        if path.starts_with("file://") {
            let path = path.strip_prefix("file://").unwrap_or(path);
            return !path.starts_with(workspace);
        }
        !path.starts_with(workspace) && !path.starts_with("./")
    })
}

fn collect_paths(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => {
            for item in items {
                out.extend(collect_paths(item));
            }
        }
        Value::Object(map) => {
            for (key, val) in map {
                if key.eq_ignore_ascii_case("path")
                    || key.eq_ignore_ascii_case("uri")
                    || key.ends_with("Path")
                {
                    if let Some(s) = val.as_str() {
                        out.push(s.to_owned());
                    }
                }
                out.extend(collect_paths(val));
            }
        }
        _ => {}
    }
    out
}

fn allow_workspace_option(options: &[Value]) -> Option<String> {
    for opt in options {
        let id = option_id(opt)?;
        let kind = opt
            .get("kind")
            .or_else(|| opt.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        if kind.contains("deny") || kind.contains("cancel") || kind.contains("reject") {
            continue;
        }
        return Some(id);
    }
    options.first().and_then(option_id)
}

fn deny_option(options: &[Value]) -> Option<String> {
    for opt in options {
        let kind = opt
            .get("kind")
            .or_else(|| opt.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        if kind.contains("deny") || kind.contains("cancel") || kind.contains("reject") {
            return option_id(opt);
        }
    }
    None
}

fn option_id(opt: &Value) -> Option<String> {
    opt.get("optionId")
        .or_else(|| opt.get("option_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // TC-1306
    #[test]
    fn denies_permission_when_path_outside_workspace() {
        let params = json!({
            "options": [
                {"optionId": "allow", "kind": "allow"},
                {"optionId": "deny", "kind": "deny"}
            ],
            "path": "/etc/passwd"
        });
        let ctx = PermissionContext {
            workspace: "/home/user/proj".to_owned(),
            sensitive_gate_active: false,
        };
        assert_eq!(
            choose_permission_option(&params, &ctx).as_deref(),
            Some("deny")
        );
    }

    // TC-1307
    #[test]
    fn sensitive_gate_cancels_permission() {
        let params = json!({
            "options": [{"optionId": "allow", "kind": "allow"}]
        });
        let ctx = PermissionContext {
            workspace: "/home/user/proj".to_owned(),
            sensitive_gate_active: true,
        };
        assert!(choose_permission_option(&params, &ctx).is_none());
    }
}
