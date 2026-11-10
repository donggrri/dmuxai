#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::run_journal::generate_run_id;
use crate::term::backend::{Backend, now_ms};
use crate::term::buffer::RingBuffer;
use crate::term::client::socket_path;
use crate::term::journal::{TERM_IO_COALESCE_MAX_DELAY_MS, TermJournal};
use crate::term::policy::{ApprovalGate, Decision, Policy, RUN_ID_ENV, approve_with_run_guard};
use crate::term::proto::{PROTOCOL_VERSION, RequestOp, Response, read_request, write_response};
use crate::term::wait::{
    MARKER_PREFIX, WaitOptions, WaitReason, WaitResult, exec_output_text, marker_command,
    parse_exit_marker, wait,
};
use crate::term::{
    SessionInfo, SessionKind, SessionSpec, SessionState, TermError, check_input_size,
    ensure_input_within_limit, exit, parse_key_list, validate_dimensions, validate_name,
};

const DEFAULT_IDLE_GRACE: Duration = Duration::from_secs(600);
const JOURNAL_CHECK_WINDOW: Duration = Duration::from_millis(250);
const EXEC_CMD_IDLE: Duration = Duration::from_millis(200);
const DEFAULT_EXEC_TIMEOUT: Duration = Duration::from_secs(30);

type SessionHandle = Arc<Mutex<Session>>;

struct Session {
    spec: SessionSpec,
    state: SessionState,
    backend: Backend,
    created_ms: u64,
    takeover: bool,
    journal_error: Option<String>,
}

impl Session {
    fn refresh(&mut self) {
        if self.state != SessionState::Open {
            return;
        }
        if self.backend.is_lost() {
            self.state = SessionState::Lost;
            self.backend.record_event("state lost");
        } else if self.backend.exit_status().is_some() {
            self.state = SessionState::Exited;
        }
    }

    fn info(&self) -> SessionInfo {
        let exit = self.backend.exit_status();
        SessionInfo {
            name: self.spec.name.clone(),
            kind: self.spec.kind,
            state: self.state,
            pid: self.backend.pid(),
            created_ms: self.created_ms,
            last_output_ms: self.backend.last_output_ms(),
            cols: self.spec.cols,
            rows: self.spec.rows,
            takeover: self.takeover,
            exit_code: exit.and_then(|status| status.code),
            signal: exit.and_then(|status| status.signal),
            seq: self.backend.seq(),
            journal_error: self
                .journal_error
                .clone()
                .or_else(|| self.backend.journal_error()),
        }
    }
}

struct Shared {
    workspace_root: PathBuf,
    sessions: Mutex<BTreeMap<String, SessionHandle>>,
    policy: Policy,
    approvals: ApprovalGate,
    shutdown: AtomicBool,
    wake: UnixStream,
}

pub struct Host {
    shared: Arc<Shared>,
    listener: UnixListener,
    idle_grace: Duration,
    journal_flushed: BTreeMap<String, u64>,
    journal_check_after: Option<Instant>,
    wake_rx: UnixStream,
}

pub fn run(workspace_root: &Path) -> Result<(), TermError> {
    let path = socket_path(workspace_root).map_err(|error| {
        TermError::internal(format!("could not choose the term socket: {error}"))
    })?;
    run_at(workspace_root, &path)
}

