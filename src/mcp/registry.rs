//! Resolved MCP server registry backing the dashboard routes (slice C:
//! INT-562, FR-561, FR-562). It owns the resolved server list plus the
//! resolve-time warnings and connects lazily, one client per import request.
//!
//! Nothing here ever renders an env value, a header value or a URL query
//! string; see [`McpRegistry::servers_view`].

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};

use crate::mcp::client::McpClient;
use crate::mcp::config::{
    McpServerSpec, ResolvedMcpServer, ServerKind, Transport, resolve_servers,
};
use crate::todo::{
    ExternalMeta, MAX_BODY_CHARS, MAX_TITLE_CHARS, NewTodo, Todo, TodoError, TodoSource, TodoStore,
    is_valid_external_key, is_valid_link_url,
};

/// Per-request connect/tool timeout (matches the PLAN "20 s").
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Maximum items imported per request; the response reports `truncated: true`
/// when a broader result was cut off (finding 7).
const MAX_IMPORT_ITEMS: usize = 100;

/// Maximum server-provided error text surfaced in a 502 body (finding 5).
const MAX_ERROR_CHARS: usize = 500;

// Jira issue key (e.g. `DEMO-1`) or a numeric Confluence page id select the
// `tools.get` form; anything else is a search.
static TICKET_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9]+-\d+$").expect("valid ticket key regex"));
static PAGE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+$").expect("valid page id regex"));

/// Redacted view returned by `GET /api/mcp/servers` (FR-562).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpServersView {
    pub servers: Vec<McpServerView>,
    pub warnings: Vec<String>,
}

