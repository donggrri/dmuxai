#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::run_journal::generate_run_id;
use crate::term::backend::{Backend, now_ms};
use crate::term::client::socket_path;
use crate::term::journal::TermJournal;
use crate::term::proto::{PROTOCOL_VERSION, RequestOp, Response, read_request, write_response};
use crate::term::wait::{
    MARKER_PREFIX, WaitOptions, WaitReason, WaitResult, marker_command, parse_exit_marker,
    strip_exit_marker, wait,
};
use crate::term::{
    SessionInfo, SessionKind, SessionSpec, SessionState, TermError, check_input_size,
    ensure_input_within_limit, exit, parse_key_list, validate_dimensions, validate_name,
};

const DEFAULT_IDLE_GRACE: Duration = Duration::from_secs(600);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const EXEC_CMD_IDLE: Duration = Duration::from_millis(200);
const DEFAULT_EXEC_TIMEOUT: Duration = Duration::from_secs(30);

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
        if self.state == SessionState::Open
            && let Some(exit) = self.backend.exit_status()
        {
            self.state = SessionState::Exited;
            let _ = exit;
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

pub struct Host {
    workspace_root: PathBuf,
    listener: UnixListener,
    sessions: BTreeMap<String, Session>,
    idle_grace: Duration,
    shutdown: bool,
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
    let mut host = Host {
        workspace_root: workspace_root.to_path_buf(),
        listener,
        sessions: BTreeMap::new(),
        idle_grace,
        shutdown: false,
    };
    host.event_loop()
}

impl Host {
    fn event_loop(&mut self) -> Result<(), TermError> {
        let mut idle_since: Option<Instant> = None;
        loop {
            self.flush_journals();
            self.refresh_sessions();
            if self
                .sessions
                .values()
                .any(|session| !session.state.is_terminal())
            {
                idle_since = None;
            } else {
                let since = *idle_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= self.idle_grace {
                    break;
                }
            }

            if poll_readable(self.listener.as_raw_fd(), POLL_INTERVAL)? {
                loop {
                    match self.listener.accept() {
                        Ok((stream, _)) => self.handle_connection(stream),
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) => {
                            return Err(TermError::internal(format!(
                                "could not accept a term connection: {error}"
                            )));
                        }
                    }
                    if self.shutdown {
                        return Ok(());
                    }
                }
            }
            if self.shutdown {
                return Ok(());
            }
        }
        Ok(())
    }

    fn refresh_sessions(&mut self) {
        for session in self.sessions.values_mut() {
            session.refresh();
        }
    }

    fn flush_journals(&self) {
        let time = now_ms();
        for session in self.sessions.values() {
            if let Some(journal) = session.backend.journal()
                && let Ok(mut journal) = journal.lock()
            {
                let _ = journal.flush_output_due(time);
            }
        }
    }

    fn handle_connection(&mut self, mut stream: UnixStream) {
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

    fn dispatch(&mut self, op: RequestOp) -> Response {
        match self.dispatch_op(op) {
            Ok(response) => response,
            Err(error) => Response::error(&error),
        }
    }

    fn dispatch_op(&mut self, op: RequestOp) -> Result<Response, TermError> {
        match op {
            RequestOp::Open {
                name,
                kind,
                cwd,
                env,
                cols,
                rows,
                argv,
            } => self.open(SessionSpec {
                name,
                kind,
                cwd: cwd.map(PathBuf::from),
                env,
                cols,
                rows,
                argv,
            }),
            RequestOp::Send {
                name,
                text,
                newline,
                secret,
            } => {
                let mut data = text.into_bytes();
                if newline {
                    data.push(b'\n');
                }
                ensure_input_within_limit(data.len())?;
                let session = self.open_session_mut(&name)?;
                ensure_input_allowed(&name, session.takeover)?;
                let seq = session.backend.send(&data, secret)?;
                Ok(Response::ok(json!({"seq": seq})))
            }
            RequestOp::SendKeys { name, keys } => {
                let data = parse_key_list(&keys)?;
                let session = self.open_session_mut(&name)?;
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
                let session = self.session_mut(&name)?;
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
                let options = WaitOptions {
                    regex,
                    idle: idle_ms.map(Duration::from_millis),
                    timeout: Duration::from_millis(timeout),
                    since,
                };
                let session = self.session_mut(&name)?;
                session.refresh();
                let buffer = session.backend.buffer();
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
                    .sessions
                    .values()
                    .map(|session| serde_json::to_value(session.info()).unwrap_or(Value::Null))
                    .collect();
                Ok(Response::ok(json!(sessions)))
            }
            RequestOp::Show { name } => {
                let session = self.session_mut(&name)?;
                session.refresh();
                Ok(Response::ok(
                    serde_json::to_value(session.info()).unwrap_or(Value::Null),
                ))
            }
            RequestOp::Resize { name, cols, rows } => {
                validate_dimensions(cols, rows)?;
                let session = self.open_session_mut(&name)?;
                session.backend.resize(cols, rows)?;
                session.spec.cols = cols;
                session.spec.rows = rows;
                Ok(Response::ok(json!({"cols": cols, "rows": rows})))
            }
            RequestOp::Close { name, force, all } => self.close(name, force, all),
            RequestOp::Reopen { name } => self.reopen(&name),
            RequestOp::Takeover { name } => {
                let session = self.session_mut(&name)?;
                session.takeover = true;
                session.backend.record_event("takeover");
                Ok(Response::ok(json!({"takeover": true})))
            }
            RequestOp::Release { name } => {
                let session = self.session_mut(&name)?;
                session.takeover = false;
                session.backend.record_event("release");
                Ok(Response::ok(json!({"takeover": false})))
            }
            RequestOp::Shutdown => {
                for session in self.sessions.values_mut() {
                    session.backend.close(true);
                    session.state = SessionState::Closed;
                }
                self.shutdown = true;
                Ok(Response::ok(json!({"state": "closed"})))
            }
        }
    }

    fn open(&mut self, spec: SessionSpec) -> Result<Response, TermError> {
        validate_name(&spec.name)?;
        validate_dimensions(spec.cols, spec.rows)?;
        if spec.kind == SessionKind::Cmd && spec.argv.is_empty() {
            return Err(TermError::usage("--kind cmd needs a command after --"));
        }
        if let Some(existing) = self.sessions.get(&spec.name)
            && !existing.state.is_terminal()
        {
            return Err(TermError::conflict(format!(
                "session '{}' is already open",
                spec.name
            )));
        }
        self.sessions.remove(&spec.name);
        let name = spec.name.clone();
        let session = self.spawn_session(spec)?;
        let data = json!({
            "name": name,
            "kind": kind_name(session.spec.kind),
            "state": "open",
            "pid": session.backend.pid(),
        });
        self.sessions.insert(name, session);
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
        Ok(Session {
            spec,
            state: SessionState::Open,
            backend,
            created_ms,
            takeover: false,
            journal_error,
        })
    }

    fn exec(
        &mut self,
        name: String,
        command: String,
        timeout_ms: Option<u64>,
    ) -> Result<Response, TermError> {
        let started = Instant::now();
        let timeout = timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_EXEC_TIMEOUT);
        let (buffer, since, nonce, kind) = {
            let session = self.open_session_mut(&name)?;
            ensure_input_allowed(&name, session.takeover)?;
            let since = session.backend.seq();
            match session.spec.kind {
                SessionKind::Shell => {
                    let nonce = generate_run_id();
                    let text = format!("{command}\n{}\n", marker_command(&nonce));
                    check_input_size(text.as_bytes())?;
                    session.backend.send(text.as_bytes(), false)?;
                    (
                        session.backend.buffer(),
                        since,
                        Some(nonce),
                        SessionKind::Shell,
                    )
                }
                SessionKind::Cmd => {
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
                exec_json(&result, None, elapsed_ms, &nonce, WaitReason::Timeout),
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
        let data = exec_json(&result, exit_code, elapsed_ms, &nonce, reason);
        Ok(Response::ok(data))
    }

    fn close(
        &mut self,
        name: Option<String>,
        force: bool,
        all: bool,
    ) -> Result<Response, TermError> {
        if all {
            let names: Vec<String> = self.sessions.keys().cloned().collect();
            for name in &names {
                self.close_session(name, force)?;
            }
            return Ok(Response::ok(json!({"closed": names})));
        }
        let name = name.ok_or_else(|| TermError::usage("close needs a session name or --all"))?;
        self.close_session(&name, force)?;
        Ok(Response::ok(json!({"name": name, "state": "closed"})))
    }

    fn close_session(&mut self, name: &str, force: bool) -> Result<(), TermError> {
        let session = self.session_mut(name)?;
        session.refresh();
        if session.state == SessionState::Closed {
            return Ok(());
        }
        session.backend.close(force);
        session.state = SessionState::Closed;
        session.backend.record_event("state closed");
        Ok(())
    }

    fn reopen(&mut self, name: &str) -> Result<Response, TermError> {
        let spec = self.session(name)?.spec.clone();
        self.sessions.remove(name);
        let session = self.spawn_session(spec)?;
        let data = json!({
            "name": name,
            "kind": kind_name(session.spec.kind),
            "state": "open",
            "pid": session.backend.pid(),
        });
        self.sessions.insert(name.to_owned(), session);
        Ok(Response::ok(data))
    }

    fn session(&self, name: &str) -> Result<&Session, TermError> {
        self.sessions
            .get(name)
            .ok_or_else(|| TermError::not_found(name))
    }

    fn session_mut(&mut self, name: &str) -> Result<&mut Session, TermError> {
        self.sessions
            .get_mut(name)
            .ok_or_else(|| TermError::not_found(name))
    }

    fn open_session_mut(&mut self, name: &str) -> Result<&mut Session, TermError> {
        let session = self.session_mut(name)?;
        session.refresh();
        if session.state != SessionState::Open {
            return Err(TermError::conflict(format!("session '{name}' is not open")));
        }
        Ok(session)
    }
}

fn exec_json(
    result: &WaitResult,
    exit_code: Option<i32>,
    elapsed_ms: u64,
    nonce: &Option<String>,
    reason: WaitReason,
) -> Value {
    let text = match nonce {
        Some(nonce) => strip_exit_marker(&result.text, nonce),
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

fn poll_readable(fd: RawFd, timeout: Duration) -> Result<bool, TermError> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        let result = unsafe { libc::poll(&mut pollfd, 1, timeout.as_millis() as i32) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(TermError::internal(format!(
                "could not poll the term socket: {error}"
            )));
        }
        return Ok(result > 0);
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
