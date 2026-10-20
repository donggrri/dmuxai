//! ACP `session/request_permission` policy (SPEC-acp-runners FR-1304).

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

/// Inputs for permission decisions (sensitive gates are enforced by the App before
/// tools run; this layer handles workspace boundaries on ACP permission prompts).
#[derive(Debug, Clone, Default)]
pub struct PermissionContext {
    pub workspace: String,
    /// When true, prefer deny/cancel for write/exec style options (graph gate).
    pub sensitive_gate_active: bool,
    /// Read-only turn but `session/set_mode` failed: deny write/exec permissions (FR-1317).
    pub read_only_fail_closed: bool,
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
    if ctx.read_only_fail_closed {
        return deny_option(options).or_else(|| deny_write_options(options));
    }

    let workspace = ctx.workspace.as_str();
    if path_outside_workspace(params, workspace) {
        return deny_option(options);
    }

    allow_workspace_option(options)
}

fn path_outside_workspace(params: &Value, workspace: &str) -> bool {
    let paths = collect_path_fields(params);
    if paths.is_empty() {
        return false;
    }
    let workspace = normalize_path(Path::new(workspace.trim()));
    paths.iter().any(|path| {
        let path = path.trim();
        if path.is_empty() {
            return false;
        }
        let path = path.strip_prefix("file://").unwrap_or(path);
        !path_within_workspace(&workspace, Path::new(path))
    })
}

fn is_path_field_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k == "path"
        || k == "uri"
        || k == "filepath"
        || k == "file"
        || k.ends_with("path")
        || k.ends_with("uri")
}

fn collect_path_fields(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_path_fields_inner(value, &mut out);
    out
}

fn collect_path_fields_inner(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_path_fields_inner(item, out);
            }
        }
        Value::Object(map) => {
            for (key, val) in map {
                if is_path_field_key(key) {
                    if let Some(s) = val.as_str() {
                        out.push(s.to_owned());
                    } else if let Some(s) = val.get("path").and_then(Value::as_str) {
                        out.push(s.to_owned());
                    }
                }
                collect_path_fields_inner(val, out);
            }
        }
        _ => {}
    }
}

pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

pub(crate) fn path_within_workspace(workspace: &Path, candidate: &Path) -> bool {
    let workspace = normalize_path(workspace);
    let candidate = normalize_path(candidate);
    candidate == workspace || candidate.starts_with(&workspace)
}

fn allow_workspace_option(options: &[Value]) -> Option<String> {
    prefer_allow_once(options).or_else(|| prefer_non_always_allow(options))
}

fn prefer_allow_once(options: &[Value]) -> Option<String> {
    for opt in options {
        let kind = option_kind(opt);
        if kind.contains("allow_once") || kind == "allow-once" {
            return option_id(opt);
        }
    }
    None
}

fn prefer_non_always_allow(options: &[Value]) -> Option<String> {
    for opt in options {
        let kind = option_kind(opt);
        if kind.contains("deny") || kind.contains("cancel") || kind.contains("reject") {
            continue;
        }
        if kind.contains("always") {
            continue;
        }
        return option_id(opt);
    }
    None
}

fn deny_write_options(options: &[Value]) -> Option<String> {
    for opt in options {
        let kind = option_kind(opt);
        if kind.contains("deny")
            || kind.contains("cancel")
            || kind.contains("reject")
            || kind.contains("refuse")
        {
            return option_id(opt);
        }
    }
    None
}

fn option_kind(opt: &Value) -> String {
    opt.get("kind")
        .or_else(|| opt.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase()
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
            read_only_fail_closed: false,
        };
        assert_eq!(
            choose_permission_option(&params, &ctx).as_deref(),
            Some("deny")
        );
    }

    // TC-1325 / FR-1304: prefer allow_once over always-allow (Grok).
    #[test]
    fn prefers_allow_once_over_always_allow() {
        let params = json!({
            "options": [
                {"optionId": "always", "kind": "allow_always"},
                {"optionId": "once", "kind": "allow_once"}
            ]
        });
        let ctx = PermissionContext {
            workspace: "/home/user/proj".to_owned(),
            sensitive_gate_active: false,
            read_only_fail_closed: false,
        };
        assert_eq!(
            choose_permission_option(&params, &ctx).as_deref(),
            Some("once")
        );
    }

    // TC-1313 / FR-1313: session ids and titles must not be treated as paths.
    #[test]
    fn tc_1313_ignores_session_id_strings_for_workspace_check() {
        let params = json!({
            "options": [
                {"optionId": "allow", "kind": "allow"},
                {"optionId": "deny", "kind": "deny"}
            ],
            "sessionId": "11111111-1111-1111-1111-111111111111",
            "title": "/tmp/not-a-real-path"
        });
        let ctx = PermissionContext {
            workspace: "/home/user/proj".to_owned(),
            sensitive_gate_active: false,
            read_only_fail_closed: false,
        };
        assert_eq!(
            choose_permission_option(&params, &ctx).as_deref(),
            Some("allow")
        );
    }

    // TC-1314 / FR-1304: component-wise prefix — /proj-evil is not inside /proj.
    #[test]
    fn tc_1314_path_prefix_is_component_wise() {
        assert!(!path_within_workspace(
            Path::new("/home/user/proj"),
            Path::new("/home/user/proj-evil/secret")
        ));
        assert!(path_within_workspace(
            Path::new("/home/user/proj"),
            Path::new("/home/user/proj/src/main.rs")
        ));
        assert!(!path_within_workspace(
            Path::new("/home/user/proj"),
            Path::new("/home/user/proj/../proj-evil")
        ));
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
            read_only_fail_closed: false,
        };
        assert!(choose_permission_option(&params, &ctx).is_none());
    }
}
