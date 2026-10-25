//! Durable, provider-neutral conversations backed by SQLite.
//!
//! `ConversationEngine` is the module's interface. It owns identifiers,
//! transactions, recovery, provider bindings, and context projection so callers
//! do not need to coordinate those invariants themselves.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rusqlite::{
    Connection, OptionalExtension, Transaction, TransactionBehavior, params, types::Type,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::activity::{ActivityItem, ToolStatus, collapsed_line, merge_activity};

const SCHEMA_VERSION: i64 = 3;
pub const MAX_ACTIVITIES_PER_TURN: usize = 400;
pub const MAX_ACTIVITY_TEXT_CHARS: usize = 16_000;
const MAX_CONTEXT_CHARS: usize = 32_000;
const RECENT_TAIL_CHARS: usize = 12_000;
const MAX_SUMMARY_CHARS: usize = MAX_CONTEXT_CHARS - RECENT_TAIL_CHARS;
const SUMMARY_PROMPT_VERSION: &str = "conversation-summary-v1";
const SESSION_LEASE_MS: i64 = 30_000;

/// Total time the open phase waits for a concurrent DmuxAI process to release
/// the conversation database before it reports the database as busy (FR-2120).
const OPEN_RETRY_BUDGET: Duration = Duration::from_secs(20);
/// First backoff step between two open attempts (FR-2120).
const OPEN_RETRY_INITIAL_DELAY: Duration = Duration::from_millis(50);
/// Longest backoff step between two open attempts (FR-2120).
const OPEN_RETRY_MAX_DELAY: Duration = Duration::from_secs(1);
/// Busy timeout of a single open attempt (FR-2120). The retry loop owns the
/// waiting, so one attempt must not block for long.
const OPEN_ATTEMPT_BUSY_TIMEOUT: Duration = Duration::from_millis(250);
/// Busy timeout of the connection once the database is open.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Total time a save waits for a concurrent DmuxAI process to release the
/// conversation database before the save is reported as busy (FR-2140). It
/// matches the order of the open-phase budget.
pub const SAVE_RETRY_BUDGET: Duration = Duration::from_secs(20);
/// Busy timeout of a single save attempt (FR-2140). The retry loop owns the
/// waiting, so one attempt must not block for long.
const SAVE_ATTEMPT_BUSY_TIMEOUT: Duration = Duration::from_millis(250);
/// Message shown when the conversation database stays locked for the whole
/// retry budget (FR-2120).
const BUSY_MESSAGE: &str = "another dmuxai process is using this workspace's conversation \
                            database (database is locked); retry when it finishes";

const MIGRATION_1: &str = r#"
CREATE TABLE IF NOT EXISTS runtime_sessions (
    id              TEXT PRIMARY KEY,
    workspace_root  TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('active', 'closed')),
    created_at_ms   INTEGER NOT NULL,
    last_seen_at_ms INTEGER NOT NULL,
    closed_at_ms    INTEGER
);

CREATE INDEX IF NOT EXISTS runtime_sessions_by_workspace_status
    ON runtime_sessions(workspace_root, status, last_seen_at_ms);

CREATE TABLE IF NOT EXISTS conversations (
    id              TEXT PRIMARY KEY,
    workspace_root  TEXT NOT NULL,
    title           TEXT NOT NULL,
    created_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL,
    archived_at_ms  INTEGER
);

CREATE INDEX IF NOT EXISTS conversations_by_workspace_activity
    ON conversations(workspace_root, archived_at_ms, updated_at_ms DESC);

CREATE TABLE IF NOT EXISTS turns (
    id                  TEXT PRIMARY KEY,
    conversation_id     TEXT NOT NULL
        REFERENCES conversations(id) ON DELETE CASCADE,
    seq                 INTEGER NOT NULL,
    requested_agent     TEXT NOT NULL
        CHECK (requested_agent IN ('codex', 'cursor', 'agy')),
    model_id            TEXT,
    status              TEXT NOT NULL
        CHECK (status IN (
            'queued', 'running', 'completed',
            'cancelled', 'failed', 'interrupted'
        )),
    error_text          TEXT,
    context_through_seq INTEGER NOT NULL DEFAULT 0,
    created_at_ms       INTEGER NOT NULL,
    started_at_ms       INTEGER,
    completed_at_ms     INTEGER,
    owner_session_id    TEXT REFERENCES runtime_sessions(id),
    UNIQUE (conversation_id, seq)
);

CREATE INDEX IF NOT EXISTS turns_by_conversation_seq
    ON turns(conversation_id, seq);

CREATE INDEX IF NOT EXISTS turns_by_owner_status
    ON turns(owner_session_id, status);

CREATE TABLE IF NOT EXISTS messages (
    id              TEXT PRIMARY KEY,
    turn_id         TEXT NOT NULL
        REFERENCES turns(id) ON DELETE CASCADE,
    ordinal         INTEGER NOT NULL CHECK (ordinal IN (0, 1)),
    role            TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    text            TEXT NOT NULL DEFAULT '',
    state           TEXT NOT NULL
        CHECK (state IN ('complete', 'streaming', 'interrupted')),
    created_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL,
    UNIQUE (turn_id, ordinal),
    UNIQUE (turn_id, role)
);

CREATE TABLE IF NOT EXISTS provider_bindings (
    conversation_id      TEXT NOT NULL
        REFERENCES conversations(id) ON DELETE CASCADE,
    agent                TEXT NOT NULL
        CHECK (agent IN ('codex', 'cursor', 'agy')),
    provider_session_id  TEXT NOT NULL,
    status               TEXT NOT NULL CHECK (status IN ('active', 'stale')),
    known_through_seq    INTEGER NOT NULL DEFAULT 0,
    bound_by_turn_id     TEXT REFERENCES turns(id),
    created_at_ms        INTEGER NOT NULL,
    updated_at_ms        INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, agent),
    UNIQUE (agent, provider_session_id)
);

CREATE TABLE IF NOT EXISTS context_checkpoints (
    conversation_id    TEXT NOT NULL
        REFERENCES conversations(id) ON DELETE CASCADE,
    agent              TEXT NOT NULL
        CHECK (agent IN ('codex', 'cursor', 'agy')),
    covers_through_seq INTEGER NOT NULL,
    source_hash        BLOB NOT NULL CHECK (length(source_hash) = 32),
    prompt_version     TEXT NOT NULL,
    summary_json       TEXT NOT NULL,
    created_at_ms      INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, agent, covers_through_seq)
);

CREATE INDEX IF NOT EXISTS checkpoints_latest
    ON context_checkpoints(
        conversation_id,
        agent,
        covers_through_seq DESC
    );

PRAGMA user_version = 1;
"#;

/// v2: drop the built-in-only agent CHECK constraints so `[[agents.custom]]`
/// agents can own turns, bindings and checkpoints. SQLite cannot alter a
/// CHECK, so each affected table is rebuilt from its own stored definition.
fn migrate_to_v2(connection: &mut Connection) -> Result<(), ConversationError> {
    connection.pragma_update(None, "foreign_keys", "OFF")?;
    let db_path = connection
        .path()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("<unknown>"));
    let result = (|| -> Result<(), ConversationError> {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (table, column) in [
            ("turns", "requested_agent"),
            ("provider_bindings", "agent"),
            ("context_checkpoints", "agent"),
        ] {
            let sql: Option<String> = transaction
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(sql) = sql else { continue };
            let old_check = format!("CHECK ({column} IN ('codex', 'cursor', 'agy'))");
            if !sql.contains(&old_check) {
                continue;
            }
            let indexes = {
                let mut statement = transaction.prepare(
                    "SELECT sql FROM sqlite_master
                      WHERE type = 'index' AND tbl_name = ?1 AND sql IS NOT NULL",
                )?;
                statement
                    .query_map([table], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            let new_table = format!("{table}_v2");
            let create = sql
                .replacen(&old_check, &format!("CHECK (length({column}) > 0)"), 1)
                .replacen(
                    &format!("CREATE TABLE {table}"),
                    &format!("CREATE TABLE {new_table}"),
                    1,
                );
            transaction.execute_batch(&create)?;
            transaction.execute_batch(&format!(
                "INSERT INTO {new_table} SELECT * FROM {table};
                 DROP TABLE {table};
                 ALTER TABLE {new_table} RENAME TO {table};"
            ))?;
            for index in indexes {
                transaction.execute_batch(&index)?;
            }
        }
        transaction.execute_batch("PRAGMA user_version = 2;")?;
        let violations: i64 =
            transaction.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        if violations != 0 {
            return Err(ConversationError::UnsupportedSchema {
                version: -1,
                db_path,
            });
        }
        transaction.commit()?;
        Ok(())
    })();
    connection.pragma_update(None, "foreign_keys", "ON")?;
    result
}

/// Reports whether an object (table or index) exists in `sqlite_master`.
fn sqlite_object_kind(
    connection: &Connection,
    name: &str,
) -> Result<Option<String>, ConversationError> {
    let kind: Option<String> = connection
        .query_row(
            "SELECT type FROM sqlite_master WHERE name = ?1",
            [name],
            |row| row.get(0),
        )
        .optional()?;
    Ok(kind)
}

/// Reports whether a table has the given column.
fn table_has_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, ConversationError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(columns.iter().any(|name| name == column))
}

