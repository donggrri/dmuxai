//! Pure functions that turn a resolved MCP server list into each runner's
//! native per-run form, plus the Cursor project-file guard and the AGY notice.
//!
//! Secrets only ever travel through generated per-run files/env, never through
//! arguments that name a value. See docs/specs/SPEC-web-dashboard.md (INT-561,
//! TC-563..TC-566) and the PLAN "Slice C contract" runner matrix.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::config::{ResolvedMcpServer, Transport, redact_url};

fn json_string_map(map: &BTreeMap<String, String>) -> Value {
    Value::Object(
        map.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

fn stdio_command_line(command: &str, args: &[String]) -> Value {
    let mut line = Vec::with_capacity(args.len() + 1);
    line.push(Value::String(command.to_owned()));
    line.extend(args.iter().cloned().map(Value::String));
    Value::Array(line)
}

/// Adds the `mcp` key to a per-run opencode config JSON object. Existing keys
/// (for example `permission`) are preserved, and an existing `mcp.<name>` entry
/// wins over the generated one.
///
/// Returns `(config, applied)`. `applied` is `false` when the servers could not
/// be merged because the JSON is not an object or an existing `mcp` key is not
/// an object, so the caller can raise the once-per-session MCP notice
/// (INT-561). An empty server list leaves the config unchanged and reports
/// `true` (nothing to apply).
pub fn merge_opencode_mcp(config_json: &str, servers: &[ResolvedMcpServer]) -> (String, bool) {
    if servers.is_empty() {
        return (config_json.to_owned(), true);
    }
    let Ok(mut value) = serde_json::from_str::<Value>(config_json) else {
        return (config_json.to_owned(), false);
    };
    let Some(root) = value.as_object_mut() else {
        return (config_json.to_owned(), false);
    };
    let mcp_value = root
        .entry("mcp".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(mcp) = mcp_value.as_object_mut() else {
        return (config_json.to_owned(), false);
    };
    for server in servers {
        if mcp.contains_key(&server.name) {
            continue;
        }
        mcp.insert(server.name.clone(), opencode_entry(server));
    }
    let merged = serde_json::to_string(&value).unwrap_or_else(|_| config_json.to_owned());
    (merged, true)
}

fn opencode_entry(server: &ResolvedMcpServer) -> Value {
    let mut object = Map::new();
    match &server.transport {
        Transport::Stdio { command, args, env } => {
            object.insert("type".to_owned(), json!("local"));
            object.insert("command".to_owned(), stdio_command_line(command, args));
            if !env.is_empty() {
                object.insert("environment".to_owned(), json_string_map(env));
            }
        }
        Transport::Http { url, headers } => {
            object.insert("type".to_owned(), json!("remote"));
            object.insert("url".to_owned(), Value::String(url.clone()));
            if !headers.is_empty() {
                object.insert("headers".to_owned(), json_string_map(headers));
            }
        }
    }
    object.insert("enabled".to_owned(), json!(true));
    Value::Object(object)
}

fn toml_encode(value: toml::Value) -> String {
    value.to_string()
}

fn toml_string(value: &str) -> String {
    toml_encode(toml::Value::String(value.to_owned()))
}

fn toml_string_array(items: &[String]) -> String {
    toml_encode(toml::Value::Array(
        items.iter().cloned().map(toml::Value::String).collect(),
    ))
}

fn toml_string_table(map: &BTreeMap<String, String>) -> String {
    let table = map
        .iter()
        .map(|(key, value)| (key.clone(), toml::Value::String(value.clone())))
        .collect();
    toml_encode(toml::Value::Table(table))
}

fn push_override(args: &mut Vec<String>, name: &str, field: &str, encoded: &str) {
    args.push("-c".to_owned());
    args.push(format!("mcp_servers.{name}.{field}={encoded}"));
}

/// Builds Codex `-c mcp_servers.<name>.<field>=<toml>` argv pairs and child process
/// environment variables, ensuring no resolved secret values appear in argv (NFR-561).
pub fn codex_config_overrides_and_env(
    servers: &[ResolvedMcpServer],
) -> (Vec<String>, BTreeMap<String, String>, Vec<String>) {
    let mut args = Vec::new();
    let mut child_env = BTreeMap::new();
    let mut notices = Vec::new();

    for server in servers {
        match &server.transport {
            Transport::Stdio {
                command,
                args: cmd_args,
                env,
            } => {
                let mut valid = true;
                for k in env.keys() {
                    if k.contains('=') || k.contains('\0') || k.is_empty() {
                        valid = false;
                        break;
                    }
                }
                if !valid {
                    notices.push(format!(
                        "MCP server '{}' contains an invalid environment variable key; cannot be passed to Codex.",
                        server.name
                    ));
                    continue;
                }

                push_override(&mut args, &server.name, "command", &toml_string(command));
                if !cmd_args.is_empty() {
                    push_override(
                        &mut args,
                        &server.name,
                        "args",
                        &toml_string_array(cmd_args),
                    );
                }
                if !env.is_empty() {
                    let mut env_vars = Vec::new();
                    for (k, v) in env {
                        child_env.insert(k.clone(), v.clone());
                        env_vars.push(k.clone());
                    }
                    push_override(
                        &mut args,
                        &server.name,
                        "env_vars",
                        &toml_string_array(&env_vars),
                    );
                }
            }
            Transport::Http { url, headers } => {
                let mut valid = true;
                for k in headers.keys() {
                    if k.contains('\0') || k.is_empty() {
                        valid = false;
                        break;
                    }
                }
                if !valid {
                    notices.push(format!(
                        "MCP server '{}' contains an invalid HTTP header key; cannot be passed to Codex.",
                        server.name
                    ));
                    continue;
                }

                push_override(&mut args, &server.name, "url", &toml_string(url));
                if !headers.is_empty() {
                    let mut env_http_headers = BTreeMap::new();
                    for (k, v) in headers {
                        let sanitized_srv = server.name.to_ascii_uppercase().replace('-', "_");
                        let sanitized_hdr = k.to_ascii_uppercase().replace('-', "_");
                        let env_var_name = format!("DMUXAI_MCP_{sanitized_srv}_{sanitized_hdr}");
                        child_env.insert(env_var_name.clone(), v.clone());
                        env_http_headers.insert(k.clone(), env_var_name);
                    }
                    push_override(
                        &mut args,
                        &server.name,
                        "env_http_headers",
                        &toml_string_table(&env_http_headers),
                    );
                }
            }
        }
    }
    (args, child_env, notices)
}

/// Builds Codex `-c mcp_servers.<name>.<field>=<toml>` argv pairs, inserted
/// before the prompt/subcommand. Values are TOML-encoded, so quotes and
/// backslashes survive round-trips.
pub fn codex_config_overrides(servers: &[ResolvedMcpServer]) -> Vec<String> {
    let (args, _, _) = codex_config_overrides_and_env(servers);
    args
}

/// Claude/Cursor-style `{"mcpServers":{...}}` document, written per turn for
/// custom agents and used as the Cursor project file body.
pub fn custom_mcp_json(servers: &[ResolvedMcpServer]) -> String {
    let mut mcp_servers = Map::new();
    for server in servers {
        mcp_servers.insert(server.name.clone(), custom_entry(server));
    }
    let mut root = Map::new();
    root.insert("mcpServers".to_owned(), Value::Object(mcp_servers));
    serde_json::to_string(&Value::Object(root)).unwrap_or_else(|_| "{\"mcpServers\":{}}".to_owned())
}

fn custom_entry(server: &ResolvedMcpServer) -> Value {
    let mut object = Map::new();
    match &server.transport {
        Transport::Stdio { command, args, env } => {
            object.insert("command".to_owned(), Value::String(command.clone()));
            if !args.is_empty() {
                object.insert(
                    "args".to_owned(),
                    Value::Array(args.iter().cloned().map(Value::String).collect()),
                );
            }
            if !env.is_empty() {
                object.insert("env".to_owned(), json_string_map(env));
            }
        }
        Transport::Http { url, headers } => {
            object.insert("url".to_owned(), Value::String(url.clone()));
            if !headers.is_empty() {
                object.insert("headers".to_owned(), json_string_map(headers));
            }
        }
    }
    Value::Object(object)
}

/// Remove-on-drop guard for a project `.cursor/mcp.json` DmuxAI wrote itself.
#[derive(Debug)]
pub struct CursorMcpGuard {
    file: PathBuf,
    dir: PathBuf,
    created_dir: bool,
}

impl Drop for CursorMcpGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
        if self.created_dir {
            let empty = std::fs::read_dir(&self.dir)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false);
            if empty {
                let _ = std::fs::remove_dir(&self.dir);
            }
        }
    }
}