pub(crate) fn run_at(workspace_root: &Path, path: &Path) -> Result<(), TermError> {
    let listener = prepare_listener(path)?;
    let _guard = SocketGuard(path.to_path_buf());
    let idle_grace = std::env::var("DMUXAI_TERM_IDLE_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_IDLE_GRACE);
    let (wake, wake_rx) = UnixStream::pair().map_err(|error| {
        TermError::internal(format!("could not create the term wake socket: {error}"))
    })?;
    wake.set_nonblocking(true).map_err(|error| {
        TermError::internal(format!("could not configure the term wake socket: {error}"))
    })?;
    wake_rx.set_nonblocking(true).map_err(|error| {
        TermError::internal(format!("could not configure the term wake socket: {error}"))
    })?;
    let terminal_settings = crate::settings::DmuxSettings::load(workspace_root).terminal;
    let (policy, _warnings) = Policy::compile(&terminal_settings);
    let approve_timeout = policy.approve_timeout();
    let shared = Arc::new(Shared {
        workspace_root: workspace_root.to_path_buf(),
        sessions: Mutex::new(BTreeMap::new()),
        policy,
        approvals: ApprovalGate::new(approve_timeout),
        shutdown: AtomicBool::new(false),
        wake,
    });
    let mut host = Host {
        shared,
        listener,
        idle_grace,
        journal_flushed: BTreeMap::new(),
        journal_check_after: None,
        wake_rx,
    };
    host.event_loop()
}

impl Host {
    fn event_loop(&mut self) -> Result<(), TermError> {
        let mut idle_since: Option<Instant> = None;
        loop {
            self.flush_journals();
            self.shared.refresh_sessions();
            if self.shared.shutdown.load(Ordering::SeqCst) {
                return Ok(());
            }
            if self.shared.has_live_sessions() {
                idle_since = None;
            } else {
                let since = *idle_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= self.idle_grace {
                    break;
                }
            }

            let timeout = self.next_poll_timeout(idle_since);
            let ready = poll_ready(
                &[self.listener.as_raw_fd(), self.wake_rx.as_raw_fd()],
                timeout,
            )?;
            if ready.get(1).copied().unwrap_or(false) {
                drain_wake(&self.wake_rx);
            }
            if ready.first().copied().unwrap_or(false) {
                loop {
                    match self.listener.accept() {
                        Ok((stream, _)) => self.spawn_connection(stream)?,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) => {
                            return Err(TermError::internal(format!(
                                "could not accept a term connection: {error}"
                            )));
                        }
                    }
                }
            }
            if self.shared.shutdown.load(Ordering::SeqCst) {
                return Ok(());
            }
        }
        Ok(())
    }

    fn spawn_connection(&mut self, stream: UnixStream) -> Result<(), TermError> {
        self.journal_check_after = Some(Instant::now() + JOURNAL_CHECK_WINDOW);
        let shared = Arc::clone(&self.shared);
        thread::Builder::new()
            .name("term-connection".to_owned())
            .spawn(move || shared.handle_connection(stream))
            .map(|_| ())
            .map_err(|error| {
                TermError::internal(format!("could not start a term connection thread: {error}"))
            })
    }

    fn flush_journals(&mut self) {
        let time = now_ms();
        for (name, handle) in self.shared.snapshot_sessions() {
            let session = handle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let last = session.backend.last_output_ms();
            let flushed = self.journal_flushed.get(&name).copied().unwrap_or(0);
            if last <= flushed {
                continue;
            }
            if let Some(journal) = session.backend.journal()
                && let Ok(mut journal) = journal.lock()
            {
                let _ = journal.flush_output_due(time);
            }
            if time >= last.saturating_add(TERM_IO_COALESCE_MAX_DELAY_MS) {
                self.journal_flushed.insert(name, last);
            }
        }
        if self
            .journal_check_after
            .is_some_and(|deadline| deadline <= Instant::now())
        {
            self.journal_check_after = None;
        }
    }

    fn next_poll_timeout(&self, idle_since: Option<Instant>) -> Option<Duration> {
        let now = Instant::now();
        let mut next = idle_since.map(|since| since + self.idle_grace);
        let now_ms = now_ms();
        for (name, handle) in self.shared.snapshot_sessions() {
            let session = handle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let last = session.backend.last_output_ms();
            let flushed = self.journal_flushed.get(&name).copied().unwrap_or(0);
            if last > flushed {
                let due_ms = last.saturating_add(TERM_IO_COALESCE_MAX_DELAY_MS);
                let due = if due_ms <= now_ms {
                    now
                } else {
                    now + Duration::from_millis(due_ms - now_ms)
                };
                next = Some(next.map_or(due, |current| current.min(due)));
            }
        }
        if let Some(check) = self.journal_check_after {
            next = Some(next.map_or(check, |current| current.min(check)));
        }
        next.map(|deadline| {
            deadline
                .checked_duration_since(now)
                .unwrap_or(Duration::ZERO)
        })
    }
}

