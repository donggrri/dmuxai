//! Per-repo to-do store (SPEC-web-dashboard FR-540). The store keeps its own
//! `rusqlite::Connection` to the per-repo DB file behind a `Mutex`.

use std::path::Path;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use regex::Regex;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_BODY_CHARS: usize = 20_000;
pub const MAX_EXTERNAL_KEY_CHARS: usize = 64;
pub const MAX_LINK_URL_CHARS: usize = 2048;
/// Jira status/assignee are import-only and truncated, never rejected (FR-563).
pub const MAX_EXTERNAL_STATUS_CHARS: usize = 40;
pub const MAX_EXTERNAL_ASSIGNEE_CHARS: usize = 80;

/// Jira-style issue key (`PROJ-123`), FR-563.
static EXTERNAL_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9_]*-\d+$").expect("valid external key regex"));

const CREATE_SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS todos (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    body TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL CHECK (status IN ('open','running','done')),
    source TEXT NOT NULL,
    source_ref TEXT,
    external_key TEXT,
    link_url TEXT,
    external_status TEXT,
    external_assignee TEXT,
    run_kind TEXT,
    run_ref TEXT,
    last_error TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS todos_by_source_ref
    ON todos(source, source_ref) WHERE source_ref IS NOT NULL;";

const LIST_SQL: &str = "SELECT id, title, body, status, source, source_ref, \
     external_key, link_url, external_status, external_assignee, run_kind, run_ref, last_error, created_at_ms, updated_at_ms \
     FROM todos \
     ORDER BY CASE status \
         WHEN 'open' THEN 0 \
         WHEN 'running' THEN 1 \
         WHEN 'done' THEN 2 \
         ELSE 3 END, \
       created_at_ms ASC, id ASC";

const GET_SQL: &str = "SELECT id, title, body, status, source, source_ref, \
     external_key, link_url, external_status, external_assignee, run_kind, run_ref, last_error, created_at_ms, updated_at_ms \
     FROM todos WHERE id = ?1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Open,
    Running,
    Done,
}

impl TodoStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Running => "running",
            Self::Done => "done",
        }
    }

    fn parse(value: &str) -> Result<Self, TodoError> {
        match value {
            "open" => Ok(Self::Open),
            "running" => Ok(Self::Running),
            "done" => Ok(Self::Done),
            other => Err(TodoError::Storage(format!(
                "unknown to-do status '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoSource {
    Manual,
    Jira,
    Confluence,
}

impl TodoSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Jira => "jira",
            Self::Confluence => "confluence",
        }
    }

    fn parse(value: &str) -> Result<Self, TodoError> {
        match value {
            "manual" => Ok(Self::Manual),
            "jira" => Ok(Self::Jira),
            "confluence" => Ok(Self::Confluence),
            other => Err(TodoError::Storage(format!(
                "unknown to-do source '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    pub id: String,
    pub title: String,
    pub body: String,
    pub status: TodoStatus,
    pub source: TodoSource,
    pub source_ref: Option<String>,
    pub external_key: Option<String>,
    pub link_url: Option<String>,
    /// Jira status name (e.g. `In Progress`); import-only (FR-563).
    pub external_status: Option<String>,
    /// Jira assignee display name; import-only (FR-563).
    pub external_assignee: Option<String>,
    pub run_kind: Option<String>,
    pub run_ref: Option<String>,
    pub last_error: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTodo {
    pub title: String,
    pub body: String,
    pub source: TodoSource,
    pub source_ref: Option<String>,
}

/// Jira/Confluence link and import-managed metadata (FR-563). `external_status`
/// and `external_assignee` are set by import/upsert only; user writes are
/// rejected at the route. Long values are truncated, not rejected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExternalMeta {
    pub external_key: Option<String>,
    pub link_url: Option<String>,
    pub external_status: Option<String>,
    pub external_assignee: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub status: Option<TodoStatus>,
    /// Outer `None` keeps the current value, `Some(None)` clears it and
    /// `Some(Some(value))` sets it (FR-563 PATCH semantics).
    pub external_key: Option<Option<String>>,
    pub link_url: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoError {
    NotFound,
    Invalid(String),
    Storage(String),
}

impl std::fmt::Display for TodoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("to-do not found"),
            Self::Invalid(message) => write!(f, "invalid to-do: {message}"),
            Self::Storage(message) => write!(f, "to-do storage error: {message}"),
        }
    }
}

impl std::error::Error for TodoError {}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn storage(error: rusqlite::Error) -> TodoError {
    TodoError::Storage(error.to_string())
}

fn validate_title(title: &str) -> Result<String, TodoError> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err(TodoError::Invalid("title must not be empty".to_owned()));
    }
    if trimmed.chars().count() > MAX_TITLE_CHARS {
        return Err(TodoError::Invalid(format!(
            "title must be at most {MAX_TITLE_CHARS} characters"
        )));
    }
    Ok(trimmed.to_owned())
}

fn validate_body(body: &str) -> Result<(), TodoError> {
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(TodoError::Invalid(format!(
            "body must be at most {MAX_BODY_CHARS} characters"
        )));
    }
    Ok(())
}

/// True when `value` is a Jira-style issue key (`PROJ-123`, FR-563).
pub fn is_valid_external_key(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= MAX_EXTERNAL_KEY_CHARS
        && EXTERNAL_KEY_RE.is_match(trimmed)
}

/// True when `value` is an absolute `http(s)` URL within the length limit
/// (FR-563); therefore `javascript:` and relative URLs are rejected.
pub fn is_valid_link_url(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_LINK_URL_CHARS {
        return false;
    }
    let Some(authority) = absolute_http_authority(trimmed) else {
        return false;
    };
    !authority.is_empty()
}

fn validate_external_key(key: &str) -> Result<String, TodoError> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err(TodoError::Invalid(
            "external_key must not be empty".to_owned(),
        ));
    }
    if trimmed.chars().count() > MAX_EXTERNAL_KEY_CHARS {
        return Err(TodoError::Invalid(format!(
            "external_key must be at most {MAX_EXTERNAL_KEY_CHARS} characters"
        )));
    }
    if !EXTERNAL_KEY_RE.is_match(trimmed) {
        return Err(TodoError::Invalid(
            "external_key must look like PROJ-123".to_owned(),
        ));
    }
    Ok(trimmed.to_owned())
}

