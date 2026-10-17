//! Per-repo to-do store (SPEC-web-dashboard FR-540). The store keeps its own
//! `rusqlite::Connection` to the per-repo DB file behind a `Mutex`.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_BODY_CHARS: usize = 20_000;

const CREATE_SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS todos (
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
CREATE UNIQUE INDEX IF NOT EXISTS todos_by_source_ref
    ON todos(source, source_ref) WHERE source_ref IS NOT NULL;";

const LIST_SQL: &str = "SELECT id, title, body, status, source, source_ref, \
     run_kind, run_ref, last_error, created_at_ms, updated_at_ms \
     FROM todos \
     ORDER BY CASE status \
         WHEN 'open' THEN 0 \
         WHEN 'running' THEN 1 \
         WHEN 'done' THEN 2 \
         ELSE 3 END, \
       created_at_ms ASC, id ASC";

const GET_SQL: &str = "SELECT id, title, body, status, source, source_ref, \
     run_kind, run_ref, last_error, created_at_ms, updated_at_ms \
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub status: Option<TodoStatus>,
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
        Ok(Self {
            connection: Mutex::new(connection),
        })
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
        let title = validate_title(&new.title)?;
        validate_body(&new.body)?;
        let connection = self.lock();
        let now = now_ms() as i64;
        let id: String = connection
            .query_row(
                "INSERT INTO todos \
                 (id, title, body, status, source, source_ref, run_kind, run_ref, last_error, created_at_ms, updated_at_ms) \
                 VALUES (?1, ?2, ?3, 'open', ?4, ?5, NULL, NULL, NULL, ?6, ?6) \
                 ON CONFLICT(source, source_ref) WHERE source_ref IS NOT NULL \
                 DO UPDATE SET title = excluded.title, body = excluded.body, updated_at_ms = excluded.updated_at_ms \
                 RETURNING id",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    title,
                    new.body,
                    new.source.as_str(),
                    new.source_ref,
                    now
                ],
                |row| row.get(0),
            )
            .map_err(storage)?;
        fetch(&connection, &id)
    }

    /// Validates; `status: Some(Running)` -> `Invalid`. Title, body and status
    /// are applied in one atomic `UPDATE` via `COALESCE`, so readers never see a
    /// half-applied patch.
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
        fetch(&connection, id)?;
        if validated_title.is_none() && patch.body.is_none() && patch.status.is_none() {
            return fetch(&connection, id);
        }
        let now = now_ms() as i64;
        let status = patch.status.map(TodoStatus::as_str);
        connection
            .execute(
                "UPDATE todos SET \
                 title = COALESCE(?2, title), \
                 body = COALESCE(?3, body), \
                 status = COALESCE(?4, status), \
                 updated_at_ms = ?5 \
                 WHERE id = ?1",
                params![id, validated_title, patch.body, status, now],
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
}