impl Shared {
    fn snapshot_sessions(&self) -> Vec<(String, SessionHandle)> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(name, session)| (name.clone(), Arc::clone(session)))
            .collect()
    }

    fn refresh_sessions(&self) {
        for (_, handle) in self.snapshot_sessions() {
            let mut session = handle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            session.refresh();
        }
    }

    fn has_live_sessions(&self) -> bool {
        self.snapshot_sessions().into_iter().any(|(_, handle)| {
            let mut session = handle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            session.refresh();
            !session.state.is_terminal()
        })
    }

    fn session(&self, name: &str) -> Result<SessionHandle, TermError> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(name)
            .cloned()
            .ok_or_else(|| TermError::not_found(name))
    }

    fn handle_connection(&self, mut stream: UnixStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
        let allowed = peer_uid(&stream)
            .map(|uid| uid == unsafe { libc::geteuid() })
            .unwrap_or(false);
        if !allowed {
            let error = TermError::new(exit::FORBIDDEN, "term socket peer uid does not match");
            let _ = write_response(&mut stream, &Response::error(&error));
            return;
        }
        let response = match read_request(&mut stream) {
            Ok(request) if request.v == PROTOCOL_VERSION => self.dispatch(request.op),
            Ok(request) => Response::error(&TermError::usage(format!(
                "unsupported term protocol version {}",
                request.v
            ))),
            Err(error) => Response::error(&TermError::internal(format!(
                "invalid term request: {error}"
            ))),
        };
        let _ = write_response(&mut stream, &response);
    }

    fn dispatch(&self, op: RequestOp) -> Response {
        match self.dispatch_op(op) {
            Ok(response) => response,
            Err(error) => Response::error(&error),
        }
    }

    fn dispatch_op(&self, op: RequestOp) -> Result<Response, TermError> {
        match op {
            RequestOp::Open {
                name,
                kind,
                cwd,
                env,
                cols,
                rows,
                argv,
                serial,
            } => self.open(SessionSpec {
                name,
                kind,
                cwd: cwd.map(PathBuf::from),
                env,
                cols,
                rows,
                argv,
                serial,
            }),
            RequestOp::Send {
                name,
                text,
                newline,
                secret,
            } => {
                let size = text.len() + usize::from(newline);
                ensure_input_within_limit(size)?;
                let handle = self.session(&name)?;
                let mut session = handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                session.refresh();
                if session.state != SessionState::Open {
                    return Err(TermError::conflict(format!("session '{name}' is not open")));
                }
                ensure_input_allowed(&name, session.takeover)?;
                self.ensure_policy_allows(&name, &text)?;
                let mut data = text.into_bytes();
                if newline {
                    data.push(b'\n');
                }
                let seq = session.backend.send(&data, secret)?;
                Ok(Response::ok(json!({"seq": seq})))
            }
            RequestOp::SendKeys { name, keys } => {
                let data = parse_key_list(&keys)?;
                let handle = self.session(&name)?;
                let mut session = handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                session.refresh();
                if session.state != SessionState::Open {
                    return Err(TermError::conflict(format!("session '{name}' is not open")));
                }
                ensure_input_allowed(&name, session.takeover)?;
                let seq = session.backend.send(&data, false)?;
                Ok(Response::ok(json!({"seq": seq})))
            }
            RequestOp::Read {
                name,
                since,
                tail,
                raw,
            } => {
                let handle = self.session(&name)?;
                let mut session = handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                session.refresh();
                let read = match tail {
                    Some(lines) => session.backend.read_tail(lines, raw),
                    None => session.backend.read_since(since, raw),
                };
                Ok(Response::ok(json!({
                    "text": read.text,
                    "seq_from": read.seq_from,
                    "seq_to": read.seq_to,
                    "dropped_bytes": read.dropped_bytes,
                })))
            }
            RequestOp::Wait {
                name,
                regex,
                idle_ms,
                timeout_ms,
                since,
            } => {
                let timeout =
                    timeout_ms.ok_or_else(|| TermError::usage("wait requires --timeout"))?;
                let handle = self.session(&name)?;
                let (buffer, since) = {
                    let mut session = handle
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    session.refresh();
                    let since = since.unwrap_or_else(|| session.backend.seq());
                    (session.backend.buffer(), since)
                };
                let options = WaitOptions {
                    regex,
                    idle: idle_ms.map(Duration::from_millis),
                    timeout: Duration::from_millis(timeout),
                    since,
                };
                let result = wait(&buffer, &options)?;
                let data = wait_result_json(&result);
                if result.reason == WaitReason::Timeout {
                    Ok(Response::with_exit_code(data, exit::FAILED))
                } else {
                    Ok(Response::ok(data))
                }
            }
            RequestOp::Exec {
                name,
                command,
                timeout_ms,
            } => self.exec(name, command, timeout_ms),
            RequestOp::List => {
                self.refresh_sessions();
                let sessions: Vec<Value> = self
                    .snapshot_sessions()
                    .into_iter()
                    .map(|(_, handle)| {
                        let mut session = handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        session.refresh();
                        serde_json::to_value(session.info()).unwrap_or(Value::Null)
                    })
                    .collect();
                Ok(Response::ok(json!(sessions)))
            }
            RequestOp::Show { name } => {
                let handle = self.session(&name)?;
                let mut session = handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                session.refresh();
                Ok(Response::ok(
                    serde_json::to_value(session.info()).unwrap_or(Value::Null),
                ))
            }
            RequestOp::Resize { name, cols, rows } => {
                validate_dimensions(cols, rows)?;
                let handle = self.session(&name)?;
                let mut session = handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                session.refresh();
                if session.state != SessionState::Open {
                    return Err(TermError::conflict(format!("session '{name}' is not open")));
                }
                session.backend.resize(cols, rows)?;
                session.spec.cols = cols;
                session.spec.rows = rows;
                Ok(Response::ok(json!({"cols": cols, "rows": rows})))
            }
            RequestOp::Close { name, force, all } => self.close(name, force, all),
            RequestOp::Reopen { name } => self.reopen(&name),
            RequestOp::Approve { id } => {
                approve_with_run_guard(
                    &self.approvals,
                    &id,
                    std::env::var(RUN_ID_ENV).ok().as_deref(),
                    Instant::now(),
                )?;
                Ok(Response::ok(json!({"approved": id})))
            }
            RequestOp::Takeover { name } => {
                let handle = self.session(&name)?;
                let mut session = handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                session.takeover = true;
                session.backend.record_event("takeover");
                Ok(Response::ok(json!({"takeover": true})))
            }
            RequestOp::Release { name } => {
                let handle = self.session(&name)?;
                let mut session = handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                session.takeover = false;
                session.backend.record_event("release");
                Ok(Response::ok(json!({"takeover": false})))
            }
            RequestOp::Shutdown => {
                for (_, handle) in self.snapshot_sessions() {
                    let mut session = handle
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    session.backend.close(true);
                    session.state = SessionState::Closed;
                }
                self.shutdown.store(true, Ordering::SeqCst);
                let _ = (&self.wake).write(&[1]);
                Ok(Response::ok(json!({"state": "closed"})))
            }
        }
    }

    /// FR-2630: evaluate CLI/agent input against the trusted policy. A blocked
    /// input is not written; it creates a one-shot approval and reports the id.
    /// An already-approved identical input on this session is consumed here.
    fn ensure_policy_allows(&self, name: &str, text: &str) -> Result<(), TermError> {
        match self.policy.evaluate(text) {
            Decision::Allowed => Ok(()),
            Decision::ApprovalRequired { matched } => {
                let now = Instant::now();
                if self.approvals.release(name, text, now).is_some() {
                    return Ok(());
                }
                let id = self.approvals.issue(name, text, &matched, now);
                Err(TermError::approval_required(name, &matched, &id))
            }
        }
    }

    fn open(&self, spec: SessionSpec) -> Result<Response, TermError> {
        validate_name(&spec.name)?;
        validate_dimensions(spec.cols, spec.rows)?;
        match spec.kind {
            SessionKind::Cmd if spec.argv.is_empty() => {
                return Err(TermError::usage("--kind cmd needs a command after --"));
            }
            SessionKind::Serial => {
                let config = spec.serial.as_ref().ok_or_else(|| {
                    TermError::usage("serial sessions need a --serial device path")
                })?;
                config.validate()?;
                if !spec.argv.is_empty() {
                    return Err(TermError::usage(
                        "serial sessions do not take a command after --",
                    ));
                }
            }
            _ if spec.serial.is_some() => {
                return Err(TermError::usage(
                    "--serial is only valid for serial sessions",
                ));
            }
            _ => {}
        }
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = sessions.get(&spec.name) {
            let existing = existing
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !existing.state.is_terminal() {
                return Err(TermError::conflict(format!(
                    "session '{}' is already open",
                    spec.name
                )));
            }
        }
        let name = spec.name.clone();
        let session = self.spawn_session(spec)?;
        let data = json!({
            "name": name,
            "kind": kind_name(session.spec.kind),
            "state": "open",
            "pid": session.backend.pid(),
        });
        sessions.remove(&name);
        sessions.insert(name, Arc::new(Mutex::new(session)));
        Ok(Response::ok(data))
    }

    fn spawn_session(&self, spec: SessionSpec) -> Result<Session, TermError> {
        let created_ms = now_ms();
        let (journal, journal_error) = match TermJournal::create(
            &self.workspace_root,
            &spec.name,
            kind_name(spec.kind),
            created_ms,
        ) {
            Ok(journal) => (Some(Arc::new(Mutex::new(journal))), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let backend = Backend::spawn(&spec, &self.workspace_root, journal)?;
        self.spawn_session_watcher(backend.buffer());
        Ok(Session {
            spec,
            state: SessionState::Open,
            backend,
            created_ms,
            takeover: false,
            journal_error,
        })
    }

    fn spawn_session_watcher(&self, buffer: Arc<RingBuffer>) {
        let Ok(wake) = self.wake.try_clone() else {
            return;
        };
        let _ = thread::Builder::new()
            .name("term-session-watch".to_owned())
            .spawn(move || {
                let mut inner = buffer.lock();
                while inner.exit.is_none() {
                    inner = buffer
                        .condvar()
                        .wait(inner)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let _ = (&wake).write(&[1]);
                }
                drop(inner);
                let _ = (&wake).write(&[1]);
            });
    }

    fn exec(
        &self,
        name: String,
        command: String,
        timeout_ms: Option<u64>,
    ) -> Result<Response, TermError> {
        let started = Instant::now();
        let timeout = timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_EXEC_TIMEOUT);
        let (buffer, since, nonce, kind) = {
            let handle = self.session(&name)?;
            let mut session = handle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            session.refresh();
            if session.state != SessionState::Open {
                return Err(TermError::conflict(format!("session '{name}' is not open")));
            }
            ensure_input_allowed(&name, session.takeover)?;
            self.ensure_policy_allows(&name, &command)?;
            let since = session.backend.seq();
            match session.spec.kind {
                SessionKind::Shell => {
                    let nonce = generate_run_id();
                    let text = format!("{command}; {}\n", marker_command(&nonce));
                    check_input_size(text.as_bytes())?;
                    session.backend.send(text.as_bytes(), false)?;
                    (
                        session.backend.buffer(),
                        since,
                        Some(nonce),
                        SessionKind::Shell,
                    )
                }
                SessionKind::Cmd | SessionKind::Serial => {
                    let text = format!("{command}\n");
                    check_input_size(text.as_bytes())?;
                    session.backend.send(text.as_bytes(), false)?;
                    (session.backend.buffer(), since, None, SessionKind::Cmd)
                }
            }
        };

        let (options, nonce) = match (&kind, nonce) {
            (SessionKind::Shell, Some(nonce)) => (
                WaitOptions {
                    regex: Some(format!(
                        "{}-?[0-9]+__",
                        regex::escape(&format!("{MARKER_PREFIX}{nonce}_"))
                    )),
                    idle: None,
                    timeout,
                    since,
                },
                Some(nonce),
            ),
            _ => (
                WaitOptions {
                    regex: None,
                    idle: Some(EXEC_CMD_IDLE),
                    timeout,
                    since,
                },
                None,
            ),
        };
        let result = wait(&buffer, &options)?;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        if result.reason == WaitReason::Timeout {
            return Ok(Response::with_exit_code(
                exec_json(
                    &result,
                    None,
                    elapsed_ms,
                    &command,
                    &nonce,
                    WaitReason::Timeout,
                ),
                exit::FAILED,
            ));
        }
        let marker_code = nonce
            .as_deref()
            .and_then(|nonce| parse_exit_marker(&result.text, nonce));
        let exit_code = marker_code.or_else(|| nonce.as_ref().and(result.exit_code));
        let reason = if marker_code.is_some() {
            WaitReason::Marker
        } else {
            result.reason
        };
        let data = exec_json(&result, exit_code, elapsed_ms, &command, &nonce, reason);
        Ok(Response::ok(data))
    }

    fn close(&self, name: Option<String>, force: bool, all: bool) -> Result<Response, TermError> {
        if all {
            let names: Vec<String> = self
                .snapshot_sessions()
                .into_iter()
                .map(|(name, _)| name)
                .collect();
            for name in &names {
                self.close_session(name, force)?;
            }
            return Ok(Response::ok(json!({"closed": names})));
        }
        let name = name.ok_or_else(|| TermError::usage("close needs a session name or --all"))?;
        self.close_session(&name, force)?;
        Ok(Response::ok(json!({"name": name, "state": "closed"})))
    }

    fn close_session(&self, name: &str, force: bool) -> Result<(), TermError> {
        let handle = self.session(name)?;
        let mut session = handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        session.refresh();
        if session.state == SessionState::Closed {
            return Ok(());
        }
        session.backend.close(force);
        session.state = SessionState::Closed;
        session.backend.record_event("state closed");
        Ok(())
    }

    fn reopen(&self, name: &str) -> Result<Response, TermError> {
        let (spec, handle) = {
            let sessions = self
                .sessions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let handle = sessions
                .get(name)
                .cloned()
                .ok_or_else(|| TermError::not_found(name))?;
            let spec = {
                let session = handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                session.spec.clone()
            };
            (spec, handle)
        };
        if matches!(spec.kind, SessionKind::Serial) {
            let mut session = handle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            session.refresh();
            if let Backend::Serial(serial) = &mut session.backend {
                serial.release();
            }
            session.state = SessionState::Lost;
            session.backend.record_event("state lost");
        }
        let session = self.spawn_session(spec)?;
        let data = json!({
            "name": name,
            "kind": kind_name(session.spec.kind),
            "state": "open",
            "pid": session.backend.pid(),
        });
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        sessions.remove(name);
        sessions.insert(name.to_owned(), Arc::new(Mutex::new(session)));
        Ok(Response::ok(data))
    }
}