/// Writes `<workspace>/.cursor/mcp.json` only when it does not already exist.
/// Returns `Ok(None)` when the user owns the file (the caller emits a notice).
/// On unix the file is created with mode 0600. The returned guard removes the
/// file, and the `.cursor` directory when DmuxAI created it and it is empty.
pub fn cursor_mcp_guard(
    workspace: &Path,
    servers: &[ResolvedMcpServer],
) -> std::io::Result<Option<CursorMcpGuard>> {
    let dir = workspace.join(".cursor");
    let file = dir.join("mcp.json");
    if file.exists() {
        return Ok(None);
    }
    let created_dir = if dir.exists() {
        false
    } else {
        std::fs::create_dir_all(&dir)?;
        true
    };
    let contents = custom_mcp_json(servers);

    // `create_new(true)` on every platform so a file that appears after the
    // `exists()` check is never truncated/overwritten (fail closed).
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    use std::io::Write as _;
    let mut handle = options.open(&file)?;
    handle.write_all(contents.as_bytes())?;

    Ok(Some(CursorMcpGuard {
        file,
        dir,
        created_dir,
    }))
}

/// A notice listing the equivalent `agy mcp add` commands. AGY only has a
/// user-level store, so DmuxAI never runs these (OQ-523). Flags come before
/// `<name>` (the CLI rejects flags after it), `--header` uses the required
/// `"Key: Value"` form, environment/header values are shown as `$KEY`
/// placeholders, and URLs are stripped of any query/fragment.
pub fn agy_mcp_notice(servers: &[ResolvedMcpServer]) -> Option<String> {
    if servers.is_empty() {
        return None;
    }
    let mut out =
        String::from("MCP servers are not propagated to AGY automatically; add them once with:\n");
    for server in servers {
        match &server.transport {
            Transport::Stdio { command, args, env } => {
                let _ = write!(out, "  agy mcp add");
                for key in env.keys() {
                    let _ = write!(out, " --env {key}=${key}");
                }
                let _ = write!(out, " {} -- {}", server.name, command);
                for arg in args {
                    let _ = write!(out, " {arg}");
                }
                out.push('\n');
            }
            Transport::Http { url, headers } => {
                let _ = write!(out, "  agy mcp add");
                for key in headers.keys() {
                    let _ = write!(out, " --header \"{key}: ${key}\"");
                }
                let _ = writeln!(out, " {} {}", server.name, redact_url(url));
            }
        }
    }
    Some(out.trim_end().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::config::{ResolvedMcpServer, ServerKind, ToolNames, Transport};

    fn stdio(name: &str, command: &str, args: &[&str], env: &[(&str, &str)]) -> ResolvedMcpServer {
        ResolvedMcpServer {
            name: name.to_owned(),
            kind: ServerKind::Generic,
            transport: Transport::Stdio {
                command: command.to_owned(),
                args: args.iter().map(|arg| (*arg).to_owned()).collect(),
                env: env
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                    .collect(),
            },
            runners: None,
            tools: ToolNames::default(),
        }
    }

    fn http(name: &str, url: &str, headers: &[(&str, &str)]) -> ResolvedMcpServer {
        ResolvedMcpServer {
            name: name.to_owned(),
            kind: ServerKind::Jira,
            transport: Transport::Http {
                url: url.to_owned(),
                headers: headers
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                    .collect(),
            },
            runners: None,
            tools: ToolNames::default(),
        }
    }

    #[test]
    fn opencode_config_gets_mcp_key() {
        // TC-563, INT-561: temp JSON gets mcp.<name> local/remote entries and
        // preserves existing permission keys.
        let servers = vec![
            stdio(
                "demo",
                "npx",
                &["-y", "@modelcontextprotocol/server-everything"],
                &[("API_TOKEN", "demo-token")],
            ),
            http("jira", "https://mcp.example.invalid/mcp", &[]),
        ];
        let merged = merge_opencode_mcp(r#"{"permission":{"edit":"allow"}}"#, &servers);
        assert!(merged.1, "servers applied");
        let value: Value = serde_json::from_str(&merged.0).unwrap();
        assert_eq!(value["permission"]["edit"], "allow");
        assert_eq!(value["mcp"]["demo"]["type"], "local");
        assert_eq!(value["mcp"]["demo"]["command"][0], "npx");
        assert_eq!(
            value["mcp"]["demo"]["command"][2],
            "@modelcontextprotocol/server-everything"
        );
        assert_eq!(
            value["mcp"]["demo"]["environment"]["API_TOKEN"],
            "demo-token"
        );
        assert_eq!(value["mcp"]["demo"]["enabled"], true);
        assert_eq!(value["mcp"]["jira"]["type"], "remote");
        assert_eq!(
            value["mcp"]["jira"]["url"],
            "https://mcp.example.invalid/mcp"
        );

        // An existing mcp.<name> entry wins over the generated one.
        let existing = r#"{"mcp":{"demo":{"type":"local","command":["already"]}}}"#;
        let merged = merge_opencode_mcp(existing, &servers);
        assert!(merged.1);
        let value: Value = serde_json::from_str(&merged.0).unwrap();
        assert_eq!(value["mcp"]["demo"]["command"][0], "already");
        assert_eq!(value["mcp"]["jira"]["type"], "remote");

        // An empty server list is a no-op and reports applied.
        assert_eq!(
            merge_opencode_mcp("{}", &[]),
            ("{}".to_owned(), true),
            "nothing to apply"
        );

        // A non-object `mcp` key blocks the merge and reports it (INT-561).
        let blocked = r#"{"mcp":"owned","permission":{"edit":"allow"}}"#;
        let (json, applied) = merge_opencode_mcp(blocked, &servers);
        assert!(!applied, "non-object mcp key must report not-applied");
        assert_eq!(json, blocked, "config is left unchanged");

        // Non-object / invalid configs report not-applied and stay unchanged.
        let (json, applied) = merge_opencode_mcp("not json", &servers);
        assert!(!applied);
        assert_eq!(json, "not json");
        let (json, applied) = merge_opencode_mcp("[]", &servers);
        assert!(!applied);
        assert_eq!(json, "[]");
    }

    #[test]
    fn codex_overrides_encode_mcp_servers() {
        // TC-564, INT-561: -c mcp_servers.<name>.* pairs, TOML-encoded so
        // quotes/backslashes survive, http uses url.
        let servers = vec![
            stdio(
                "demo",
                "npx",
                &["-y", "a\"b", "c\\d"],
                &[("API_TOKEN", "demo-token")],
            ),
            http("jira", "https://mcp.example.invalid/mcp", &[]),
        ];
        let overrides = codex_config_overrides(&servers);
        assert_eq!(overrides.len() % 2, 0);
        assert!(overrides.iter().step_by(2).all(|flag| flag == "-c"));
        assert!(overrides.contains(&"mcp_servers.demo.command=\"npx\"".to_owned()));

        // Every value after `=` is valid TOML and round-trips.
        for pair in overrides.chunks(2) {
            let assignment = &pair[1];
            let (key, encoded) = assignment.split_once('=').unwrap();
            let document: toml::Value = toml::from_str(&format!("v = {encoded}")).unwrap();
            let parsed = &document["v"];
            match key {
                "mcp_servers.demo.command" => {
                    assert_eq!(parsed.as_str(), Some("npx"));
                }
                "mcp_servers.demo.args" => {
                    let array = parsed.as_array().unwrap();
                    assert_eq!(array[1].as_str(), Some("a\"b"));
                    assert_eq!(array[2].as_str(), Some("c\\d"));
                }
                "mcp_servers.demo.env_vars" => {
                    let array = parsed.as_array().unwrap();
                    assert_eq!(array[0].as_str(), Some("API_TOKEN"));
                }
                "mcp_servers.jira.url" => {
                    assert_eq!(parsed.as_str(), Some("https://mcp.example.invalid/mcp"));
                }
                other => panic!("unexpected override key {other}"),
            }
        }
    }

    #[test]
    fn custom_agent_mcp_json_shape() {
        // TC-565 (pure part), INT-561: mcpServers JSON with command/args/env or
        // url/headers.
        let servers = vec![
            stdio(
                "demo",
                "npx",
                &["-y", "server"],
                &[("API_TOKEN", "demo-token")],
            ),
            http(
                "jira",
                "https://mcp.example.invalid/mcp",
                &[("Authorization", "demo-token")],
            ),
        ];
        let value: Value = serde_json::from_str(&custom_mcp_json(&servers)).unwrap();
        assert_eq!(value["mcpServers"]["demo"]["command"], "npx");
        assert_eq!(value["mcpServers"]["demo"]["args"][1], "server");
        assert_eq!(
            value["mcpServers"]["demo"]["env"]["API_TOKEN"],
            "demo-token"
        );
        assert_eq!(
            value["mcpServers"]["jira"]["url"],
            "https://mcp.example.invalid/mcp"
        );
        assert_eq!(
            value["mcpServers"]["jira"]["headers"]["Authorization"],
            "demo-token"
        );

        let empty: Value = serde_json::from_str(&custom_mcp_json(&[])).unwrap();
        assert_eq!(empty["mcpServers"], json!({}));
    }

    #[test]
    fn cursor_guard_writes_only_when_absent_and_agy_notice() {
        // TC-566, INT-561: Cursor project file is written only when absent and
        // removed on drop; AGY notice lists commands with $VAR placeholders and
        // no secret values.
        let workspace = tempfile::tempdir().unwrap();
        let servers = vec![
            stdio(
                "demo",
                "npx",
                &["-y", "server"],
                &[("API_TOKEN", "demo-token")],
            ),
            http(
                "jira",
                "https://mcp.example.invalid/mcp?api-key=demo-query-secret",
                &[("Authorization", "demo-token")],
            ),
        ];

        let guard = cursor_mcp_guard(workspace.path(), &servers).unwrap();
        let guard = guard.expect("guard written when absent");
        let cursor_dir = workspace.path().join(".cursor");
        let file = cursor_dir.join("mcp.json");
        assert!(file.exists());
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written["mcpServers"]["demo"]["command"], "npx");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        drop(guard);
        assert!(!file.exists(), "guard must remove the file");
        assert!(
            !cursor_dir.exists(),
            "guard must remove the created empty dir"
        );

        // A pre-existing file is never overwritten.
        std::fs::create_dir_all(&cursor_dir).unwrap();
        std::fs::write(&file, r#"{"mcpServers":{"user":"owned"}}"#).unwrap();
        assert!(
            cursor_mcp_guard(workspace.path(), &servers)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            r#"{"mcpServers":{"user":"owned"}}"#
        );

        // TC-566: the AGY notice uses flags before <name>, the
        // `"Key: $KEY"` header form, `-- <command>`, and no secret/query text.
        let notice = agy_mcp_notice(&servers).expect("notice for configured servers");
        assert!(
            notice.contains("  agy mcp add --env API_TOKEN=$API_TOKEN demo -- npx -y server"),
            "stdio line:\n{notice}"
        );
        assert!(
            notice.contains(
                "  agy mcp add --header \"Authorization: $Authorization\" jira https://mcp.example.invalid/mcp"
            ),
            "http line:\n{notice}"
        );
        assert!(!notice.contains("--transport"), "no bogus --transport flag");
        assert!(!notice.contains("demo-token"));
        assert!(!notice.contains("demo-query-secret"));
        assert!(!notice.contains("api-key"));
        assert!(agy_mcp_notice(&[]).is_none());

        // Finding 1: the write fails closed rather than truncating a file that
        // appears between the `exists()` check and the open. A dangling symlink
        // reports `exists() == false` yet occupies the path.
        #[cfg(unix)]
        {
            let raced = tempfile::tempdir().unwrap();
            let raced_dir = raced.path().join(".cursor");
            std::fs::create_dir_all(&raced_dir).unwrap();
            let raced_file = raced_dir.join("mcp.json");
            std::os::unix::fs::symlink(raced.path().join("missing-target"), &raced_file).unwrap();
            assert!(
                cursor_mcp_guard(raced.path(), &servers).is_err(),
                "must fail closed instead of overwriting"
            );
            assert!(
                std::fs::symlink_metadata(&raced_file).unwrap().is_symlink(),
                "the existing symlink is untouched"
            );
        }
    }
}

#[cfg(test)]
mod builtin_memory_tests {
    use super::*;
    use crate::mcp::resolve_servers;
    use crate::settings::{BUILTIN_MEMORY_SERVER_NAME, DmuxSettings};

    // TC-734 (FR-733, INT-561): with memory on, the built-in entry becomes an
    // opencode `local` entry and a custom `mcpServers` entry, with no env, headers
    // or vault path.
    #[test]
    fn builtin_memory_server_reaches_opencode_and_custom_json() {
        let ws = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        let v = vault.path().display().to_string().replace('\\', "/");
        std::fs::write(
            ws.path().join("dmuxai.toml"),
            format!("[memory]\nvault = \"{v}\"\n"),
        )
        .unwrap();
        let exe = std::env::temp_dir().join("dmuxai-bin").join("dmuxai");
        let exe_str = exe.to_string_lossy().into_owned();
        let ws_str = ws.path().to_string_lossy().into_owned();
        let s = DmuxSettings::load_with_exe(ws.path(), None, Some(&exe));
        let (resolved, warnings) = resolve_servers(&s.mcp_servers, |_| None);
        assert!(warnings.is_empty(), "{warnings:?}");

        let (json, applied) = merge_opencode_mcp("{}", &resolved);
        assert!(applied);
        let value: Value = serde_json::from_str(&json).unwrap();
        let entry = &value["mcp"][BUILTIN_MEMORY_SERVER_NAME];
        assert_eq!(entry["type"], "local");
        assert_eq!(entry["command"][0].as_str(), Some(exe_str.as_str()));
        assert_eq!(entry["command"][1], "mcp-memory");
        assert_eq!(entry["command"][2], "--workspace");
        assert_eq!(entry["command"][3].as_str(), Some(ws_str.as_str()));
        assert!(entry.get("environment").is_none());

        let custom_text = custom_mcp_json(&resolved);
        let custom: Value = serde_json::from_str(&custom_text).unwrap();
        let entry = &custom["mcpServers"][BUILTIN_MEMORY_SERVER_NAME];
        assert_eq!(entry["command"].as_str(), Some(exe_str.as_str()));
        assert_eq!(entry["args"][0], "mcp-memory");
        assert!(entry.get("env").is_none() && entry.get("headers").is_none());

        // The vault path (JSON-escaped) appears in neither generated document.
        let vault_json = Value::String(vault.path().display().to_string()).to_string();
        let needle = &vault_json[1..vault_json.len() - 1];
        assert!(!json.contains(needle), "opencode config leaks vault path");
        assert!(
            !custom_text.contains(needle),
            "custom json leaks vault path"
        );
    }
}

/// ACP `mcpServers` array for `session/new|load|resume` (SPEC-acp-runners FR-1414).
/// Only servers that apply to `runner` are included.
pub fn acp_mcp_servers(servers: &[ResolvedMcpServer], runner: &str) -> Value {
    let pairs = |map: &BTreeMap<String, String>| -> Value {
        Value::Array(
            map.iter()
                .map(|(name, value)| json!({ "name": name, "value": value }))
                .collect(),
        )
    };
    Value::Array(
        servers
            .iter()
            .filter(|server| server.applies_to(runner))
            .map(|server| match &server.transport {
                Transport::Stdio { command, args, env } => json!({
                    "name": server.name,
                    "command": command,
                    "args": args,
                    "env": pairs(env),
                }),
                Transport::Http { url, headers } => json!({
                    "type": "http",
                    "name": server.name,
                    "url": url,
                    "headers": pairs(headers),
                }),
            })
            .collect(),
    )
}