/// Read-only check for the runtime-session objects (FR-2120).
///
/// When everything is already in place the caller can skip the write
/// transaction, so opening a current database never blocks a concurrent
/// DmuxAI process.
fn runtime_session_schema_is_current(connection: &Connection) -> Result<bool, ConversationError> {
    if sqlite_object_kind(connection, "runtime_sessions")?.as_deref() != Some("table") {
        return Ok(false);
    }
    if sqlite_object_kind(connection, "runtime_sessions_by_workspace_status")?.as_deref()
        != Some("index")
    {
        return Ok(false);
    }
    if sqlite_object_kind(connection, "turns")?.as_deref() == Some("table") {
        if !table_has_column(connection, "turns", "owner_session_id")? {
            return Ok(false);
        }
        if sqlite_object_kind(connection, "turns_by_owner_status")?.as_deref() != Some("index") {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Installs the runtime-session objects for databases created by the original
/// v1 migration. The extension intentionally does not bump `user_version`:
/// older binaries can ignore the additive table and nullable column during a
/// rollback. Every change is committed together or not at all.
///
/// The schema is inspected read-only first (FR-2120): a database that already
/// has every object needs no write transaction, so a concurrent process is not
/// blocked. Only a database that is missing something takes the immediate
/// transaction, where the `IF NOT EXISTS` clauses and the column re-check keep
/// a racing process harmless.
fn ensure_runtime_session_schema(connection: &mut Connection) -> Result<(), ConversationError> {
    if runtime_session_schema_is_current(connection)? {
        return Ok(());
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS runtime_sessions (
            id              TEXT PRIMARY KEY,
            workspace_root  TEXT NOT NULL,
            status          TEXT NOT NULL CHECK (status IN ('active', 'closed')),
            created_at_ms   INTEGER NOT NULL,
            last_seen_at_ms INTEGER NOT NULL,
            closed_at_ms    INTEGER
        );

        CREATE INDEX IF NOT EXISTS runtime_sessions_by_workspace_status
            ON runtime_sessions(workspace_root, status, last_seen_at_ms);
        "#,
    )?;

    let turns_kind: Option<String> = transaction
        .query_row(
            "SELECT type FROM sqlite_master WHERE name = 'turns'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if turns_kind.as_deref() == Some("table") {
        let has_owner_column = {
            let mut statement = transaction.prepare("PRAGMA table_info(turns)")?;
            statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .any(|name| name == "owner_session_id")
        };
        if !has_owner_column {
            transaction.execute_batch(
                "ALTER TABLE turns ADD COLUMN owner_session_id TEXT REFERENCES runtime_sessions(id);",
            )?;
        }
        transaction.execute_batch(
            "CREATE INDEX IF NOT EXISTS turns_by_owner_status
                ON turns(owner_session_id, status);",
        )?;
    }

    transaction.commit()?;
    Ok(())
}

/// v3: add optional display_text to messages and create activities table.
fn migrate_to_v3(connection: &mut Connection) -> Result<(), ConversationError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let has_display_text_column = {
        let mut statement = transaction.prepare("PRAGMA table_info(messages)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .any(|name| name == "display_text")
    };
    if !has_display_text_column {
        transaction.execute_batch("ALTER TABLE messages ADD COLUMN display_text TEXT;")?;
    }
    transaction.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS activities (
            turn_id         TEXT NOT NULL REFERENCES turns(id) ON DELETE CASCADE,
            ordinal         INTEGER NOT NULL CHECK (ordinal >= 0),
            kind            TEXT NOT NULL CHECK (kind IN ('tool', 'thinking')),
            summary         TEXT NOT NULL DEFAULT '',
            state           TEXT NOT NULL CHECK (state IN ('running', 'ok', 'failed', 'live', 'done')),
            payload_json    TEXT NOT NULL,
            created_at_ms   INTEGER NOT NULL,
            updated_at_ms   INTEGER NOT NULL,
            PRIMARY KEY (turn_id, ordinal)
        );
        PRAGMA user_version = 3;
        "#,
    )?;
    transaction.commit()?;
    Ok(())
}

#[derive(Debug)]
pub enum ConversationError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    Json(serde_json::Error),
    /// The conversation database stayed locked for the whole retry budget
    /// (FR-2120). The message is actionable for the caller.
    Busy(String),
    NoActiveConversation,
    ConversationNotFound(ConversationId),
    AgentBusy(AgentKey),
    InvalidOutput(String),
    StaleSummary,
    SummaryTooLarge {
        actual_chars: usize,
        maximum_chars: usize,
    },
    UnsupportedSchema {
        version: i64,
        db_path: PathBuf,
    },
    ClockBeforeUnixEpoch,
}

impl fmt::Display for ConversationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "conversation database error: {error}"),
            Self::Io(error) => write!(formatter, "conversation I/O error: {error}"),
            Self::Json(error) => write!(formatter, "invalid conversation JSON: {error}"),
            Self::Busy(message) => formatter.write_str(message),
            Self::NoActiveConversation => formatter.write_str("no conversation is active"),
            Self::ConversationNotFound(id) => write!(formatter, "conversation {id} was not found"),
            Self::AgentBusy(agent) => write!(formatter, "{agent} already has an active turn"),
            Self::InvalidOutput(message) => write!(formatter, "invalid agent output: {message}"),
            Self::StaleSummary => formatter.write_str("summary source changed before commit"),
            Self::SummaryTooLarge {
                actual_chars,
                maximum_chars,
            } => write!(
                formatter,
                "summary is {actual_chars} characters; maximum is {maximum_chars}"
            ),
            Self::UnsupportedSchema { version, db_path } => write!(
                formatter,
                "conversation database {} has schema {version}, which was written by a newer \
                 dmuxai; point DMUXAI_STATE_DIR at a directory this dmuxai wrote",
                db_path.display()
            ),
            Self::ClockBeforeUnixEpoch => {
                formatter.write_str("system clock is before the Unix epoch")
            }
        }
    }
}

impl Error for ConversationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for ConversationError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<std::io::Error> for ConversationError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ConversationError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// True for SQLite's "database is locked" and "database table is locked"
/// errors (FR-2120).
fn is_busy(error: &rusqlite::Error) -> bool {
    match error {
        rusqlite::Error::SqliteFailure(inner, _) => matches!(
            inner.code,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
        ),
        _ => false,
    }
}

/// True when a conversation error is a busy database (FR-2120).
fn is_busy_error(error: &ConversationError) -> bool {
    match error {
        ConversationError::Sqlite(inner) => is_busy(inner),
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConversationId(String);

impl ConversationId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ConversationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TurnId(String);

impl TurnId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TurnId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentKey {
    Codex,
    Cursor,
    Agy,
    /// A `[[agents.custom]]` agent by registry index; `u8::MAX` = no longer configured.
    Custom(u8),
}

/// Registry index used for stored turns of a custom agent that is no longer configured.
pub const UNKNOWN_CUSTOM_AGENT: u8 = u8::MAX;

impl AgentKey {
    fn as_db(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Agy => "agy",
            Self::Custom(index) => crate::custom_agent::spec_at(index)
                .map(|spec| spec.name.as_str())
                .unwrap_or("custom-unknown"),
        }
    }

    fn from_db(value: &str) -> rusqlite::Result<Self> {
        match value {
            "codex" => Ok(Self::Codex),
            "cursor" => Ok(Self::Cursor),
            "agy" => Ok(Self::Agy),
            "" => Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                Type::Text,
                "empty agent name".into(),
            )),
            other => Ok(Self::Custom(
                crate::custom_agent::index_of(other).unwrap_or(UNKNOWN_CUSTOM_AGENT),
            )),
        }
    }
}

impl Serialize for AgentKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_db())
    }
}