fn exec_json(
    result: &WaitResult,
    exit_code: Option<i32>,
    elapsed_ms: u64,
    command: &str,
    nonce: &Option<String>,
    reason: WaitReason,
) -> Value {
    let text = match nonce {
        Some(nonce) => exec_output_text(&result.text, command, nonce),
        None => result.text.clone(),
    };
    json!({
        "text": text,
        "exit_code": exit_code,
        "reason": reason,
        "elapsed_ms": elapsed_ms,
        "seq_from": result.seq_from,
        "seq_to": result.seq_to,
        "dropped_bytes": result.dropped_bytes,
    })
}

fn wait_result_json(result: &WaitResult) -> Value {
    json!({
        "reason": result.reason,
        "text": result.text,
        "seq_from": result.seq_from,
        "seq_to": result.seq_to,
        "dropped_bytes": result.dropped_bytes,
        "exit_code": result.exit_code,
        "signal": result.signal,
    })
}

fn kind_name(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Shell => "shell",
        SessionKind::Cmd => "cmd",
        SessionKind::Serial => "serial",
    }
}

fn ensure_input_allowed(name: &str, takeover: bool) -> Result<(), TermError> {
    if takeover {
        Err(TermError::conflict(format!(
            "session '{name}' is in human takeover; ask the user to release it"
        )))
    } else {
        Ok(())
    }
}