fn validate_link_url(url: &str) -> Result<String, TodoError> {
    let trimmed = url.trim();
    if trimmed.chars().count() > MAX_LINK_URL_CHARS {
        return Err(TodoError::Invalid(format!(
            "link_url must be at most {MAX_LINK_URL_CHARS} characters"
        )));
    }
    if !is_valid_link_url(trimmed) {
        return Err(TodoError::Invalid(
            "link_url must be an absolute http(s) URL".to_owned(),
        ));
    }
    Ok(trimmed.to_owned())
}

/// Returns the authority (host[:port]) of an absolute `http`/`https` URL, or
/// `None` for any other scheme or a relative reference.
fn absolute_http_authority(value: &str) -> Option<&str> {
    let lower = value.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("http://")
        .map(|_| &value["http://".len()..])
        .or_else(|| {
            lower
                .strip_prefix("https://")
                .map(|_| &value["https://".len()..])
        })?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Extracts `KEY-N` from a Jira browse path (`.../browse/KEY-N`, FR-564).
fn jira_key_from_browse_url(url: &str) -> Option<String> {
    let without_fragment = url.split('#').next().unwrap_or(url);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    let (_, after) = without_query.split_once("/browse/")?;
    let candidate = after.split('/').next().unwrap_or("").trim();
    is_valid_external_key(candidate).then(|| candidate.to_owned())
}

/// Pure Jira link derivation (FR-564, TC-571):
/// - a key without a link and with a base URL gains `<base>/browse/<KEY>`;
/// - a browse URL without a key gains that key;
/// - without a base URL the key is kept without a link.
pub fn derive_jira_link(
    key: Option<&str>,
    url: Option<&str>,
    base: Option<&str>,
) -> (Option<String>, Option<String>) {
    let base = base
        .map(str::trim)
        .map(|value| value.trim_end_matches('/'))
        .filter(|value| !value.is_empty());
    let mut key = key
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let mut url = url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);

    if key.is_none()
        && let Some(link) = &url
        && let Some(extracted) = jira_key_from_browse_url(link)
    {
        key = Some(extracted);
    }
    if url.is_none()
        && let (Some(external_key), Some(base)) = (key.as_deref(), base)
    {
        url = Some(format!("{base}/browse/{external_key}"));
    }
    (key, url)
}

fn apply_field_update(field: &Option<Option<String>>, current: &Option<String>) -> Option<String> {
    match field {
        None => current.clone(),
        Some(None) => None,
        Some(Some(value)) => Some(value.clone()),
    }
}

/// Truncates on a char boundary so a multi-byte value never panics.
fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let cutoff = value
        .char_indices()
        .nth(limit)
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    value[..cutoff].to_owned()
}

/// Import-only text field: blank becomes `None`; longer than `limit` is
/// truncated on a char boundary (FR-563).
fn normalize_external_text(value: Option<String>, limit: usize) -> Option<String> {
    let value = value?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_chars(trimmed, limit))
}