/// One redacted server. Carries no command env, header value or URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpServerView {
    pub name: String,
    pub kind: &'static str,
    pub transport: &'static str,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Result of `POST /api/import` (FR-561).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportResult {
    pub imported: usize,
    pub updated: usize,
    pub todos: Vec<Todo>,
    /// True when the server returned more than [`MAX_IMPORT_ITEMS`] items and
    /// the import was cut off (finding 7).
    #[serde(skip_serializing_if = "is_false")]
    pub truncated: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Failure mode of an import; the route maps each to a status code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// No merged server with that name (404).
    UnknownServer(String),
    /// Server exists but is neither `jira` nor `confluence` (400).
    WrongKind { server: String, kind: &'static str },
    /// Query or store validation failed (400).
    Invalid(String),
    /// Connect or tool error; message never contains a secret (502).
    Tool { server: String, message: String },
    /// Local to-do persistence failed (500).
    Storage(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::UnknownServer(server) => write!(f, "unknown mcp server: {server}"),
            ImportError::WrongKind { server, kind } => {
                write!(f, "mcp server {server} is kind {kind}, not jira/confluence")
            }
            ImportError::Invalid(message) => f.write_str(message),
            ImportError::Tool { server, message } => write!(f, "{server}: {message}"),
            ImportError::Storage(message) => write!(f, "to-do storage error: {message}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// Shared, read-mostly registry of resolved MCP servers.
#[derive(Debug, Clone, Default)]
pub struct McpRegistry {
    servers: Vec<ResolvedMcpServer>,
    warnings: Vec<String>,
}

impl McpRegistry {
    pub fn new(servers: Vec<ResolvedMcpServer>, warnings: Vec<String>) -> Self {
        Self { servers, warnings }
    }

    /// Resolves merged settings specs, expanding env references with
    /// `env_lookup` (typically `|k| std::env::var(k).ok()`).
    pub fn from_specs(
        specs: &[McpServerSpec],
        env_lookup: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let (servers, warnings) = resolve_servers(specs, env_lookup);
        Self { servers, warnings }
    }

    pub fn servers(&self) -> &[ResolvedMcpServer] {
        &self.servers
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Redacted listing for the dashboard; never includes secrets, headers or
    /// URL query strings.
    pub fn servers_view(&self) -> McpServersView {
        let servers = self
            .servers
            .iter()
            .map(|server| McpServerView {
                name: server.name.clone(),
                kind: server.kind.as_str(),
                transport: match server.transport {
                    Transport::Stdio { .. } => "stdio",
                    Transport::Http { .. } => "http",
                },
                enabled: true,
                warnings: self.warnings_for(&server.name),
            })
            .collect();
        McpServersView {
            servers,
            warnings: self.warnings.clone(),
        }
    }

    fn warnings_for(&self, name: &str) -> Vec<String> {
        let prefix = format!("mcp server {name} ");
        self.warnings
            .iter()
            .filter(|warning| warning.starts_with(&prefix))
            .cloned()
            .collect()
    }

    /// Imports items for a `jira`/`confluence` server into `store`, upserting by
    /// `(source, source_ref)`. Connects lazily with a 20 s timeout.
    pub async fn import(
        &self,
        server: &str,
        query: &str,
        store: &TodoStore,
    ) -> Result<ImportResult, ImportError> {
        let resolved = self
            .servers
            .iter()
            .find(|entry| entry.name == server)
            .ok_or_else(|| ImportError::UnknownServer(server.to_owned()))?;
        let source = match resolved.kind {
            ServerKind::Jira => TodoSource::Jira,
            ServerKind::Confluence => TodoSource::Confluence,
            other => {
                return Err(ImportError::WrongKind {
                    server: server.to_owned(),
                    kind: other.as_str(),
                });
            }
        };
        if query.trim().is_empty() {
            return Err(ImportError::Invalid("query must not be empty".to_owned()));
        }

        let is_single = TICKET_KEY.is_match(query) || PAGE_ID.is_match(query);
        let (tool, arguments) = if is_single {
            (resolved.tools.get.clone(), json!({ "id": query }))
        } else {
            (resolved.tools.search.clone(), json!({ "query": query }))
        };

        let client = McpClient::connect(resolved, REQUEST_TIMEOUT)
            .await
            .map_err(|error| ImportError::Tool {
                server: resolved.name.clone(),
                message: truncate(error.message().to_owned(), MAX_ERROR_CHARS),
            })?;
        let result =
            client
                .call_tool(&tool, arguments)
                .await
                .map_err(|error| ImportError::Tool {
                    server: resolved.name.clone(),
                    message: truncate(error.message().to_owned(), MAX_ERROR_CHARS),
                })?;

        let existing = store
            .list()
            .map_err(|error| ImportError::Storage(error.to_string()))?;
        // One lookup set instead of an O(items x existing) scan; the import is
        // always a single source, so the ref alone is the key.
        let existing_refs: HashSet<&str> = existing
            .iter()
            .filter(|todo| todo.source == source)
            .filter_map(|todo| todo.source_ref.as_deref())
            .collect();

        let (items, truncated) = prepare_items(&result);

        let mut imported = 0;
        let mut updated = 0;
        let mut todos = Vec::new();
        for item in &items {
            let was_existing = existing_refs.contains(item.id.as_str());
            // FR-563/FR-564: the item id is the external key when it looks like
            // a Jira key, and the item URL is the link when it is a valid
            // absolute http(s) URL; otherwise the configured base URL derives
            // the link from the key inside `create_with_link`.
            let external_key = is_valid_external_key(&item.id).then(|| item.id.clone());
            let link_url = item.url.clone().filter(|url| is_valid_link_url(url));
            let body = build_body(item);
            let title = truncate(item.title.clone(), MAX_TITLE_CHARS);
            let title = if title.trim().is_empty() {
                // Truncate the id too, so a weird over-long id cannot fail
                // validation and turn the whole import into a 400 (finding 8).
                truncate(item.id.clone(), MAX_TITLE_CHARS)
            } else {
                title
            };
            let source_ref = item.id.clone();
            let todo = store
                .create_with_external(
                    NewTodo {
                        title,
                        body,
                        source,
                        source_ref: Some(source_ref),
                    },
                    ExternalMeta {
                        external_key,
                        link_url,
                        external_status: item.status.clone(),
                        external_assignee: item.assignee.clone(),
                    },
                )
                .map_err(|error| match error {
                    TodoError::Invalid(message) => ImportError::Invalid(message),
                    other => ImportError::Storage(other.to_string()),
                })?;
            if was_existing {
                updated += 1;
            } else {
                imported += 1;
            }
            todos.push(todo);
        }

        Ok(ImportResult {
            imported,
            updated,
            todos,
            truncated,
        })
    }
}

/// Parses the result items, caps them at [`MAX_IMPORT_ITEMS`] and drops
/// duplicate ids. Returns the items and whether the source list was cut off.
fn prepare_items(result: &Value) -> (Vec<ImportItem>, bool) {
    let values = item_values(result);
    let truncated = values.len() > MAX_IMPORT_ITEMS;
    let mut items = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for value in values.iter().take(MAX_IMPORT_ITEMS) {
        let Some(item) = parse_item(value) else {
            continue;
        };
        if !seen.insert(item.id.clone()) {
            continue;
        }
        items.push(item);
    }
    (items, truncated)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportItem {
    id: String,
    title: String,
    url: Option<String>,
    body: Option<String>,
    status: Option<String>,
    assignee: Option<String>,
}

/// Reads `items[]` from `structuredContent` first (the mock and MCP
/// structured results), falling back to the first JSON `text` content part.
fn item_values(result: &Value) -> Vec<Value> {
    if let Some(items) = result
        .pointer("/structuredContent/items")
        .and_then(Value::as_array)
    {
        return items.clone();
    }
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for part in content {
            if let Some(text) = part.get("text").and_then(Value::as_str)
                && let Ok(parsed) = serde_json::from_str::<Value>(text)
                && let Some(items) = parsed.get("items").and_then(Value::as_array)
            {
                return items.clone();
            }
        }
    }
    Vec::new()
}

fn parse_item(value: &Value) -> Option<ImportItem> {
    let id = match value.get("id") {
        Some(Value::String(text)) if !text.is_empty() => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => return None,
    };
    Some(ImportItem {
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(id.as_str())
            .to_owned(),
        url: value.get("url").and_then(Value::as_str).map(str::to_owned),
        body: value.get("body").and_then(Value::as_str).map(str::to_owned),
        status: external_text(value.get("status")),
        assignee: external_text(value.get("assignee")),
        id,
    })
}

/// Reads a Jira-style `status`/`assignee` field: a plain string, or an object
/// carrying `displayName`/`name` (FR-563).
fn external_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => (!text.trim().is_empty()).then(|| text.clone()),
        Value::Object(map) => ["displayName", "name"]
            .iter()
            .find_map(|key| map.get(*key).and_then(Value::as_str))
            .map(str::to_owned),
        _ => None,
    }
}

fn build_body(item: &ImportItem) -> String {
    let mut body = String::new();
    if let Some(url) = &item.url {
        body.push_str(url);
    }
    if let Some(text) = &item.body {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(text);
    }
    truncate(body, MAX_BODY_CHARS)
}

/// Truncates on a char boundary so a multi-byte title/body never panics.
fn truncate(mut value: String, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value;
    }
    let cutoff = value
        .char_indices()
        .nth(limit)
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    value.truncate(cutoff);
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(name: &str, kind: ServerKind) -> ResolvedMcpServer {
        ResolvedMcpServer {
            name: name.to_owned(),
            kind,
            transport: Transport::Stdio {
                command: "npx".to_owned(),
                args: vec!["-y".to_owned(), "server".to_owned()],
                env: [("API_TOKEN".to_owned(), "demo-token".to_owned())]
                    .into_iter()
                    .collect(),
            },
            tools: crate::mcp::config::ToolNames::default(),
        }
    }

    #[test]
    fn servers_view_omits_secrets_and_query_strings() {
        // FR-562: no env value, no header value, no URL (and so no query).
        let http = ResolvedMcpServer {
            name: "jira".to_owned(),
            kind: ServerKind::Jira,
            transport: Transport::Http {
                url: "https://mcp.example.invalid/mcp?key=demo-query".to_owned(),
                headers: [("Authorization".to_owned(), "demo-header".to_owned())]
                    .into_iter()
                    .collect(),
            },
            tools: crate::mcp::config::ToolNames::default(),
        };
        let registry = McpRegistry::new(vec![stdio("demo", ServerKind::Generic), http], Vec::new());
        let view = registry.servers_view();
        assert_eq!(view.servers.len(), 2);
        assert_eq!(view.servers[0].transport, "stdio");
        assert_eq!(view.servers[1].transport, "http");
        assert_eq!(view.servers[1].kind, "jira");
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("demo-token"));
        assert!(!json.contains("demo-header"));
        assert!(!json.contains("demo-query"));
        assert!(!json.contains("example.invalid"));
    }

    // Finding 7: the import caps the item count, reports truncation and drops
    // duplicate ids from one result.
    #[test]
    fn import_caps_and_dedupes_items() {
        let mut items: Vec<Value> = (0..(MAX_IMPORT_ITEMS + 5))
            .map(|index| json!({ "id": format!("DEMO-{index}"), "title": "t" }))
            .collect();
        items.insert(1, json!({ "id": "DEMO-0", "title": "duplicate" }));
        let result = json!({ "structuredContent": { "items": items } });

        let (prepared, truncated) = prepare_items(&result);
        assert!(truncated, "more than the cap reports truncated");
        assert!(prepared.len() <= MAX_IMPORT_ITEMS);
        assert_eq!(prepared[0].id, "DEMO-0");
        let mut ids: Vec<&str> = prepared.iter().map(|item| item.id.as_str()).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "duplicate ids are dropped");

        // Exactly at the cap is not truncated.
        let exact = json!({
            "structuredContent": {
                "items": (0..MAX_IMPORT_ITEMS)
                    .map(|index| json!({ "id": format!("K-{index}") }))
                    .collect::<Vec<_>>()
            }
        });
        let (prepared, truncated) = prepare_items(&exact);
        assert!(!truncated);
        assert_eq!(prepared.len(), MAX_IMPORT_ITEMS);

        // The response only carries `truncated` when it is true.
        let cut = ImportResult {
            imported: 0,
            updated: 0,
            todos: Vec::new(),
            truncated: true,
        };
        assert!(
            serde_json::to_string(&cut)
                .unwrap()
                .contains("\"truncated\":true"),
            "truncated import must say so"
        );
        let whole = ImportResult {
            imported: 0,
            updated: 0,
            todos: Vec::new(),
            truncated: false,
        };
        assert!(!serde_json::to_string(&whole).unwrap().contains("truncated"));
    }
}