fn prepare_listener(path: &Path) -> Result<UnixListener, TermError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            TermError::internal(format!(
                "could not create the term socket directory {}: {error}",
                parent.display()
            ))
        })?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|error| {
            TermError::internal(format!(
                "could not secure the term socket directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    if path.symlink_metadata().is_ok() {
        if UnixStream::connect(path).is_ok() {
            return Err(TermError::conflict(
                "a terminal session host is already running for this workspace",
            ));
        }
        let _ = fs::remove_file(path);
    }
    let listener = UnixListener::bind(path).map_err(|error| {
        TermError::internal(format!(
            "could not bind the term socket {}: {error}",
            path.display()
        ))
    })?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        TermError::internal(format!(
            "could not secure the term socket {}: {error}",
            path.display()
        ))
    })?;
    listener.set_nonblocking(true).map_err(|error| {
        TermError::internal(format!("could not configure the term socket: {error}"))
    })?;
    Ok(listener)
}

fn poll_ready(fds: &[RawFd], timeout: Option<Duration>) -> Result<Vec<bool>, TermError> {
    let mut pollfds: Vec<libc::pollfd> = fds
        .iter()
        .map(|fd| libc::pollfd {
            fd: *fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    let timeout_ms = timeout
        .map(|duration| duration.as_millis().min(i32::MAX as u128) as i32)
        .unwrap_or(-1);
    loop {
        let result = unsafe {
            libc::poll(
                pollfds.as_mut_ptr(),
                pollfds.len() as libc::nfds_t,
                timeout_ms,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(TermError::internal(format!(
                "could not poll the term socket: {error}"
            )));
        }
        return Ok(pollfds
            .iter()
            .map(|pollfd| pollfd.revents & libc::POLLIN != 0)
            .collect());
    }
}

fn drain_wake(stream: &UnixStream) {
    let mut reader = stream;
    let mut buffer = [0u8; 64];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut credentials as *mut _ as *mut libc::c_void,
            &mut length,
        )
    };
    if result == 0 {
        Ok(credentials.uid)
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn peer_uid(_stream: &UnixStream) -> io::Result<u32> {
    Ok(unsafe { libc::geteuid() })
}

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::policy::DEFAULT_APPROVE_TIMEOUT_MS;

    fn test_shared() -> (Arc<Shared>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let (wake, wake_rx) = UnixStream::pair().unwrap();
        drop(wake_rx);
        let shared = Arc::new(Shared {
            workspace_root: dir.path().to_path_buf(),
            sessions: Mutex::new(BTreeMap::new()),
            policy: Policy::default(),
            approvals: ApprovalGate::new(Duration::from_millis(DEFAULT_APPROVE_TIMEOUT_MS)),
            shutdown: AtomicBool::new(false),
            wake,
        });
        (shared, dir)
    }

    fn insert_cmd_session(shared: &Shared, name: &str, command: &str) {
        let spec = SessionSpec::cmd(
            name,
            vec!["/bin/sh".to_owned(), "-c".to_owned(), command.to_owned()],
        );
        let backend = Backend::spawn(&spec, &shared.workspace_root, None).unwrap();
        let session = Session {
            spec,
            state: SessionState::Open,
            backend,
            created_ms: now_ms(),
            takeover: false,
            journal_error: None,
        };
        shared
            .sessions
            .lock()
            .unwrap()
            .insert(name.to_owned(), Arc::new(Mutex::new(session)));
    }

    #[test]
    fn tc_2693_wait_does_not_block_other_clients() {
        let (shared, _dir) = test_shared();
        insert_cmd_session(&shared, "a", "sleep 30");

        let waiter = {
            let shared = Arc::clone(&shared);
            thread::spawn(move || {
                shared
                    .dispatch_op(RequestOp::Wait {
                        name: "a".to_owned(),
                        regex: Some("never".to_owned()),
                        idle_ms: None,
                        timeout_ms: Some(2_000),
                        since: Some(0),
                    })
                    .unwrap()
            })
        };

        thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        let response = shared
            .dispatch_op(RequestOp::Read {
                name: "a".to_owned(),
                since: 0,
                tail: None,
                raw: true,
            })
            .unwrap();
        let elapsed = started.elapsed();
        assert!(response.ok, "read during wait failed: {response:?}");
        assert!(
            elapsed < Duration::from_millis(300),
            "read during a wait took {elapsed:?}"
        );

        let waited = waiter.join().unwrap();
        assert_eq!(waited.exit_code, Some(exit::FAILED));
        let _ = shared.dispatch_op(RequestOp::Close {
            name: Some("a".to_owned()),
            force: true,
            all: false,
        });
    }

    #[test]
    fn tc_2609_takeover_lock_rejects_input_until_release() {
        let error = ensure_input_allowed("dev", true).unwrap_err();
        assert_eq!(error.code, exit::CONFLICT);
        assert_eq!(error.error_kind, Some("conflict"));
        assert_eq!(
            error.message,
            "session 'dev' is in human takeover; ask the user to release it"
        );
        assert!(ensure_input_allowed("dev", false).is_ok());
    }
}