impl<'de> Deserialize<'de> for AgentKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::from_db(&value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for AgentKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::Agy => "AGY",
            Self::Custom(index) => crate::custom_agent::spec_at(*index)
                .map(|spec| spec.label())
                .unwrap_or("custom"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnStatus {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
    Interrupted,
}

impl TurnStatus {
    fn from_db(value: &str) -> rusqlite::Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                Type::Text,
                format!("unknown turn status {other}").into(),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
}

impl MessageRole {
    fn from_db(value: &str) -> rusqlite::Result<Self> {
        match value {
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                Type::Text,
                format!("unknown message role {other}").into(),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageState {
    Complete,
    Streaming,
    Interrupted,
}

impl MessageState {
    fn from_db(value: &str) -> rusqlite::Result<Self> {
        match value {
            "complete" => Ok(Self::Complete),
            "streaming" => Ok(Self::Streaming),
            "interrupted" => Ok(Self::Interrupted),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                Type::Text,
                format!("unknown message state {other}").into(),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BindingStatus {
    Active,
    Stale,
}

impl BindingStatus {
    fn from_db(value: &str) -> rusqlite::Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "stale" => Ok(Self::Stale),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                Type::Text,
                format!("unknown binding status {other}").into(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderBinding {
    pub agent: AgentKey,
    pub provider_session_id: String,
    pub status: BindingStatus,
    pub known_through_seq: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationCard {
    pub id: ConversationId,
    pub title: String,
    pub updated_at_ms: i64,
    pub turn_count: u64,
    pub last_agent: Option<AgentKey>,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptMessage {
    pub id: String,
    pub turn_id: TurnId,
    pub turn_seq: i64,
    pub agent: AgentKey,
    pub role: MessageRole,
    pub text: String,
    pub display_text: Option<String>,
    pub state: MessageState,
}

impl TranscriptMessage {
    pub fn shown_text(&self) -> &str {
        self.display_text.as_deref().unwrap_or(&self.text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnActivity {
    pub turn_id: TurnId,
    pub ordinal: i64,
    pub item: ActivityItem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSnapshot {
    pub id: ConversationId,
    pub title: String,
    pub turns: Vec<TurnSnapshot>,
    pub messages: Vec<TranscriptMessage>,
    pub bindings: Vec<ProviderBinding>,
    pub activities: Vec<TurnActivity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSnapshot {
    pub id: TurnId,
    pub seq: i64,
    pub agent: AgentKey,
    pub model_id: Option<String>,
    pub status: TurnStatus,
    pub error_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationSelection {
    New { title: Option<String> },
    Existing(ConversationId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginTurn {
    pub agent: AgentKey,
    pub model_id: Option<String>,
    pub prompt: String,
    pub display_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRequest {
    pub conversation_id: ConversationId,
    pub turn_id: TurnId,
    pub agent: AgentKey,
    pub model_id: Option<String>,
    pub prompt: String,
    pub resume_session: Option<String>,
    pub context: ContextProjection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeginTurnOutcome {
    Ready(AgentRequest),
    NeedsSummary(SummaryJob),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectedTurn {
    pub seq: i64,
    pub agent: AgentKey,
    pub user: String,
    pub assistant: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextProjection {
    pub summary: Option<String>,
    pub recent: Vec<ProjectedTurn>,
    pub through_seq: i64,
    pub source_hash: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryJob {
    conversation_id: ConversationId,
    agent: AgentKey,
    covers_through_seq: i64,
    source_hash: [u8; 32],
    previous_summary: Option<SummaryDocument>,
    source_turns: Vec<ProjectedTurn>,
}

impl SummaryJob {
    pub fn agent(&self) -> AgentKey {
        self.agent
    }

    pub fn covers_through_seq(&self) -> i64 {
        self.covers_through_seq
    }

    pub fn previous_summary(&self) -> Option<&SummaryDocument> {
        self.previous_summary.as_ref()
    }

    pub fn source_turns(&self) -> &[ProjectedTurn] {
        &self.source_turns
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryDocument {
    pub goal: Option<String>,
    pub constraints: Vec<String>,
    pub decisions: Vec<String>,
    pub facts: Vec<String>,
    pub files: Vec<String>,
    pub failed_attempts: Vec<String>,
    pub open_questions: Vec<String>,
}

impl SummaryDocument {
    fn render(&self) -> String {
        let mut sections = Vec::new();
        if let Some(goal) = &self.goal {
            sections.push(format!("Goal:\n{goal}"));
        }
        push_summary_section(&mut sections, "Constraints", &self.constraints);
        push_summary_section(&mut sections, "Decisions", &self.decisions);
        push_summary_section(&mut sections, "Facts", &self.facts);
        push_summary_section(&mut sections, "Files", &self.files);
        push_summary_section(&mut sections, "Failed attempts", &self.failed_attempts);
        push_summary_section(&mut sections, "Open questions", &self.open_questions);
        sections.join("\n\n")
    }
}

fn push_summary_section(sections: &mut Vec<String>, title: &str, values: &[String]) {
    if !values.is_empty() {
        let body = values
            .iter()
            .map(|value| format!("- {value}"))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("{title}:\n{body}"));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOutput {
    pub conversation_id: ConversationId,
    pub turn_id: TurnId,
    pub agent: AgentKey,
    pub event: TurnEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEvent {
    Started,
    ProviderSessionBound(String),
    TextDelta(String),
    Notice(String),
    Completed,
    Cancelled,
    Failed {
        message: String,
        invalidates_binding: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptPatch {
    TurnStarted {
        turn_id: TurnId,
        agent: AgentKey,
    },
    ProviderBound {
        agent: AgentKey,
        provider_session_id: String,
    },
    AssistantDelta {
        turn_id: TurnId,
        agent: AgentKey,
        delta: String,
    },
    Notice {
        turn_id: TurnId,
        agent: AgentKey,
        text: String,
    },
    TurnFinished {
        turn_id: TurnId,
        agent: AgentKey,
        status: TurnStatus,
    },
}

pub struct ConversationEngine {
    connection: Connection,
    session_id: String,
    session_closed: bool,
    workspace_root: String,
    active_conversation: Option<ConversationId>,
}

/// Opens the conversation database, waiting for a concurrent DmuxAI process to
/// release it (FR-2120).
///
/// The open phase is retried while the database reports SQLITE_BUSY or
/// SQLITE_LOCKED, with a bounded backoff. When the database stays locked for
/// the whole budget the error is `ConversationError::Busy`.
fn open_with_retry(
    db_path: &Path,
    workspace_root: &Path,
    budget: Duration,
) -> Result<ConversationEngine, ConversationError> {
    if let Some(parent) = db_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let workspace_root = normalize_workspace(workspace_root)?;

    with_busy_retry(budget, || open_once(db_path, &workspace_root))
}

/// Runs `attempt` while the conversation database reports SQLITE_BUSY or
/// SQLITE_LOCKED, with a bounded backoff (FR-2120, FR-2140).
///
/// The retry stops when the attempt succeeds, when the error is not a busy
/// database, or when the budget is exhausted. An exhausted budget is reported
/// as `ConversationError::Busy` so the caller can map it to a conflict.
fn with_busy_retry<T>(
    budget: Duration,
    mut attempt: impl FnMut() -> Result<T, ConversationError>,
) -> Result<T, ConversationError> {
    let started = Instant::now();
    let mut delay = OPEN_RETRY_INITIAL_DELAY;
    loop {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(error) => {
                if !is_busy_error(&error) {
                    return Err(error);
                }
                if started.elapsed().saturating_add(delay) >= budget {
                    return Err(ConversationError::Busy(BUSY_MESSAGE.to_owned()));
                }
                std::thread::sleep(delay + retry_jitter(delay));
                delay = (delay * 2).min(OPEN_RETRY_MAX_DELAY);
            }
        }
    }
}

/// Small random padding so two processes do not retry in lockstep.
fn retry_jitter(delay: Duration) -> Duration {
    let bytes = Uuid::new_v4().into_bytes();
    let fraction = u32::from(bytes[0]) % 5;
    delay / 4 * fraction
}

/// Number of save attempts the busy-retry loop made (FR-2140, test-only).
#[cfg(test)]
static SAVE_ATTEMPTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Counts one save attempt of the busy-retry loop (FR-2140, test-only).
#[cfg(test)]
fn count_save_attempt() {
    SAVE_ATTEMPTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Save attempts the busy-retry loop made since the process started
/// (FR-2140, test-only).
#[cfg(test)]
fn save_attempts() -> usize {
    SAVE_ATTEMPTS.load(std::sync::atomic::Ordering::SeqCst)
}

/// One open attempt: pragmas, migrations and the runtime-session insert.
fn open_once(
    db_path: &Path,
    workspace_root: &str,
) -> Result<ConversationEngine, ConversationError> {
    let mut connection = Connection::open(db_path)?;
    connection.busy_timeout(OPEN_ATTEMPT_BUSY_TIMEOUT)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    // Switching the journal mode needs a brief exclusive lock, and SQLite does
    // not run the busy handler for it: a database that is already in WAL mode
    // is left alone (FR-2120).
    let journal_mode: String =
        connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        connection.pragma_update(None, "journal_mode", "WAL")?;
    }
    connection.pragma_update(None, "synchronous", "NORMAL")?;

    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(ConversationError::UnsupportedSchema {
            version,
            db_path: db_path.to_path_buf(),
        });
    }
    if version < 1 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(MIGRATION_1)?;
        transaction.commit()?;
    }

    ensure_runtime_session_schema(&mut connection)?;
    if version < 2 {
        migrate_to_v2(&mut connection)?;
    }
    if version < 3 {
        migrate_to_v3(&mut connection)?;
    }
    let session_id = Uuid::new_v4().to_string();
    let now = now_ms()?;
    connection.execute(
        "INSERT INTO runtime_sessions
            (id, workspace_root, status, created_at_ms, last_seen_at_ms)
         VALUES (?1, ?2, 'active', ?3, ?3)",
        params![session_id, workspace_root, now],
    )?;
    connection.busy_timeout(BUSY_TIMEOUT)?;

    Ok(ConversationEngine {
        connection,
        session_id,
        session_closed: false,
        workspace_root: workspace_root.to_owned(),
        active_conversation: None,
    })
}

impl ConversationEngine {
    pub fn open(db_path: &Path, workspace_root: &Path) -> Result<Self, ConversationError> {
        open_with_retry(db_path, workspace_root, OPEN_RETRY_BUDGET)
    }

    /// Returns whether another non-expired DmuxAI process is using this
    /// workspace. The caller uses this only for startup conversation choice;
    /// it is not a process-wide singleton lock.
    pub fn has_live_peer(&self) -> Result<bool, ConversationError> {
        let now = now_ms()?;
        self.connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM runtime_sessions
                     WHERE workspace_root = ?1
                       AND id <> ?2
                       AND status = 'active'
                       AND last_seen_at_ms >= ?3
                 )",
                params![
                    self.workspace_root,
                    self.session_id,
                    now.saturating_sub(SESSION_LEASE_MS)
                ],
                |row| row.get(0),
            )
            .map_err(ConversationError::from)
    }

    /// Refreshes this process's lease. The TUI calls this periodically while
    /// its event loop is alive so a later process can distinguish a live peer
    /// from a crashed one.
    pub fn touch_session(&mut self) -> Result<(), ConversationError> {
        if self.session_closed {
            return Ok(());
        }
        self.connection.execute(
            "UPDATE runtime_sessions
                SET last_seen_at_ms = ?2
              WHERE id = ?1 AND status = 'active'",
            params![self.session_id, now_ms()?],
        )?;
        Ok(())
    }

    /// Marks this process closed so a later startup may recover any turn it
    /// left unfinished. Drop calls this best-effort as a final fallback.
    pub fn close_session(&mut self) -> Result<(), ConversationError> {
        if self.session_closed {
            return Ok(());
        }
        let now = now_ms()?;
        self.connection.execute(
            "UPDATE runtime_sessions
                SET status = 'closed', closed_at_ms = ?2, last_seen_at_ms = ?2
              WHERE id = ?1 AND status = 'active'",
            params![self.session_id, now],
        )?;
        self.session_closed = true;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<ConversationCard>, ConversationError> {
        let mut statement = self.connection.prepare(
            "SELECT c.id, c.title, c.updated_at_ms,
                    COUNT(t.id),
                    (SELECT requested_agent
                       FROM turns latest
                      WHERE latest.conversation_id = c.id
                      ORDER BY latest.seq DESC
                      LIMIT 1),
                    EXISTS(
                        SELECT 1
                          FROM turns active
                         WHERE active.conversation_id = c.id
                           AND active.status IN ('queued', 'running')
                    )
               FROM conversations c
               LEFT JOIN turns t ON t.conversation_id = c.id
              WHERE c.workspace_root = ?1 AND c.archived_at_ms IS NULL
              GROUP BY c.id
              ORDER BY c.updated_at_ms DESC, c.id DESC",
        )?;
        let rows = statement.query_map([&self.workspace_root], |row| {
            let last_agent: Option<String> = row.get(4)?;
            Ok(ConversationCard {
                id: ConversationId::new(row.get::<_, String>(0)?),
                title: row.get(1)?,
                updated_at_ms: row.get(2)?,
                turn_count: row.get::<_, i64>(3)?.try_into().map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(3, Type::Integer, Box::new(error))
                })?,
                last_agent: last_agent.as_deref().map(AgentKey::from_db).transpose()?,
                running: row.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(ConversationError::from)
    }

    /// The normalized workspace key this engine lists and writes under.
    pub fn workspace_key(&self) -> &str {
        &self.workspace_root
    }

    pub fn select(
        &mut self,
        selection: ConversationSelection,
    ) -> Result<ConversationSnapshot, ConversationError> {
        let conversation_id = match selection {
            ConversationSelection::New { title } => {
                let now = now_ms()?;
                let id = ConversationId::new(Uuid::new_v4().to_string());
                self.connection.execute(
                    "INSERT INTO conversations
                        (id, workspace_root, title, created_at_ms, updated_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                    params![
                        id.as_str(),
                        self.workspace_root,
                        title
                            .filter(|title| !title.trim().is_empty())
                            .unwrap_or_else(|| "New conversation".to_owned()),
                        now,
                    ],
                )?;
                id
            }
            ConversationSelection::Existing(id) => {
                if !self.conversation_exists(&id)? {
                    return Err(ConversationError::ConversationNotFound(id));
                }
                id
            }
        };

        self.active_conversation = Some(conversation_id.clone());
        self.load_snapshot(&conversation_id)
    }

    pub fn set_title_if_untitled(&mut self, text: &str) -> Result<bool, ConversationError> {
        let conversation_id = self
            .active_conversation
            .as_ref()
            .ok_or(ConversationError::NoActiveConversation)?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }
        let title = title_from_prompt(trimmed);
        if title == "New conversation" {
            return Ok(false);
        }
        let now = now_ms()?;
        let rows = self.connection.execute(
            "UPDATE conversations
                SET title = ?2, updated_at_ms = ?3
              WHERE id = ?1 AND title = 'New conversation'",
            params![conversation_id.as_str(), title, now],
        )?;
        Ok(rows > 0)
    }

    pub fn begin_turn(&mut self, input: BeginTurn) -> Result<BeginTurnOutcome, ConversationError> {
        let conversation_id = self
            .active_conversation
            .clone()
            .ok_or(ConversationError::NoActiveConversation)?;

        let projection = match self.build_projection(&conversation_id, input.agent)? {
            ProjectionBuild::Ready(projection) => projection,
            ProjectionBuild::NeedsSummary(job) => return Ok(BeginTurnOutcome::NeedsSummary(job)),
        };

        let resume_session =
            self.active_binding_for(&conversation_id, input.agent, projection.through_seq)?;
        let now = now_ms()?;
        let turn_id = TurnId::new(Uuid::new_v4().to_string());
        let message_id = Uuid::new_v4().to_string();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if Self::agent_is_busy_in(&transaction, &conversation_id, input.agent)? {
            return Err(ConversationError::AgentBusy(input.agent));
        }
        let seq: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM turns WHERE conversation_id = ?1",
            [conversation_id.as_str()],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO turns
                (id, conversation_id, seq, requested_agent, model_id, status,
                 context_through_seq, created_at_ms, owner_session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, ?7, ?8)",
            params![
                turn_id.as_str(),
                conversation_id.as_str(),
                seq,
                input.agent.as_db(),
                input.model_id,
                projection.through_seq,
                now,
                self.session_id,
            ],
        )?;
        let display_text = input
            .display_text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty() && *text != input.prompt.as_str());
        let title_prompt = display_text.unwrap_or(input.prompt.as_str());
        transaction.execute(
            "INSERT INTO messages
                (id, turn_id, ordinal, role, text, display_text, state, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, 0, 'user', ?3, ?4, 'complete', ?5, ?5)",
            params![message_id, turn_id.as_str(), input.prompt, display_text, now],
        )?;
        transaction.execute(
            "UPDATE conversations
                SET title = CASE WHEN title = 'New conversation' THEN ?2 ELSE title END,
                    updated_at_ms = ?3
              WHERE id = ?1",
            params![
                conversation_id.as_str(),
                title_from_prompt(title_prompt),
                now
            ],
        )?;
        transaction.commit()?;

        Ok(BeginTurnOutcome::Ready(AgentRequest {
            conversation_id,
            turn_id,
            agent: input.agent,
            model_id: input.model_id,
            prompt: input.prompt,
            resume_session,
            context: projection,
        }))
    }

    /// Retires one idle agent's native provider session without altering the
    /// durable conversation. The next turn receives the normal shared-context
    /// projection instead of resuming the retired provider session.
    pub fn retire_active_binding(&mut self, agent: AgentKey) -> Result<bool, ConversationError> {
        let conversation_id = self
            .active_conversation
            .clone()
            .ok_or(ConversationError::NoActiveConversation)?;
        if self.agent_is_busy(&conversation_id, agent)? {
            return Err(ConversationError::AgentBusy(agent));
        }

        let changed = self.connection.execute(
            "UPDATE provider_bindings SET status = 'stale', updated_at_ms = ?3
              WHERE conversation_id = ?1 AND agent = ?2 AND status = 'active'",
            params![conversation_id.as_str(), agent.as_db(), now_ms()?],
        )?;
        Ok(changed > 0)
    }

    /// Prepares a rolling summary for the active conversation and agent.
    ///
    /// The source contains only completed turns after the latest valid
    /// checkpoint, while the source hash covers the complete canonical head.
    pub fn prepare_summary(
        &self,
        agent: AgentKey,
    ) -> Result<Option<SummaryJob>, ConversationError> {
        let conversation_id = self
            .active_conversation
            .clone()
            .ok_or(ConversationError::NoActiveConversation)?;
        if self.agent_is_busy(&conversation_id, agent)? {
            return Err(ConversationError::AgentBusy(agent));
        }

        let turns = load_projected_turns(&self.connection, &conversation_id)?;
        let Some(covers_through_seq) = turns.last().map(|turn| turn.seq) else {
            return Ok(None);
        };
        let checkpoint = self.latest_valid_checkpoint(&conversation_id, agent, &turns)?;
        if checkpoint
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.covers >= covers_through_seq)
        {
            return Ok(None);
        }

        Ok(Some(make_summary_job(
            &conversation_id,
            agent,
            &turns,
            checkpoint.as_ref(),
            covers_through_seq,
        )))
    }

    pub fn record_turn_outputs(
        &mut self,
        outputs: &[TurnOutput],
    ) -> Result<Vec<TranscriptPatch>, ConversationError> {
        if outputs.is_empty() {
            return Ok(Vec::new());
        }

        let now = now_ms()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut patches = Vec::with_capacity(outputs.len());
        for output in outputs {
            let turn = load_turn_identity(&transaction, &output.turn_id)?;
            if turn.conversation_id != output.conversation_id || turn.agent != output.agent {
                return Err(ConversationError::InvalidOutput(format!(
                    "turn {} belongs to conversation {} and {}, not conversation {} and {}",
                    output.turn_id,
                    turn.conversation_id,
                    turn.agent,
                    output.conversation_id,
                    output.agent,
                )));
            }
            apply_turn_event(&transaction, output, &turn, now, &mut patches)?;
        }
        transaction.commit()?;
        Ok(patches)
    }

    /// Saves turn outputs, retrying while the database is busy (FR-2140).
    ///
    /// A reply that is written while another process holds the conversation
    /// database must not be lost: the save is retried with the same bounded
    /// backoff the open phase uses. `budget` bounds the total waiting time; a
    /// database that stays locked is reported as `ConversationError::Busy`.
    ///
    /// A spent budget (`Duration::ZERO`) still makes one attempt, and that
    /// attempt does not wait: the database may have become writable again, and
    /// the save has to recover instead of skipping the database for the rest of
    /// the turn.
    pub fn record_turn_outputs_with_retry(
        &mut self,
        outputs: &[TurnOutput],
        budget: Duration,
    ) -> Result<Vec<TranscriptPatch>, ConversationError> {
        if outputs.is_empty() {
            return Ok(Vec::new());
        }
        // One attempt must not block for the whole connection timeout: the
        // retry loop owns the waiting. A spent budget only probes the
        // database, so its attempt must not wait at all.
        let attempt_timeout = if budget.is_zero() {
            Duration::ZERO
        } else {
            SAVE_ATTEMPT_BUSY_TIMEOUT
        };
        self.connection.busy_timeout(attempt_timeout)?;
        let saved = with_busy_retry(budget, || {
            #[cfg(test)]
            count_save_attempt();
            self.record_turn_outputs(outputs)
        });
        self.connection.busy_timeout(BUSY_TIMEOUT)?;
        saved
    }

    pub fn commit_summary(
        &mut self,
        job: SummaryJob,
        document: SummaryDocument,
    ) -> Result<(), ConversationError> {
        let rendered_chars = document.render().chars().count();
        if rendered_chars > MAX_SUMMARY_CHARS {
            return Err(ConversationError::SummaryTooLarge {
                actual_chars: rendered_chars,
                maximum_chars: MAX_SUMMARY_CHARS,
            });
        }
        let summary_json = serde_json::to_string(&document)?;
        let now = now_ms()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM conversations
                 WHERE id = ?1 AND workspace_root = ?2 AND archived_at_ms IS NULL
             )",
            params![job.conversation_id.as_str(), self.workspace_root],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(ConversationError::ConversationNotFound(job.conversation_id));
        }
        let busy: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM turns
                 WHERE conversation_id = ?1 AND requested_agent = ?2
                   AND status IN ('queued', 'running')
             )",
            params![job.conversation_id.as_str(), job.agent.as_db()],
            |row| row.get(0),
        )?;
        if busy {
            return Err(ConversationError::AgentBusy(job.agent));
        }

        let turns = load_projected_turns(&transaction, &job.conversation_id)?;
        let current_hash = hash_turns(
            turns
                .iter()
                .filter(|turn| turn.seq <= job.covers_through_seq),
        );
        if current_hash != job.source_hash {
            return Err(ConversationError::StaleSummary);
        }

        transaction.execute(
            "INSERT INTO context_checkpoints
                (conversation_id, agent, covers_through_seq, source_hash,
                 prompt_version, summary_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(conversation_id, agent, covers_through_seq) DO UPDATE SET
                 source_hash = excluded.source_hash,
                 prompt_version = excluded.prompt_version,
                 summary_json = excluded.summary_json,
                 created_at_ms = excluded.created_at_ms",
            params![
                job.conversation_id.as_str(),
                job.agent.as_db(),
                job.covers_through_seq,
                &job.source_hash[..],
                SUMMARY_PROMPT_VERSION,
                summary_json,
                now,
            ],
        )?;
        transaction.execute(
            "UPDATE provider_bindings
                SET status = 'stale', updated_at_ms = ?3
              WHERE conversation_id = ?1 AND agent = ?2",
            params![job.conversation_id.as_str(), job.agent.as_db(), now],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn conversation_exists(&self, id: &ConversationId) -> Result<bool, ConversationError> {
        self.connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM conversations
                     WHERE id = ?1 AND workspace_root = ?2 AND archived_at_ms IS NULL
                 )",
                params![id.as_str(), self.workspace_root],
                |row| row.get(0),
            )
            .map_err(ConversationError::from)
    }

    /// Atomically interrupts unfinished turns owned by a closed, expired, or
    /// legacy-unowned session. Recovery is scoped to this engine's workspace;
    /// a live peer's queued/running turns remain untouched.
    pub fn recover_workspace(&mut self) -> Result<usize, ConversationError> {
        let now = now_ms()?;
        let cutoff = now.saturating_sub(SESSION_LEASE_MS);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "UPDATE runtime_sessions
                SET status = 'closed', closed_at_ms = ?3
              WHERE workspace_root = ?1
                AND id <> ?2
                AND status = 'active'
                AND last_seen_at_ms < ?4",
            params![self.workspace_root, self.session_id, now, cutoff],
        )?;
        transaction.execute(
            "UPDATE provider_bindings
                SET status = 'stale', updated_at_ms = ?3
              WHERE EXISTS (
                    SELECT 1
                      FROM turns t
                      JOIN conversations c ON c.id = t.conversation_id
                     WHERE c.workspace_root = ?1
                       AND t.status IN ('queued', 'running')
                       AND (
                            t.owner_session_id IS NULL
                            OR NOT EXISTS (
                                SELECT 1 FROM runtime_sessions s
                                 WHERE s.id = t.owner_session_id
                                   AND s.status = 'active'
                                   AND s.last_seen_at_ms >= ?2
                            )
                       )
                       AND t.conversation_id = provider_bindings.conversation_id
                       AND t.requested_agent = provider_bindings.agent
                )",
            params![self.workspace_root, cutoff, now],
        )?;
        transaction.execute(
            "UPDATE messages
                SET state = 'interrupted', updated_at_ms = ?3
              WHERE state = 'streaming'
                AND turn_id IN (
                    SELECT t.id
                      FROM turns t
                      JOIN conversations c ON c.id = t.conversation_id
                     WHERE c.workspace_root = ?1
                       AND t.status IN ('queued', 'running')
                       AND (
                            t.owner_session_id IS NULL
                            OR NOT EXISTS (
                                SELECT 1 FROM runtime_sessions s
                                 WHERE s.id = t.owner_session_id
                                   AND s.status = 'active'
                                   AND s.last_seen_at_ms >= ?2
                            )
                       )
                )",
            params![self.workspace_root, cutoff, now],
        )?;
        let recovered = transaction.execute(
            "UPDATE turns
                SET status = 'interrupted', completed_at_ms = ?3,
                    error_text = COALESCE(error_text, 'DmuxAI stopped before the turn completed')
              WHERE status IN ('queued', 'running')
                AND conversation_id IN (
                    SELECT id FROM conversations WHERE workspace_root = ?1
                )
                AND (
                    owner_session_id IS NULL
                    OR NOT EXISTS (
                        SELECT 1 FROM runtime_sessions s
                         WHERE s.id = turns.owner_session_id
                           AND s.status = 'active'
                           AND s.last_seen_at_ms >= ?2
                    )
                )",
            params![self.workspace_root, cutoff, now],
        )?;
        transaction.commit()?;
        Ok(recovered)
    }

    fn agent_is_busy(
        &self,
        conversation_id: &ConversationId,
        agent: AgentKey,
    ) -> Result<bool, ConversationError> {
        self.connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM turns
                     WHERE conversation_id = ?1 AND requested_agent = ?2
                       AND status IN ('queued', 'running')
                 )",
                params![conversation_id.as_str(), agent.as_db()],
                |row| row.get(0),
            )
            .map_err(ConversationError::from)
    }

    fn agent_is_busy_in(
        transaction: &Transaction<'_>,
        conversation_id: &ConversationId,
        agent: AgentKey,
    ) -> Result<bool, ConversationError> {
        transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM turns
                     WHERE conversation_id = ?1 AND requested_agent = ?2
                       AND status IN ('queued', 'running')
                 )",
                params![conversation_id.as_str(), agent.as_db()],
                |row| row.get(0),
            )
            .map_err(ConversationError::from)
    }

    fn active_binding_for(
        &self,
        conversation_id: &ConversationId,
        agent: AgentKey,
        through_seq: i64,
    ) -> Result<Option<String>, ConversationError> {
        self.connection
            .query_row(
                "SELECT provider_session_id
                   FROM provider_bindings
                  WHERE conversation_id = ?1 AND agent = ?2
                    AND status = 'active' AND known_through_seq >= ?3",
                params![conversation_id.as_str(), agent.as_db(), through_seq],
                |row| row.get(0),
            )
            .optional()
            .map_err(ConversationError::from)
    }

    fn build_projection(
        &self,
        conversation_id: &ConversationId,
        agent: AgentKey,
    ) -> Result<ProjectionBuild, ConversationError> {
        let turns = load_projected_turns(&self.connection, conversation_id)?;
        let through_seq = turns.last().map_or(0, |turn| turn.seq);
        let checkpoint = self.latest_valid_checkpoint(conversation_id, agent, &turns)?;
        let covers_through_seq = checkpoint
            .as_ref()
            .map_or(0, |checkpoint| checkpoint.covers);
        let recent = turns
            .iter()
            .filter(|turn| turn.seq > covers_through_seq)
            .cloned()
            .collect::<Vec<_>>();
        let summary = checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.document.render());
        let projected_chars = summary
            .as_ref()
            .map_or(0, |summary| summary.chars().count())
            + recent.iter().map(projected_turn_chars).sum::<usize>();
        if projected_chars <= MAX_CONTEXT_CHARS {
            return Ok(ProjectionBuild::Ready(ContextProjection {
                summary,
                recent,
                through_seq,
                source_hash: hash_turns(turns.iter()),
            }));
        }

        let mut tail_chars = 0;
        let mut tail_start = turns.len();
        for (index, turn) in turns.iter().enumerate().rev() {
            let chars = projected_turn_chars(turn);
            if tail_chars + chars > RECENT_TAIL_CHARS {
                break;
            }
            tail_chars += chars;
            tail_start = index;
        }
        let covers_index = if tail_start == 0 {
            turns.len()
        } else {
            tail_start
        };
        let source_turns = turns[..covers_index].to_vec();
        let covers = source_turns.last().map_or(through_seq, |turn| turn.seq);
        let covers = if checkpoint
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.covers >= covers)
        {
            through_seq
        } else {
            covers
        };
        Ok(ProjectionBuild::NeedsSummary(make_summary_job(
            conversation_id,
            agent,
            &turns,
            checkpoint.as_ref(),
            covers,
        )))
    }

    fn latest_valid_checkpoint(
        &self,
        conversation_id: &ConversationId,
        agent: AgentKey,
        turns: &[ProjectedTurn],
    ) -> Result<Option<Checkpoint>, ConversationError> {
        let mut statement = self.connection.prepare(
            "SELECT covers_through_seq, source_hash, summary_json
               FROM context_checkpoints
              WHERE conversation_id = ?1 AND agent = ?2 AND prompt_version = ?3
              ORDER BY covers_through_seq DESC",
        )?;
        let mut rows = statement.query(params![
            conversation_id.as_str(),
            agent.as_db(),
            SUMMARY_PROMPT_VERSION,
        ])?;
        while let Some(row) = rows.next()? {
            let covers: i64 = row.get(0)?;
            let stored_hash: Vec<u8> = row.get(1)?;
            let expected_hash = hash_turns(turns.iter().filter(|turn| turn.seq <= covers));
            if stored_hash.as_slice() != expected_hash {
                continue;
            }
            let summary_json: String = row.get(2)?;
            let document = serde_json::from_str(&summary_json)?;
            return Ok(Some(Checkpoint { covers, document }));
        }
        Ok(None)
    }

    fn load_snapshot(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<ConversationSnapshot, ConversationError> {
        let title = self.connection.query_row(
            "SELECT title FROM conversations WHERE id = ?1 AND workspace_root = ?2",
            params![conversation_id.as_str(), self.workspace_root],
            |row| row.get(0),
        )?;

        let mut turn_statement = self.connection.prepare(
            "SELECT id, seq, requested_agent, model_id, status, error_text
               FROM turns WHERE conversation_id = ?1 ORDER BY seq",
        )?;
        let turns = turn_statement
            .query_map([conversation_id.as_str()], |row| {
                Ok(TurnSnapshot {
                    id: TurnId::new(row.get::<_, String>(0)?),
                    seq: row.get(1)?,
                    agent: AgentKey::from_db(&row.get::<_, String>(2)?)?,
                    model_id: row.get(3)?,
                    status: TurnStatus::from_db(&row.get::<_, String>(4)?)?,
                    error_text: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut message_statement = self.connection.prepare(
            "SELECT m.id, m.turn_id, t.seq, t.requested_agent,
                    m.role, m.text, m.state, m.display_text
               FROM messages m
               JOIN turns t ON t.id = m.turn_id
              WHERE t.conversation_id = ?1
              ORDER BY t.seq, m.ordinal",
        )?;
        let messages = message_statement
            .query_map([conversation_id.as_str()], |row| {
                let role = MessageRole::from_db(&row.get::<_, String>(4)?)?;
                let display_text: Option<String> = if role == MessageRole::Assistant {
                    None
                } else {
                    row.get(7)?
                };
                Ok(TranscriptMessage {
                    id: row.get(0)?,
                    turn_id: TurnId::new(row.get::<_, String>(1)?),
                    turn_seq: row.get(2)?,
                    agent: AgentKey::from_db(&row.get::<_, String>(3)?)?,
                    role,
                    text: row.get(5)?,
                    display_text,
                    state: MessageState::from_db(&row.get::<_, String>(6)?)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut binding_statement = self.connection.prepare(
            "SELECT agent, provider_session_id, status, known_through_seq
               FROM provider_bindings
              WHERE conversation_id = ?1
              ORDER BY agent",
        )?;
        let bindings = binding_statement
            .query_map([conversation_id.as_str()], |row| {
                Ok(ProviderBinding {
                    agent: AgentKey::from_db(&row.get::<_, String>(0)?)?,
                    provider_session_id: row.get(1)?,
                    status: BindingStatus::from_db(&row.get::<_, String>(2)?)?,
                    known_through_seq: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut activity_statement = self.connection.prepare(
            "SELECT a.turn_id, a.ordinal, a.payload_json
               FROM activities a
               JOIN turns t ON t.id = a.turn_id
              WHERE t.conversation_id = ?1
              ORDER BY t.seq, a.ordinal",
        )?;
        let mut activities = Vec::new();
        let mut rows = activity_statement.query([conversation_id.as_str()])?;
        while let Some(row) = rows.next()? {
            let turn_id = TurnId::new(row.get::<_, String>(0)?);
            let ordinal: i64 = row.get(1)?;
            let payload_json: String = row.get(2)?;
            if let Ok(item) = serde_json::from_str::<ActivityItem>(&payload_json) {
                activities.push(TurnActivity {
                    turn_id,
                    ordinal,
                    item,
                });
            }
        }

        Ok(ConversationSnapshot {
            id: conversation_id.clone(),
            title,
            turns,
            messages,
            bindings,
            activities,
        })
    }

    pub fn record_activity(
        &mut self,
        turn_id: &TurnId,
        item: ActivityItem,
    ) -> Result<(), ConversationError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        let turn_exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM turns WHERE id = ?1)",
            [turn_id.as_str()],
            |row| row.get(0),
        )?;
        if !turn_exists {
            return Err(ConversationError::InvalidOutput(format!(
                "unknown turn {turn_id}"
            )));
        }

        let mut items = Vec::new();
        {
            let mut statement = transaction.prepare(
                "SELECT ordinal, payload_json FROM activities WHERE turn_id = ?1 ORDER BY ordinal",
            )?;
            let mut rows = statement.query([turn_id.as_str()])?;
            while let Some(row) = rows.next()? {
                let payload_json: String = row.get(1)?;
                if let Ok(deserialized) = serde_json::from_str::<ActivityItem>(&payload_json) {
                    items.push(deserialized);
                }
            }
        }

        let idx = merge_activity(&mut items, item);
        if idx >= MAX_ACTIVITIES_PER_TURN {
            return Ok(());
        }

        let target_item = &mut items[idx];
        match target_item {
            ActivityItem::Tool { detail, .. } => {
                if detail.chars().count() > MAX_ACTIVITY_TEXT_CHARS {
                    let bounded = truncate_chars(detail, MAX_ACTIVITY_TEXT_CHARS);
                    *detail = bounded.to_owned();
                }
            }
            ActivityItem::Thinking { text, .. } => {
                if text.chars().count() > MAX_ACTIVITY_TEXT_CHARS {
                    let bounded = truncate_chars(text, MAX_ACTIVITY_TEXT_CHARS);
                    *text = bounded.to_owned();
                }
            }
        }

        let kind = match target_item {
            ActivityItem::Tool { .. } => "tool",
            ActivityItem::Thinking { .. } => "thinking",
        };
        let state = match target_item {
            ActivityItem::Tool { status, .. } => match status {
                ToolStatus::Running => "running",
                ToolStatus::Ok => "ok",
                ToolStatus::Failed => "failed",
            },
            ActivityItem::Thinking { finished_ms, .. } => {
                if finished_ms.is_none() {
                    "live"
                } else {
                    "done"
                }
            }
        };
        let summary_raw = collapsed_line(target_item);
        let summary = truncate_chars(&summary_raw, 200);
        let payload_json = serde_json::to_string(target_item)?;
        let now = now_ms()?;

        transaction.execute(
            "INSERT INTO activities
                (turn_id, ordinal, kind, summary, state, payload_json, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(turn_id, ordinal) DO UPDATE SET
                 kind = excluded.kind,
                 summary = excluded.summary,
                 state = excluded.state,
                 payload_json = excluded.payload_json,
                 updated_at_ms = excluded.updated_at_ms",
            params![
                turn_id.as_str(),
                idx as i64,
                kind,
                summary,
                state,
                payload_json,
                now,
            ],
        )?;

        transaction.commit()?;
        Ok(())
    }
}

impl Drop for ConversationEngine {
    fn drop(&mut self) {
        let _ = self.close_session();
    }
}

#[derive(Debug)]
enum ProjectionBuild {
    Ready(ContextProjection),
    NeedsSummary(SummaryJob),
}

#[derive(Debug)]
struct Checkpoint {
    covers: i64,
    document: SummaryDocument,
}

fn make_summary_job(
    conversation_id: &ConversationId,
    agent: AgentKey,
    turns: &[ProjectedTurn],
    checkpoint: Option<&Checkpoint>,
    covers_through_seq: i64,
) -> SummaryJob {
    let previous_covers = checkpoint.map_or(0, |checkpoint| checkpoint.covers);
    SummaryJob {
        conversation_id: conversation_id.clone(),
        agent,
        covers_through_seq,
        source_hash: hash_turns(turns.iter().filter(|turn| turn.seq <= covers_through_seq)),
        previous_summary: checkpoint.map(|checkpoint| checkpoint.document.clone()),
        source_turns: turns
            .iter()
            .filter(|turn| turn.seq > previous_covers && turn.seq <= covers_through_seq)
            .cloned()
            .collect(),
    }
}

#[derive(Debug)]
struct TurnIdentity {
    conversation_id: ConversationId,
    agent: AgentKey,
    seq: i64,
    status: TurnStatus,
    context_through_seq: i64,
}

fn load_turn_identity(
    transaction: &Transaction<'_>,
    turn_id: &TurnId,
) -> Result<TurnIdentity, ConversationError> {
    transaction
        .query_row(
            "SELECT conversation_id, requested_agent, seq, status, context_through_seq
               FROM turns WHERE id = ?1",
            [turn_id.as_str()],
            |row| {
                Ok(TurnIdentity {
                    conversation_id: ConversationId::new(row.get::<_, String>(0)?),
                    agent: AgentKey::from_db(&row.get::<_, String>(1)?)?,
                    seq: row.get(2)?,
                    status: TurnStatus::from_db(&row.get::<_, String>(3)?)?,
                    context_through_seq: row.get(4)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| ConversationError::InvalidOutput(format!("unknown turn {turn_id}")))
}

fn apply_turn_event(
    transaction: &Transaction<'_>,
    output: &TurnOutput,
    turn: &TurnIdentity,
    now: i64,
    patches: &mut Vec<TranscriptPatch>,
) -> Result<(), ConversationError> {
    match &output.event {
        TurnEvent::Started => {
            ensure_turn_state(turn, &[TurnStatus::Queued], "start")?;
            transaction.execute(
                "UPDATE turns SET status = 'running', started_at_ms = ?2 WHERE id = ?1",
                params![output.turn_id.as_str(), now],
            )?;
            patches.push(TranscriptPatch::TurnStarted {
                turn_id: output.turn_id.clone(),
                agent: output.agent,
            });
        }
        TurnEvent::ProviderSessionBound(provider_session_id) => {
            ensure_turn_state(turn, &[TurnStatus::Queued, TurnStatus::Running], "bind")?;
            transaction.execute(
                "INSERT INTO provider_bindings
                    (conversation_id, agent, provider_session_id, status,
                     known_through_seq, bound_by_turn_id, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, 'active', ?4, ?5, ?6, ?6)
                 ON CONFLICT(conversation_id, agent) DO UPDATE SET
                     provider_session_id = excluded.provider_session_id,
                     status = 'active',
                     known_through_seq = excluded.known_through_seq,
                     bound_by_turn_id = excluded.bound_by_turn_id,
                     updated_at_ms = excluded.updated_at_ms",
                params![
                    output.conversation_id.as_str(),
                    output.agent.as_db(),
                    provider_session_id,
                    turn.context_through_seq,
                    output.turn_id.as_str(),
                    now,
                ],
            )?;
            patches.push(TranscriptPatch::ProviderBound {
                agent: output.agent,
                provider_session_id: provider_session_id.clone(),
            });
        }
        TurnEvent::TextDelta(delta) => {
            ensure_turn_state(
                turn,
                &[TurnStatus::Queued, TurnStatus::Running],
                "append text",
            )?;
            transaction.execute(
                "UPDATE turns
                    SET status = 'running', started_at_ms = COALESCE(started_at_ms, ?2)
                  WHERE id = ?1",
                params![output.turn_id.as_str(), now],
            )?;
            transaction.execute(
                "INSERT INTO messages
                    (id, turn_id, ordinal, role, text, state, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, 1, 'assistant', ?3, 'streaming', ?4, ?4)
                 ON CONFLICT(turn_id, role) DO UPDATE SET
                     text = messages.text || excluded.text,
                     updated_at_ms = excluded.updated_at_ms",
                params![
                    Uuid::new_v4().to_string(),
                    output.turn_id.as_str(),
                    delta,
                    now,
                ],
            )?;
            patches.push(TranscriptPatch::AssistantDelta {
                turn_id: output.turn_id.clone(),
                agent: output.agent,
                delta: delta.clone(),
            });
        }
        TurnEvent::Notice(text) => patches.push(TranscriptPatch::Notice {
            turn_id: output.turn_id.clone(),
            agent: output.agent,
            text: text.clone(),
        }),
        TurnEvent::Completed => {
            ensure_turn_state(turn, &[TurnStatus::Queued, TurnStatus::Running], "complete")?;
            finish_turn(transaction, output, turn, now, TurnStatus::Completed, None)?;
            transaction.execute(
                "UPDATE provider_bindings
                    SET known_through_seq = MAX(known_through_seq, ?3), updated_at_ms = ?4
                  WHERE conversation_id = ?1 AND agent = ?2 AND status = 'active'",
                params![
                    output.conversation_id.as_str(),
                    output.agent.as_db(),
                    turn.seq,
                    now,
                ],
            )?;
            patches.push(TranscriptPatch::TurnFinished {
                turn_id: output.turn_id.clone(),
                agent: output.agent,
                status: TurnStatus::Completed,
            });
        }
        TurnEvent::Cancelled => {
            ensure_turn_state(turn, &[TurnStatus::Queued, TurnStatus::Running], "cancel")?;
            finish_turn(transaction, output, turn, now, TurnStatus::Cancelled, None)?;
            mark_binding_stale(transaction, output, now)?;
            patches.push(TranscriptPatch::TurnFinished {
                turn_id: output.turn_id.clone(),
                agent: output.agent,
                status: TurnStatus::Cancelled,
            });
        }
        TurnEvent::Failed {
            message,
            invalidates_binding,
        } => {
            ensure_turn_state(turn, &[TurnStatus::Queued, TurnStatus::Running], "fail")?;
            finish_turn(
                transaction,
                output,
                turn,
                now,
                TurnStatus::Failed,
                Some(message),
            )?;
            if *invalidates_binding {
                mark_binding_stale(transaction, output, now)?;
            }
            patches.push(TranscriptPatch::TurnFinished {
                turn_id: output.turn_id.clone(),
                agent: output.agent,
                status: TurnStatus::Failed,
            });
        }
    }
    Ok(())
}

fn ensure_turn_state(
    turn: &TurnIdentity,
    allowed: &[TurnStatus],
    operation: &str,
) -> Result<(), ConversationError> {
    if allowed.contains(&turn.status) {
        Ok(())
    } else {
        Err(ConversationError::InvalidOutput(format!(
            "cannot {operation} a {:?} turn",
            turn.status
        )))
    }
}

fn finish_turn(
    transaction: &Transaction<'_>,
    output: &TurnOutput,
    _turn: &TurnIdentity,
    now: i64,
    status: TurnStatus,
    error: Option<&String>,
) -> Result<(), ConversationError> {
    let status_text = match status {
        TurnStatus::Completed => "completed",
        TurnStatus::Cancelled => "cancelled",
        TurnStatus::Failed => "failed",
        _ => unreachable!("finish_turn only accepts terminal states"),
    };
    transaction.execute(
        "UPDATE turns
            SET status = ?2, error_text = ?3, completed_at_ms = ?4
          WHERE id = ?1",
        params![output.turn_id.as_str(), status_text, error, now],
    )?;
    if status == TurnStatus::Completed {
        transaction.execute(
            "INSERT INTO messages
                (id, turn_id, ordinal, role, text, state, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, 1, 'assistant', '', 'complete', ?3, ?3)
             ON CONFLICT(turn_id, role) DO UPDATE SET
                 state = 'complete', updated_at_ms = excluded.updated_at_ms",
            params![Uuid::new_v4().to_string(), output.turn_id.as_str(), now],
        )?;
    } else {
        transaction.execute(
            "UPDATE messages SET state = 'interrupted', updated_at_ms = ?2
              WHERE turn_id = ?1 AND role = 'assistant'",
            params![output.turn_id.as_str(), now],
        )?;
    }
    transaction.execute(
        "UPDATE conversations SET updated_at_ms = ?2 WHERE id = ?1",
        params![output.conversation_id.as_str(), now],
    )?;
    Ok(())
}

fn mark_binding_stale(
    transaction: &Transaction<'_>,
    output: &TurnOutput,
    now: i64,
) -> Result<(), ConversationError> {
    transaction.execute(
        "UPDATE provider_bindings SET status = 'stale', updated_at_ms = ?3
          WHERE conversation_id = ?1 AND agent = ?2",
        params![output.conversation_id.as_str(), output.agent.as_db(), now],
    )?;
    Ok(())
}

fn load_projected_turns(
    connection: &Connection,
    conversation_id: &ConversationId,
) -> Result<Vec<ProjectedTurn>, ConversationError> {
    let mut statement = connection.prepare(
        "SELECT t.seq,
                t.requested_agent,
                MAX(CASE WHEN m.role = 'user' THEN m.text END),
                MAX(CASE WHEN m.role = 'assistant' THEN m.text END)
           FROM turns t
           JOIN messages m ON m.turn_id = t.id
          WHERE t.conversation_id = ?1
            AND t.status = 'completed'
         GROUP BY t.id, t.seq
         HAVING COUNT(CASE WHEN m.role = 'user' THEN 1 END) = 1
            AND COUNT(CASE WHEN m.role = 'assistant' THEN 1 END) = 1
            AND MAX(CASE WHEN m.role = 'user' THEN m.text END)
                NOT LIKE '[DmuxAI explicit context import]%'
          ORDER BY t.seq",
    )?;
    let turns = statement.query_map(params![conversation_id.as_str()], |row| {
        Ok(ProjectedTurn {
            seq: row.get(0)?,
            agent: AgentKey::from_db(&row.get::<_, String>(1)?)?,
            user: row.get(2)?,
            assistant: row.get(3)?,
        })
    })?;
    turns
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(ConversationError::from)
}

fn hash_turns<'a>(turns: impl IntoIterator<Item = &'a ProjectedTurn>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for turn in turns {
        hasher.update(turn.seq.to_le_bytes());
        hash_text(&mut hasher, turn.agent.as_db());
        hash_text(&mut hasher, &turn.user);
        hash_text(&mut hasher, &turn.assistant);
    }
    hasher.finalize().into()
}

fn hash_text(hasher: &mut Sha256, text: &str) {
    hasher.update((text.len() as u64).to_le_bytes());
    hasher.update(text.as_bytes());
}

fn projected_turn_chars(turn: &ProjectedTurn) -> usize {
    turn.user.chars().count() + turn.assistant.chars().count() + 32
}

fn normalize_workspace(path: &Path) -> Result<String, ConversationError> {
    crate::workspace::normalize_workspace_path(path).map_err(ConversationError::from)
}

fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

fn title_from_prompt(prompt: &str) -> String {
    let title = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = title.chars();
    let prefix = chars.by_ref().take(80).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else if prefix.is_empty() {
        "New conversation".to_owned()
    } else {
        prefix
    }
}

fn now_ms() -> Result<i64, ConversationError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ConversationError::ClockBeforeUnixEpoch)?
        .as_millis();
    i64::try_from(millis).map_err(|_| ConversationError::ClockBeforeUnixEpoch)
}

/// SPEC-dashboard-fullwidth FR-1225: read-only listing of the conversations
/// of *other* workspaces, scanned from the per-workspace databases under the
/// state dir's `repos/` (FR-450). Each database is opened read-only; only rows
/// whose workspace hashes to that database's directory are taken, so copies
/// left by the legacy-database migration (FR-452) are not listed twice.
/// Unreadable or newer-schema databases are skipped. Newest first.
pub fn list_state_dir_conversations(
    repos_dir: &Path,
    exclude_workspace: &str,
    limit: usize,
) -> Vec<(String, ConversationCard)> {
    let Ok(entries) = fs::read_dir(repos_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let db_path = entry.path().join("conversations.sqlite3");
        if !db_path.is_file() {
            continue;
        }
        let Ok(rows) = read_conversation_cards(&db_path, exclude_workspace, limit) else {
            continue;
        };
        out.extend(rows.into_iter().filter(|(workspace, _)| {
            crate::state_path::repo_key_for_canonical(workspace) == dir_name
        }));
    }
    out.sort_by(|a, b| {
        b.1.updated_at_ms
            .cmp(&a.1.updated_at_ms)
            .then_with(|| b.1.id.as_str().cmp(a.1.id.as_str()))
    });
    out.truncate(limit);
    out
}

fn read_conversation_cards(
    db_path: &Path,
    exclude_workspace: &str,
    limit: usize,
) -> Result<Vec<(String, ConversationCard)>, ConversationError> {
    let connection = Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(std::time::Duration::from_millis(250))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if !(1..=SCHEMA_VERSION).contains(&version) {
        return Ok(Vec::new());
    }
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut statement = connection.prepare(
        "SELECT c.id, c.title, c.updated_at_ms,
                COUNT(t.id),
                (SELECT requested_agent
                   FROM turns latest
                  WHERE latest.conversation_id = c.id
                  ORDER BY latest.seq DESC
                  LIMIT 1),
                EXISTS(
                    SELECT 1
                      FROM turns active
                     WHERE active.conversation_id = c.id
                       AND active.status IN ('queued', 'running')
                ),
                c.workspace_root
           FROM conversations c
           LEFT JOIN turns t ON t.conversation_id = c.id
          WHERE c.workspace_root != ?1 AND c.archived_at_ms IS NULL
          GROUP BY c.id
          ORDER BY c.updated_at_ms DESC, c.id DESC
          LIMIT ?2",
    )?;
    let rows = statement.query_map(params![exclude_workspace, limit], |row| {
        let last_agent: Option<String> = row.get(4)?;
        Ok((
            row.get::<_, String>(6)?,
            ConversationCard {
                id: ConversationId::new(row.get::<_, String>(0)?),
                title: row.get(1)?,
                updated_at_ms: row.get(2)?,
                turn_count: row.get::<_, i64>(3)?.try_into().map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(3, Type::Integer, Box::new(error))
                })?,
                last_agent: last_agent.as_deref().map(AgentKey::from_db).transpose()?,
                running: row.get(5)?,
            },
        ))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(ConversationError::from)
}

#[cfg(test)]
mod schema_v2_tests {
    use super::*;

    #[test]
    fn v1_database_is_migrated_and_accepts_custom_agents() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("ws");
        std::fs::create_dir(&workspace).unwrap();
        let db = temp.path().join("db.sqlite3");
        {
            let connection = Connection::open(&db).unwrap();
            connection.execute_batch(MIGRATION_1).unwrap();
            connection
                .execute_batch(
                    "INSERT INTO conversations VALUES ('c1', '/x', 't', 1, 1, NULL);
                     INSERT INTO turns (id, conversation_id, seq, requested_agent, status, created_at_ms)
                       VALUES ('t1', 'c1', 1, 'agy', 'completed', 1);
                     INSERT INTO messages VALUES ('m1', 't1', 0, 'user', 'hi', 'complete', 1, 1);",
                )
                .unwrap();
            assert!(
                connection
                    .execute(
                        "INSERT INTO turns (id, conversation_id, seq, requested_agent, status, created_at_ms)
                           VALUES ('t2', 'c1', 2, 'opencode', 'completed', 1)",
                        [],
                    )
                    .is_err()
            );
        }
        drop(ConversationEngine::open(&db, &workspace).unwrap());
        let connection = Connection::open(&db).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 3);
        connection
            .execute(
                "INSERT INTO turns (id, conversation_id, seq, requested_agent, status, created_at_ms)
                   VALUES ('t2', 'c1', 2, 'opencode', 'completed', 1)",
                [],
            )
            .unwrap();
        let kept: String = connection
            .query_row("SELECT text FROM messages WHERE id = 'm1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(kept, "hi");
        let index_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'turns_by_conversation_seq'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 1);
        // Re-opening a v2 database is a no-op.
        drop(ConversationEngine::open(&db, &workspace).unwrap());
    }
}

/// SPEC-agent-api FR-2120: opening the conversation database must not take a
/// write lock when the schema is current, must retry a busy database, and must
/// report a still-busy database as a conflict instead of an internal error.
#[cfg(test)]
mod busy_open_tests {
    use super::*;

    fn temp_workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let db = temp.path().join("conversations.sqlite3");
        (temp, workspace, db)
    }

    /// Opens a database that already carries the current schema.
    fn current_schema_db(workspace: &Path, db: &Path) {
        drop(ConversationEngine::open(db, workspace).unwrap());
    }

    #[test]
    fn tc_2120_open_of_current_schema_takes_no_write_lock() {
        let (_temp, workspace, db) = temp_workspace();
        current_schema_db(&workspace, &db);

        // A second process holds the write lock of the same file.
        let blocker = Connection::open(&db).unwrap();
        blocker.busy_timeout(Duration::from_secs(1)).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();

        // The runtime-session schema is current, so the open phase checks it
        // read-only and needs no write transaction.
        let reader = Connection::open(&db).unwrap();
        assert!(
            runtime_session_schema_is_current(&reader).unwrap(),
            "the runtime-session schema must be recognized as current"
        );
        drop(reader);

        // The remaining write is the runtime-session insert, which is the only
        // statement that needs the lock.
        let error = match open_with_retry(&db, &workspace, Duration::from_millis(400)) {
            Ok(_) => panic!("the open must not succeed while another process holds the lock"),
            Err(error) => error,
        };
        let ConversationError::Busy(message) = error else {
            panic!("expected a busy conversation database, got {error:?}");
        };
        assert!(
            message.contains("database is locked") && message.contains("retry"),
            "the busy message must be actionable, got {message}"
        );

        blocker.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn tc_2120_open_succeeds_once_the_lock_is_released() {
        let (_temp, workspace, db) = temp_workspace();
        current_schema_db(&workspace, &db);

        // Another process holds the write lock and releases it shortly after.
        let blocker = Connection::open(&db).unwrap();
        blocker.busy_timeout(Duration::from_secs(1)).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            blocker.execute_batch("COMMIT").unwrap();
        });

        let engine = open_with_retry(&db, &workspace, Duration::from_secs(10))
            .expect("the open must succeed once the lock is released");
        assert!(!engine.session_id.is_empty());
        releaser.join().unwrap();
    }

    #[test]
    fn tc_2120_is_busy_matches_busy_and_locked_codes() {
        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::DatabaseBusy,
                extended_code: 5,
            },
            None,
        );
        let locked = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::DatabaseLocked,
                extended_code: 6,
            },
            None,
        );
        assert!(is_busy(&busy));
        assert!(is_busy(&locked));
        assert!(!is_busy(&rusqlite::Error::InvalidQuery));
        assert!(!is_busy(&rusqlite::Error::QueryReturnedNoRows));
    }

    #[test]
    fn tc_2120_a_locked_database_produces_a_busy_sqlite_error() {
        let (_temp, workspace, db) = temp_workspace();
        current_schema_db(&workspace, &db);

        let blocker = Connection::open(&db).unwrap();
        blocker.busy_timeout(Duration::from_secs(1)).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();

        let writer = Connection::open(&db).unwrap();
        writer.busy_timeout(Duration::from_millis(50)).unwrap();
        let error = writer
            .execute(
                "INSERT INTO runtime_sessions
                    (id, workspace_root, status, created_at_ms, last_seen_at_ms)
                 VALUES ('other', '/ws', 'active', 1, 1)",
                [],
            )
            .unwrap_err();
        assert!(is_busy(&error), "expected a busy sqlite error, got {error}");

        blocker.execute_batch("ROLLBACK").unwrap();
    }
}

/// SPEC-agent-api FR-2140: a turn-output save must retry a busy conversation
/// database, persist the reply once the lock is released, and report a
/// database that stays locked as a busy error.
#[cfg(test)]
mod busy_save_tests {
    use super::*;

    fn temp_workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let db = temp.path().join("conversations.sqlite3");
        (temp, workspace, db)
    }

    /// A conversation with one open turn, ready for turn outputs.
    fn engine_with_turn(db: &Path, workspace: &Path) -> (ConversationEngine, TurnOutput) {
        let mut engine = ConversationEngine::open(db, workspace).unwrap();
        let conversation = engine
            .select(ConversationSelection::New { title: None })
            .unwrap();
        let request = match engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "hello".to_owned(),
                display_text: None,
            })
            .unwrap()
        {
            BeginTurnOutcome::Ready(request) => request,
            other => panic!("expected a ready turn, got {other:?}"),
        };
        let output = TurnOutput {
            conversation_id: ConversationId::new(conversation.id.as_str()),
            turn_id: TurnId::new(request.turn_id.as_str()),
            agent: AgentKey::Codex,
            event: TurnEvent::TextDelta("the reply".to_owned()),
        };
        (engine, output)
    }

    #[test]
    fn tc_2140_busy_save_is_retried_and_persisted_once_the_lock_is_released() {
        let (_temp, workspace, db) = temp_workspace();
        let (mut engine, output) = engine_with_turn(&db, &workspace);

        // Another process holds the write lock and releases it shortly after.
        let blocker = Connection::open(&db).unwrap();
        blocker.busy_timeout(Duration::from_secs(1)).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            blocker.execute_batch("COMMIT").unwrap();
        });

        let attempts_before = save_attempts();
        let saved = engine
            .record_turn_outputs_with_retry(&[output], Duration::from_secs(10))
            .expect("the save must succeed once the lock is released");
        let attempts = save_attempts() - attempts_before;
        assert!(
            attempts >= 2,
            "a busy save must be retried, made {attempts} attempts"
        );
        assert!(!saved.is_empty());
        releaser.join().unwrap();

        let connection = Connection::open(&db).unwrap();
        let text: String = connection
            .query_row(
                "SELECT text FROM messages WHERE role = 'assistant'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(text, "the reply");
    }

    #[test]
    fn tc_2140_a_locked_database_fails_the_save_with_a_busy_error() {
        let (_temp, workspace, db) = temp_workspace();
        let (mut engine, output) = engine_with_turn(&db, &workspace);

        let blocker = Connection::open(&db).unwrap();
        blocker.busy_timeout(Duration::from_secs(1)).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();

        let error = engine
            .record_turn_outputs_with_retry(&[output], Duration::from_millis(400))
            .expect_err("the save must fail while the database is locked");
        let ConversationError::Busy(message) = error else {
            panic!("expected a busy conversation database, got {error:?}");
        };
        assert!(
            message.contains("database is locked") && message.contains("retry"),
            "the busy message must be actionable, got {message}"
        );

        blocker.execute_batch("ROLLBACK").unwrap();
    }

    /// A spent budget must still reach the database: the lock can be released
    /// right after the budget ran out, and the save has to recover instead of
    /// skipping the database for the rest of the turn (FR-2140).
    #[test]
    fn tc_2140_a_spent_budget_still_saves_once_the_lock_is_released() {
        let (_temp, workspace, db) = temp_workspace();
        let (mut engine, output) = engine_with_turn(&db, &workspace);

        let blocker = Connection::open(&db).unwrap();
        blocker.busy_timeout(Duration::from_secs(1)).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();

        // The budget is spent: the save reports the busy database.
        let error = engine
            .record_turn_outputs_with_retry(std::slice::from_ref(&output), Duration::ZERO)
            .expect_err("a locked database must fail the save");
        assert!(
            matches!(error, ConversationError::Busy(_)),
            "expected a busy conversation database, got {error:?}"
        );

        // The lock is released: the next save recovers, even with a spent
        // budget, and the reply is stored.
        blocker.execute_batch("ROLLBACK").unwrap();
        engine
            .record_turn_outputs_with_retry(&[output], Duration::ZERO)
            .expect("the save must recover once the database is writable");

        let connection = Connection::open(&db).unwrap();
        let text: String = connection
            .query_row(
                "SELECT text FROM messages WHERE role = 'assistant'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(text, "the reply");
    }
}