/// Adds the slice C link/status columns to a pre-existing `todos` table.
/// Additive only: `ALTER TABLE ... ADD COLUMN` when missing, never a
/// `user_version` bump (FR-563).
fn ensure_optional_columns(connection: &Connection) -> Result<(), TodoError> {
    let mut statement = connection
        .prepare("PRAGMA table_info(todos)")
        .map_err(storage)?;
    let mut rows = statement.query([]).map_err(storage)?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next().map_err(storage)? {
        let name: String = row.get("name").map_err(storage)?;
        columns.push(name);
    }
    drop(rows);
    drop(statement);
    for column in [
        "external_key",
        "link_url",
        "external_status",
        "external_assignee",
    ] {
        if !columns.iter().any(|existing| existing == column) {
            connection
                .execute(&format!("ALTER TABLE todos ADD COLUMN {column} TEXT"), [])
                .map_err(storage)?;
        }
    }
    Ok(())
}

fn todo_from_row(row: &rusqlite::Row<'_>) -> Result<Todo, TodoError> {
    let id: String = row.get("id").map_err(storage)?;
    let title: String = row.get("title").map_err(storage)?;
    let body: String = row.get("body").map_err(storage)?;
    let status: String = row.get("status").map_err(storage)?;
    let source: String = row.get("source").map_err(storage)?;
    let created_at_ms: i64 = row.get("created_at_ms").map_err(storage)?;
    let updated_at_ms: i64 = row.get("updated_at_ms").map_err(storage)?;
    Ok(Todo {
        id,
        title,
        body,
        status: TodoStatus::parse(&status)?,
        source: TodoSource::parse(&source)?,
        source_ref: row.get("source_ref").map_err(storage)?,
        external_key: row.get("external_key").map_err(storage)?,
        link_url: row.get("link_url").map_err(storage)?,
        external_status: row.get("external_status").map_err(storage)?,
        external_assignee: row.get("external_assignee").map_err(storage)?,
        run_kind: row.get("run_kind").map_err(storage)?,
        run_ref: row.get("run_ref").map_err(storage)?,
        last_error: row.get("last_error").map_err(storage)?,
        created_at_ms: created_at_ms.max(0) as u64,
        updated_at_ms: updated_at_ms.max(0) as u64,
    })
}

fn fetch(connection: &Connection, id: &str) -> Result<Todo, TodoError> {
    let mut statement = connection.prepare(GET_SQL).map_err(storage)?;
    let mut rows = statement.query(params![id]).map_err(storage)?;
    match rows.next().map_err(storage)? {
        Some(row) => todo_from_row(row),
        None => Err(TodoError::NotFound),
    }
}

/// Thread-safe store over its own SQLite connection to the per-repo DB file.
pub struct TodoStore {
    connection: Mutex<rusqlite::Connection>,
    jira_base_url: Mutex<Option<String>>,
}

impl TodoStore {
    /// Opens (creating the parent dir and the additive `todos` table if needed).
    pub fn open(db_path: &Path) -> Result<Self, TodoError> {
        if let Some(parent) = db_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| TodoError::Storage(error.to_string()))?;
        }
        let connection = Connection::open(db_path).map_err(storage)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(storage)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(storage)?;
        connection.execute_batch(CREATE_SCHEMA).map_err(storage)?;
        ensure_optional_columns(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
            jira_base_url: Mutex::new(None),
        })
    }

    /// Sets the `[todo] jira_base_url` used to derive links for keys (FR-564).
    /// The value is expected to already be validated and slash-trimmed.
    pub fn set_jira_base_url(&self, base: Option<String>) {
        *self
            .jira_base_url
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = base;
    }

    /// The configured `[todo] jira_base_url`, if any (FR-564); the dashboard
    /// snapshot surfaces it so the page can preview links.
    pub fn jira_base_url(&self) -> Option<String> {
        self.jira_base_url
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn lock(&self) -> MutexGuard<'_, rusqlite::Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Open/running first, then by `created_at_ms` ascending.
    pub fn list(&self) -> Result<Vec<Todo>, TodoError> {
        let connection = self.lock();
        let mut statement = connection.prepare(LIST_SQL).map_err(storage)?;
        let mut rows = statement.query([]).map_err(storage)?;
        let mut todos = Vec::new();
        while let Some(row) = rows.next().map_err(storage)? {
            todos.push(todo_from_row(row)?);
        }
        Ok(todos)
    }

    pub fn get(&self, id: &str) -> Result<Todo, TodoError> {
        fetch(&self.lock(), id)
    }

    /// Validates title/body. When `source_ref` is set and `(source, source_ref)`
    /// exists, updates title/body of that row instead (status kept). The
    /// insert-or-update is a single atomic statement, so concurrent writers on
    /// the same DB never race into a unique-index 500.
    pub fn create(&self, new: NewTodo) -> Result<Todo, TodoError> {
        self.create_with_external(new, ExternalMeta::default())
    }

    /// Like [`Self::create`] but with only the user-settable Jira link fields
    /// (FR-563/FR-564); status/assignee default to unset.
    pub fn create_with_link(
        &self,
        new: NewTodo,
        external_key: Option<String>,
        link_url: Option<String>,
    ) -> Result<Todo, TodoError> {
        self.create_with_external(
            new,
            ExternalMeta {
                external_key,
                link_url,
                ..ExternalMeta::default()
            },
        )
    }

    /// Like [`Self::create`] but also stores the Jira link and import-managed
    /// metadata (FR-563/FR-564). `external_key` is the issue key, `link_url` a
    /// URL; both are validated and completed from the configured base URL by
    /// [`derive_jira_link`]. `external_status`/`external_assignee` are
    /// import-only and truncated. An upsert by `(source, source_ref)` refreshes
    /// all four fields.
    pub fn create_with_external(
        &self,
        new: NewTodo,
        meta: ExternalMeta,
    ) -> Result<Todo, TodoError> {
        let title = validate_title(&new.title)?;
        validate_body(&new.body)?;
        let external_key = match meta.external_key {
            Some(key) => Some(validate_external_key(&key)?),
            None => None,
        };
        let link_url = match meta.link_url {
            Some(url) => Some(validate_link_url(&url)?),
            None => None,
        };
        let external_status =
            normalize_external_text(meta.external_status, MAX_EXTERNAL_STATUS_CHARS);
        let external_assignee =
            normalize_external_text(meta.external_assignee, MAX_EXTERNAL_ASSIGNEE_CHARS);
        let base = self.jira_base_url();
        let (external_key, link_url) = derive_jira_link(
            external_key.as_deref(),
            link_url.as_deref(),
            base.as_deref(),
        );
        let connection = self.lock();
        let now = now_ms() as i64;
        let id: String = connection
            .query_row(
                "INSERT INTO todos \
                 (id, title, body, status, source, source_ref, external_key, link_url, external_status, external_assignee, run_kind, run_ref, last_error, created_at_ms, updated_at_ms) \
                 VALUES (?1, ?2, ?3, 'open', ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL, NULL, ?10, ?10) \
                 ON CONFLICT(source, source_ref) WHERE source_ref IS NOT NULL \
                 DO UPDATE SET title = excluded.title, body = excluded.body, \
                     external_key = excluded.external_key, link_url = excluded.link_url, \
                     external_status = excluded.external_status, \
                     external_assignee = excluded.external_assignee, \
                     updated_at_ms = excluded.updated_at_ms \
                 RETURNING id",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    title,
                    new.body,
                    new.source.as_str(),
                    new.source_ref,
                    external_key,
                    link_url,
                    external_status,
                    external_assignee,
                    now
                ],
                |row| row.get(0),
            )
            .map_err(storage)?;
        fetch(&connection, &id)
    }

    /// Validates; `status: Some(Running)` -> `Invalid`. Title, body, status and
    /// the Jira link fields are applied in one atomic `UPDATE`, so readers never
    /// see a half-applied patch. `external_key`/`link_url` accept
    /// `Some(None)` to clear (FR-563).
    pub fn update(&self, id: &str, patch: TodoPatch) -> Result<Todo, TodoError> {
        let validated_title = match &patch.title {
            Some(title) => Some(validate_title(title)?),
            None => None,
        };
        if let Some(body) = &patch.body {
            validate_body(body)?;
        }
        if patch.status == Some(TodoStatus::Running) {
            return Err(TodoError::Invalid(
                "cannot set status to running directly".to_owned(),
            ));
        }

        let connection = self.lock();
        let current = fetch(&connection, id)?;
        let touches_link = patch.external_key.is_some() || patch.link_url.is_some();
        if validated_title.is_none()
            && patch.body.is_none()
            && patch.status.is_none()
            && !touches_link
        {
            return Ok(current);
        }

        let key = apply_field_update(&patch.external_key, &current.external_key);
        let link = apply_field_update(&patch.link_url, &current.link_url);
        let key = match key {
            Some(value) => Some(validate_external_key(&value)?),
            None => None,
        };
        let link = match link {
            Some(value) => Some(validate_link_url(&value)?),
            None => None,
        };
        let base = self.jira_base_url();
        let (mut external_key, mut link_url) =
            derive_jira_link(key.as_deref(), link.as_deref(), base.as_deref());
        // An explicit `null` clears the field; do not re-derive it.
        if matches!(patch.link_url, Some(None)) {
            link_url = None;
        }
        if matches!(patch.external_key, Some(None)) {
            external_key = None;
        }

        let now = now_ms() as i64;
        let status = patch.status.map(TodoStatus::as_str);
        connection
            .execute(
                "UPDATE todos SET \
                 title = COALESCE(?2, title), \
                 body = COALESCE(?3, body), \
                 status = COALESCE(?4, status), \
                 external_key = ?5, \
                 link_url = ?6, \
                 updated_at_ms = ?7 \
                 WHERE id = ?1",
                params![
                    id,
                    validated_title,
                    patch.body,
                    status,
                    external_key,
                    link_url,
                    now
                ],
            )
            .map_err(storage)?;
        fetch(&connection, id)
    }

    pub fn delete(&self, id: &str) -> Result<(), TodoError> {
        let connection = self.lock();
        let affected = connection
            .execute("DELETE FROM todos WHERE id = ?1", params![id])
            .map_err(storage)?;
        if affected == 0 {
            Err(TodoError::NotFound)
        } else {
            Ok(())
        }
    }

    /// Sets `running`, `run_kind`, `run_ref`, clears `last_error`.
    pub fn mark_running(&self, id: &str, run_kind: &str, run_ref: &str) -> Result<Todo, TodoError> {
        let connection = self.lock();
        let now = now_ms() as i64;
        let affected = connection
            .execute(
                "UPDATE todos \
                 SET status = 'running', run_kind = ?2, run_ref = ?3, last_error = NULL, updated_at_ms = ?4 \
                 WHERE id = ?1",
                params![id, run_kind, run_ref, now],
            )
            .map_err(storage)?;
        if affected == 0 {
            return Err(TodoError::NotFound);
        }
        fetch(&connection, id)
    }

    /// `success` -> `done`; otherwise `open` with `last_error`.
    pub fn finish_run(
        &self,
        id: &str,
        success: bool,
        error: Option<String>,
    ) -> Result<Todo, TodoError> {
        let connection = self.lock();
        let now = now_ms() as i64;
        let (status, last_error) = if success {
            (TodoStatus::Done, None)
        } else {
            (TodoStatus::Open, error)
        };
        let affected = connection
            .execute(
                "UPDATE todos SET status = ?2, last_error = ?3, updated_at_ms = ?4 WHERE id = ?1",
                params![id, status.as_str(), last_error, now],
            )
            .map_err(storage)?;
        if affected == 0 {
            return Err(TodoError::NotFound);
        }
        fetch(&connection, id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::ConversationEngine;

    fn manual(title: &str) -> NewTodo {
        NewTodo {
            title: title.to_owned(),
            body: String::new(),
            source: TodoSource::Manual,
            source_ref: None,
        }
    }

    // TC-540 / FR-540: create, update, reopen, list order, upsert by
    // (source, source_ref), mark_running/finish_run, invalid inputs, and the
    // conversation engine can still open the same DB file afterwards.
    #[test]
    fn todo_store_persists_and_orders() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nested").join("conversations.sqlite3");
        let store = TodoStore::open(&db_path).unwrap();
        assert!(
            db_path.is_file(),
            "store creates the parent dir and DB file"
        );

        let first = store.create(manual("first")).unwrap();
        assert_eq!(first.status, TodoStatus::Open);
        assert_eq!(first.source, TodoSource::Manual);
        assert!(first.body.is_empty());
        assert!(first.source_ref.is_none());

        // A done to-do must sort after open ones regardless of age.
        let done = store.create(manual("done item")).unwrap();
        let done = store
            .update(
                &done.id,
                TodoPatch {
                    status: Some(TodoStatus::Done),
                    ..TodoPatch::default()
                },
            )
            .unwrap();
        assert_eq!(done.status, TodoStatus::Done);

        let running = store.create(manual("running item")).unwrap();
        let running = store.mark_running(&running.id, "agent", "codex").unwrap();
        assert_eq!(running.status, TodoStatus::Running);
        assert_eq!(running.run_kind.as_deref(), Some("agent"));
        assert_eq!(running.run_ref.as_deref(), Some("codex"));

        let ordered = store.list().unwrap();
        let done_pos = ordered.iter().position(|t| t.id == done.id).unwrap();
        let running_pos = ordered.iter().position(|t| t.id == running.id).unwrap();
        let first_pos = ordered.iter().position(|t| t.id == first.id).unwrap();
        assert!(running_pos < done_pos, "running before done");
        assert!(first_pos < done_pos, "open before done");

        // Reopen: rows survive.
        drop(store);
        let store = TodoStore::open(&db_path).unwrap();
        assert_eq!(store.list().unwrap().len(), 3);
        assert_eq!(store.get(&first.id).unwrap().title, "first");

        // Title/body update.
        let edited = store
            .update(
                &first.id,
                TodoPatch {
                    title: Some("first edited".to_owned()),
                    body: Some("hello".to_owned()),
                    status: None,
                    ..TodoPatch::default()
                },
            )
            .unwrap();
        assert_eq!(edited.title, "first edited");
        assert_eq!(edited.body, "hello");
        assert!(edited.updated_at_ms >= edited.created_at_ms);

        // Upsert by (source, source_ref).
        let jira = NewTodo {
            title: "J-1".to_owned(),
            body: "one".to_owned(),
            source: TodoSource::Jira,
            source_ref: Some("JIRA-1".to_owned()),
        };
        let created = store.create(jira.clone()).unwrap();
        let upserted = store
            .create(NewTodo {
                title: "J-1 updated".to_owned(),
                body: "two".to_owned(),
                ..jira
            })
            .unwrap();
        assert_eq!(created.id, upserted.id, "upsert keeps the same row");
        assert_eq!(upserted.title, "J-1 updated");
        assert_eq!(upserted.body, "two");
        assert_eq!(store.list().unwrap().len(), 4);

        // finish_run failure returns to open with last_error.
        let failed = store
            .finish_run(&running.id, false, Some("boom".to_owned()))
            .unwrap();
        assert_eq!(failed.status, TodoStatus::Open);
        assert_eq!(failed.last_error.as_deref(), Some("boom"));

        // finish_run success -> done.
        let finished = store.finish_run(&running.id, true, None).unwrap();
        assert_eq!(finished.status, TodoStatus::Done);
        assert!(finished.last_error.is_none());

        // Invalid inputs.
        assert!(matches!(
            store.create(manual("   ")),
            Err(TodoError::Invalid(_))
        ));
        assert!(matches!(
            store.create(manual(&"x".repeat(MAX_TITLE_CHARS + 1))),
            Err(TodoError::Invalid(_))
        ));
        assert!(matches!(
            store.create(NewTodo {
                title: "ok".to_owned(),
                body: "y".repeat(MAX_BODY_CHARS + 1),
                source: TodoSource::Manual,
                source_ref: None,
            }),
            Err(TodoError::Invalid(_))
        ));
        assert!(matches!(
            store.update(
                &first.id,
                TodoPatch {
                    status: Some(TodoStatus::Running),
                    ..TodoPatch::default()
                }
            ),
            Err(TodoError::Invalid(_))
        ));

        // Unknown ids.
        assert!(matches!(store.get("missing"), Err(TodoError::NotFound)));
        assert!(matches!(
            store.update(
                "missing",
                TodoPatch {
                    title: Some("x".to_owned()),
                    ..TodoPatch::default()
                }
            ),
            Err(TodoError::NotFound)
        ));
        assert!(matches!(
            store.mark_running("missing", "agent", "codex"),
            Err(TodoError::NotFound)
        ));
        assert!(matches!(
            store.finish_run("missing", true, None),
            Err(TodoError::NotFound)
        ));
        assert!(matches!(store.delete("missing"), Err(TodoError::NotFound)));

        // delete removes the row.
        store.delete(&done.id).unwrap();
        assert!(matches!(store.get(&done.id), Err(TodoError::NotFound)));

        // FR-540: the to-do table is additive without touching user_version.
        {
            let connection = store.lock();
            let version: i64 = connection
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap();
            assert_eq!(version, 0, "TodoStore must not touch user_version");
        }

        // A ConversationEngine can still open the same DB file afterwards.
        drop(store);
        let engine = ConversationEngine::open(&db_path, dir.path()).unwrap();
        drop(engine);
    }

    // FR-540: upserting the same (source, source_ref) repeatedly never
    // duplicates or errors, even from two open stores on the same DB file
    // (the TUI + `serve` case).
    #[test]
    fn todo_store_upsert_source_ref_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("todos.db");
        let tui = TodoStore::open(&db_path).unwrap();
        let serve = TodoStore::open(&db_path).unwrap();

        let jira = |title: &str| NewTodo {
            title: title.to_owned(),
            body: format!("body of {title}"),
            source: TodoSource::Jira,
            source_ref: Some("JIRA-1".to_owned()),
        };

        let first = tui.create(jira("first")).unwrap();
        assert_eq!(first.source, TodoSource::Jira);
        let second = serve.create(jira("second")).unwrap();
        let third = tui.create(jira("third")).unwrap();
        assert_eq!(first.id, second.id, "upsert reuses the existing row");
        assert_eq!(second.id, third.id);
        assert_eq!(third.title, "third");
        assert_eq!(third.status, TodoStatus::Open);

        // Upsert after the row is done keeps the status.
        tui.update(
            &first.id,
            TodoPatch {
                status: Some(TodoStatus::Done),
                ..TodoPatch::default()
            },
        )
        .unwrap();
        let done = serve.create(jira("fourth")).unwrap();
        assert_eq!(done.id, first.id);
        assert_eq!(done.title, "fourth");
        assert_eq!(done.status, TodoStatus::Done, "upsert keeps status");

        let all = tui.list().unwrap();
        assert_eq!(all.len(), 1, "no duplicate rows");
        assert_eq!(all[0].body, "body of fourth");
    }

    // TC-570 / FR-563: the link columns are added to an existing slice B DB
    // without a `user_version` bump, values validate, and PATCH null clears.
    #[test]
    fn todo_link_columns_migrate_and_validate() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("legacy.sqlite3");

        // A slice B schema: no external_key/link_url, user_version set so the
        // test can prove the additive migration never bumps it.
        {
            let legacy = rusqlite::Connection::open(&db_path).unwrap();
            legacy
                .execute_batch(
                    "CREATE TABLE todos (
                        id TEXT PRIMARY KEY,
                        title TEXT NOT NULL,
                        body TEXT NOT NULL DEFAULT '',
                        status TEXT NOT NULL CHECK (status IN ('open','running','done')),
                        source TEXT NOT NULL,
                        source_ref TEXT,
                        run_kind TEXT,
                        run_ref TEXT,
                        last_error TEXT,
                        created_at_ms INTEGER NOT NULL,
                        updated_at_ms INTEGER NOT NULL
                    );
                    PRAGMA user_version = 7;",
                )
                .unwrap();
        }

        let store = TodoStore::open(&db_path).unwrap();
        {
            // FR-563: old DBs open and gain the columns.
            let columns = {
                let connection = store.lock();
                let mut statement = connection.prepare("PRAGMA table_info(todos)").unwrap();
                let mut rows = statement.query([]).unwrap();
                let mut names = Vec::new();
                while let Some(row) = rows.next().unwrap() {
                    names.push(row.get::<_, String>("name").unwrap());
                }
                names
            };
            assert!(columns.iter().any(|name| name == "external_key"));
            assert!(columns.iter().any(|name| name == "link_url"));
            assert!(columns.iter().any(|name| name == "external_status"));
            assert!(columns.iter().any(|name| name == "external_assignee"));
            let connection = store.lock();
            let version: i64 = connection
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap();
            assert_eq!(version, 7, "no user_version bump");
        }

        // Valid values round-trip.
        let linked = store
            .create_with_link(
                manual("linked"),
                Some("DEMO-9".to_owned()),
                Some("https://example.invalid/jira/DEMO-9".to_owned()),
            )
            .unwrap();
        assert_eq!(linked.external_key.as_deref(), Some("DEMO-9"));
        assert_eq!(
            linked.link_url.as_deref(),
            Some("https://example.invalid/jira/DEMO-9")
        );
        assert_eq!(store.get(&linked.id).unwrap(), linked, "values persist");

        // Invalid link URLs and keys -> Invalid (400 at the route).
        for bad in [
            "javascript:alert(1)",
            "/relative",
            "https://",
            "ftp://example.invalid",
        ] {
            assert!(
                store
                    .create_with_link(manual("bad link"), None, Some(bad.to_owned()))
                    .is_err(),
                "link_url {bad:?} must be rejected"
            );
        }
        let overlong = format!("https://example.invalid/{}", "a".repeat(MAX_LINK_URL_CHARS));
        assert!(matches!(
            store.create_with_link(manual("long"), None, Some(overlong)),
            Err(TodoError::Invalid(_))
        ));
        for bad in ["123", "PROJ", "PROJ-", "proj 1", "-1"] {
            assert!(
                store
                    .create_with_link(manual("bad key"), Some(bad.to_owned()), None)
                    .is_err(),
                "external_key {bad:?} must be rejected"
            );
        }
        assert!(matches!(
            store.create_with_link(
                manual("long key"),
                Some(format!("PROJ-{}", "1".repeat(MAX_EXTERNAL_KEY_CHARS))),
                None,
            ),
            Err(TodoError::Invalid(_))
        ));

        // FR-563: import-only status/assignee round-trip and truncate on a
        // char boundary; blank becomes `None`.
        let long_status = "s".repeat(MAX_EXTERNAL_STATUS_CHARS + 5);
        let long_assignee = "한".repeat(MAX_EXTERNAL_ASSIGNEE_CHARS + 5);
        let imported = store
            .create_with_external(
                NewTodo {
                    title: "imported".to_owned(),
                    body: String::new(),
                    source: TodoSource::Jira,
                    source_ref: Some("DEMO-7".to_owned()),
                },
                ExternalMeta {
                    external_key: Some("DEMO-7".to_owned()),
                    link_url: None,
                    external_status: Some(long_status),
                    external_assignee: Some(long_assignee),
                },
            )
            .unwrap();
        assert_eq!(
            imported.external_status.as_deref().unwrap().chars().count(),
            MAX_EXTERNAL_STATUS_CHARS
        );
        let assignee = imported.external_assignee.as_deref().unwrap();
        assert_eq!(assignee.chars().count(), MAX_EXTERNAL_ASSIGNEE_CHARS);
        assert!(std::str::from_utf8(assignee.as_bytes()).is_ok());
        assert_eq!(store.get(&imported.id).unwrap(), imported);
        let blank = store
            .create_with_external(
                manual("blank status"),
                ExternalMeta {
                    external_status: Some("   ".to_owned()),
                    external_assignee: Some(String::new()),
                    ..ExternalMeta::default()
                },
            )
            .unwrap();
        assert!(blank.external_status.is_none());
        assert!(blank.external_assignee.is_none());

        // PATCH: a browse URL with no key sets the key, null clears the link.
        let patched = store
            .update(
                &linked.id,
                TodoPatch {
                    link_url: Some(Some(
                        "https://example.atlassian.net/browse/DEMO-9".to_owned(),
                    )),
                    ..TodoPatch::default()
                },
            )
            .unwrap();
        assert_eq!(patched.external_key.as_deref(), Some("DEMO-9"));
        let cleared = store
            .update(
                &linked.id,
                TodoPatch {
                    link_url: Some(None),
                    ..TodoPatch::default()
                },
            )
            .unwrap();
        assert!(cleared.link_url.is_none(), "null clears the link");

        // An explicit `null` clears the key even when the link is a browse URL.
        store
            .update(
                &linked.id,
                TodoPatch {
                    link_url: Some(Some(
                        "https://example.atlassian.net/browse/DEMO-9".to_owned(),
                    )),
                    ..TodoPatch::default()
                },
            )
            .unwrap();
        let key_cleared = store
            .update(
                &linked.id,
                TodoPatch {
                    external_key: Some(None),
                    ..TodoPatch::default()
                },
            )
            .unwrap();
        assert!(key_cleared.external_key.is_none(), "null clears the key");
    }

    // TC-571 / FR-564: base-URL derivation and browse-URL key extraction.
    #[test]
    fn todo_jira_key_and_url_derivation() {
        let base = "https://example.atlassian.net";

        // Pure derivation.
        assert_eq!(
            derive_jira_link(Some("PROJ-1"), None, Some(base)),
            (
                Some("PROJ-1".to_owned()),
                Some("https://example.atlassian.net/browse/PROJ-1".to_owned())
            )
        );
        assert_eq!(
            derive_jira_link(
                None,
                Some("https://example.atlassian.net/browse/PROJ-2"),
                Some(base)
            ),
            (
                Some("PROJ-2".to_owned()),
                Some("https://example.atlassian.net/browse/PROJ-2".to_owned())
            )
        );
        assert_eq!(
            derive_jira_link(Some("PROJ-3"), None, None),
            (Some("PROJ-3".to_owned()), None),
            "no base URL -> key without a link"
        );
        assert_eq!(
            derive_jira_link(
                Some("PROJ-4"),
                Some("https://example.invalid/x/PROJ-4"),
                Some(base)
            ),
            (
                Some("PROJ-4".to_owned()),
                Some("https://example.invalid/x/PROJ-4".to_owned())
            ),
            "an existing link is never overwritten"
        );
        assert_eq!(
            derive_jira_link(
                None,
                Some("https://example.invalid/not-a-browse-url"),
                Some(base)
            ),
            (
                None,
                Some("https://example.invalid/not-a-browse-url".to_owned())
            )
        );
        // A trailing slash on the base is tolerated.
        assert_eq!(
            derive_jira_link(Some("PROJ-5"), None, Some("https://example.atlassian.net/")),
            (
                Some("PROJ-5".to_owned()),
                Some("https://example.atlassian.net/browse/PROJ-5".to_owned())
            )
        );

        // Store-level derivation via `set_jira_base_url`.
        let dir = tempfile::tempdir().unwrap();
        let store = TodoStore::open(&dir.path().join("todos.db")).unwrap();
        store.set_jira_base_url(Some(base.to_owned()));
        let keyed = store
            .create_with_link(manual("key only"), Some("PROJ-6".to_owned()), None)
            .unwrap();
        assert_eq!(
            keyed.link_url.as_deref(),
            Some("https://example.atlassian.net/browse/PROJ-6")
        );

        // A browse link on create fills the key.
        let from_link = store
            .create_with_link(
                manual("link only"),
                None,
                Some("https://example.atlassian.net/browse/PROJ-7".to_owned()),
            )
            .unwrap();
        assert_eq!(from_link.external_key.as_deref(), Some("PROJ-7"));

        // No base URL -> the key is stored without a link.
        store.set_jira_base_url(None);
        let no_base = store
            .create_with_link(manual("no base"), Some("PROJ-8".to_owned()), None)
            .unwrap();
        assert_eq!(no_base.external_key.as_deref(), Some("PROJ-8"));
        assert!(no_base.link_url.is_none());
    }
}
