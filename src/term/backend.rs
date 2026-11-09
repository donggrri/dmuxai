#![cfg(unix)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use portable_pty::ChildKiller;
use portable_pty::{CommandBuilder, MasterPty, PtyPair, PtySize, native_pty_system};

use crate::process::INTERNAL_ENV_VARS;
use crate::process::supervisor::{TreeSnapshot, group_alive, snapshot_tree, terminate_snapshot};
use crate::term::buffer::{BufferRead, DEFAULT_CAPACITY, RingBuffer};
use crate::term::journal::{TermJournal, TermJournalDir};
use crate::term::{
    ExitStatus, SessionKind, SessionSpec, TermError, check_input_size, validate_dimensions,
};

pub struct Backend {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    pgid: Option<u32>,
    buffer: Arc<RingBuffer>,
    last_output_ms: Arc<AtomicU64>,
    journal: Option<Arc<Mutex<TermJournal>>>,
}

impl Backend {
    pub fn spawn(
        spec: &SessionSpec,
        workspace_root: &Path,
        journal: Option<Arc<Mutex<TermJournal>>>,
    ) -> Result<Self, TermError> {
        validate_dimensions(spec.cols, spec.rows)?;
        if spec.kind == SessionKind::Cmd && spec.argv.is_empty() {
            return Err(TermError::usage("cmd sessions need a command after --"));
        }

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: spec.rows,
                cols: spec.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TermError::internal(format!("could not open a PTY: {error}")))?;
        let PtyPair { slave, master } = pair;

        let mut command = match spec.kind {
            SessionKind::Shell => {
                let shell = std::env::var("SHELL")
                    .ok()
                    .filter(|shell| !shell.is_empty())
                    .unwrap_or_else(|| "/bin/sh".to_owned());
                let mut command = CommandBuilder::new(shell);
                command.arg("-l");
                command
            }
            SessionKind::Cmd => {
                let mut command = CommandBuilder::new(&spec.argv[0]);
                command.args(&spec.argv[1..]);
                command
            }
        };
        for variable in INTERNAL_ENV_VARS {
            command.env_remove(variable);
        }
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        if command.get_env("TERM").is_none() {
            command.env("TERM", "xterm-256color");
        }
        let cwd: PathBuf = spec
            .cwd
            .clone()
            .unwrap_or_else(|| workspace_root.to_path_buf());
        command.cwd(cwd);

        let mut child = slave.spawn_command(command).map_err(|error| {
            TermError::internal(format!("could not start the session: {error}"))
        })?;
        drop(slave);

        let pid = child.process_id();
        let pgid = master
            .process_group_leader()
            .and_then(|pgid| u32::try_from(pgid).ok())
            .filter(|pgid| *pgid > 1);
        let mut reader = master
            .try_clone_reader()
            .map_err(|error| TermError::internal(format!("could not read the PTY: {error}")))?;
        let writer = master
            .take_writer()
            .map_err(|error| TermError::internal(format!("could not write the PTY: {error}")))?;
        let killer = child.clone_killer();

        let buffer = Arc::new(RingBuffer::new(DEFAULT_CAPACITY));
        let last_output_ms = Arc::new(AtomicU64::new(now_ms()));
        let reader_buffer = Arc::clone(&buffer);
        let reader_last_output = Arc::clone(&last_output_ms);
        let reader_journal = journal.clone();
        let name = spec.name.clone();
        thread::Builder::new()
            .name(format!("term-reader-{name}"))
            .spawn(move || {
                let mut chunk = vec![0u8; 8192];
                loop {
                    match reader.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(read) => {
                            let data = &chunk[..read];
                            let time = now_ms();
                            let seq = reader_buffer.push(data);
                            reader_last_output.store(time, Ordering::Relaxed);
                            if let Some(journal) = &reader_journal
                                && let Ok(mut journal) = journal.lock()
                            {
                                let text = String::from_utf8_lossy(data);
                                let _ = journal.record_output(seq, time, "child", None, &text);
                                let _ = journal.flush_output_due(time);
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                let exit = child.wait().map(convert_exit).unwrap_or_default();
                reader_buffer.set_exited(exit);
                if let Some(journal) = &reader_journal
                    && let Ok(mut journal) = journal.lock()
                {
                    let _ = journal.flush_output();
                    let _ = journal.record_event(
                        reader_buffer.seq(),
                        now_ms(),
                        TermJournalDir::Out,
                        "child",
                        None,
                        "state exited",
                    );
                }
            })
            .map_err(|error| {
                TermError::internal(format!("could not start the PTY reader: {error}"))
            })?;

        Ok(Self {
            master: Mutex::new(master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            pid,
            pgid,
            buffer,
            last_output_ms,
            journal,
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn seq(&self) -> u64 {
        self.buffer.seq()
    }

    pub fn buffer(&self) -> Arc<RingBuffer> {
        Arc::clone(&self.buffer)
    }

    pub fn last_output_ms(&self) -> u64 {
        self.last_output_ms.load(Ordering::Relaxed)
    }

    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.buffer.exit_status()
    }

    pub fn read_since(&self, since: u64, raw: bool) -> BufferRead {
        self.buffer.read_since(since, raw)
    }

    pub fn read_tail(&self, lines: usize, raw: bool) -> BufferRead {
        self.buffer.read_tail(lines, raw)
    }

    pub fn send(&self, data: &[u8], secret: bool) -> Result<u64, TermError> {
        check_input_size(data)?;
        let seq = self.buffer.seq();
        {
            let mut writer = self.lock_writer();
            writer.write_all(data).map_err(|error| {
                TermError::internal(format!("could not write to the PTY: {error}"))
            })?;
            writer.flush().map_err(|error| {
                TermError::internal(format!("could not flush the PTY: {error}"))
            })?;
        }
        if let Some(journal) = &self.journal
            && let Ok(mut journal) = journal.lock()
        {
            let text = String::from_utf8_lossy(data);
            let _ = journal.record_input(seq, now_ms(), "cli", None, &text, secret);
        }
        Ok(seq)
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), TermError> {
        validate_dimensions(cols, rows)?;
        let master = self
            .master
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TermError::internal(format!("could not resize the PTY: {error}")))?;
        drop(master);
        self.record_event(&format!("resize {cols}x{rows}"));
        Ok(())
    }

    pub fn close(&self, force: bool) {
        if let Some(pgid) = self.pgid {
            // Snapshot before signalling: an interactive shell with job control
            // puts background jobs in their own process group, and once the
            // root exits those descendants may no longer be discoverable.
            let snapshot = snapshot_tree(pgid);
            signal_snapshot_hup(&snapshot, pgid);
            let grace = if force {
                Duration::ZERO
            } else {
                Duration::from_secs(2)
            };
            let deadline = Instant::now() + grace;
            while Instant::now() < deadline {
                if !snapshot_alive(&snapshot, pgid) {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            if snapshot_alive(&snapshot, pgid) {
                // #230 pattern: kill every member recorded in the snapshot,
                // including descendants that escaped into another group.
                terminate_snapshot_blocking(&snapshot, pgid);
            }
        }
        if let Ok(mut killer) = self.killer.lock() {
            let _ = killer.kill();
        }
    }

    pub fn journal(&self) -> Option<Arc<Mutex<TermJournal>>> {
        self.journal.clone()
    }

    pub fn journal_error(&self) -> Option<String> {
        self.journal.as_ref().and_then(|journal| {
            journal
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .last_error()
                .map(str::to_owned)
        })
    }

    pub fn record_event(&self, data: &str) {
        if let Some(journal) = &self.journal
            && let Ok(mut journal) = journal.lock()
        {
            let _ = journal.record_event(
                self.buffer.seq(),
                now_ms(),
                TermJournalDir::Out,
                "cli",
                None,
                data,
            );
        }
    }

    fn lock_writer(&self) -> std::sync::MutexGuard<'_, Box<dyn Write + Send>> {
        self.writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.close(true);
    }
}

fn snapshot_groups(snapshot: &TreeSnapshot, root_pgid: u32) -> std::collections::BTreeSet<u32> {
    let mut groups = std::collections::BTreeSet::new();
    if root_pgid > 1 && root_pgid <= i32::MAX as u32 {
        groups.insert(root_pgid);
    }
    for process in &snapshot.processes {
        if process.pgid > 1 && process.pgid <= i32::MAX as u32 {
            groups.insert(process.pgid);
        }
    }
    groups
}

fn signal_snapshot_hup(snapshot: &TreeSnapshot, root_pgid: u32) {
    for pgid in snapshot_groups(snapshot, root_pgid) {
        signal_hup(pgid);
    }
}

fn snapshot_alive(snapshot: &TreeSnapshot, root_pgid: u32) -> bool {
    snapshot_groups(snapshot, root_pgid)
        .into_iter()
        .any(group_alive)
}

fn terminate_snapshot_blocking(snapshot: &TreeSnapshot, root_pgid: u32) {
    let snapshot = snapshot.clone();
    let terminate = move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
        else {
            return;
        };
        runtime.block_on(terminate_snapshot(&snapshot, root_pgid, Duration::ZERO));
    };
    if tokio::runtime::Handle::try_current().is_ok() {
        // A nested block_on would panic on a runtime worker, so run the
        // supervisor future on a plain thread instead.
        let _ = thread::spawn(terminate).join();
    } else {
        terminate();
    }
}

fn signal_hup(pgid: u32) -> bool {
    if pgid <= 1 || pgid > i32::MAX as u32 {
        return false;
    }
    let own_pgid = unsafe { libc::getpgrp() };
    if pgid as i32 == own_pgid {
        return false;
    }
    unsafe { libc::killpg(pgid as i32, libc::SIGHUP) == 0 }
}

fn convert_exit(status: portable_pty::ExitStatus) -> ExitStatus {
    match status.signal() {
        Some(name) => ExitStatus {
            code: None,
            signal: Some(signal_number(name)),
        },
        None => ExitStatus::from_code(status.exit_code() as i32),
    }
}

fn signal_number(name: &str) -> i32 {
    match name.to_ascii_lowercase().as_str() {
        "hangup" | "sighup" => 1,
        "interrupt" | "sigint" => 2,
        "quit" | "sigquit" => 3,
        "aborted" | "sigabrt" => 6,
        "killed" | "sigkill" => 9,
        "terminated" | "sigterm" => 15,
        _ => 0,
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn tc_2607_close_kills_background_child_in_other_group() {
        let workspace = std::env::temp_dir();
        let spec = SessionSpec::cmd("jobcontrol", vec!["/bin/sh".into(), "-i".into()]);
        let backend = Backend::spawn(&spec, &workspace, None).expect("spawn session");
        let root_pid = backend.pid().expect("session pid");
        let root_pgid = backend.pgid.expect("session pgid");

        backend
            .send(b"set -m; sleep 100 & echo CHILD_$!\n", false)
            .expect("send job-control command");
        let sleep_pid =
            wait_for_child_pid(&backend, Duration::from_secs(5)).expect("background child pid");
        let child_pgid = proc_pgid(sleep_pid).expect("child /proc stat");
        assert_ne!(
            child_pgid, root_pgid,
            "job control should put the background child in its own group"
        );
        assert_eq!(
            child_pgid, sleep_pid,
            "background child should lead its own process group"
        );

        backend.close(false);

        let child_gone = wait_for_exit(sleep_pid, Duration::from_secs(3));
        let root_gone = wait_for_exit(root_pid, Duration::from_secs(3));
        if !child_gone {
            unsafe {
                libc::kill(sleep_pid as i32, libc::SIGKILL);
            }
        }
        assert!(child_gone, "close left background child {sleep_pid} alive");
        assert!(root_gone, "close left session child {root_pid} alive");
    }

    #[cfg(target_os = "linux")]
    fn wait_for_child_pid(backend: &Backend, limit: Duration) -> Option<u32> {
        let deadline = Instant::now() + limit;
        loop {
            let read = backend.read_since(0, true);
            if let Some(pid) = child_pid_in(&read.text) {
                return Some(pid);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(target_os = "linux")]
    fn child_pid_in(text: &str) -> Option<u32> {
        for (index, _) in text.match_indices("CHILD_") {
            let rest = &text[index + "CHILD_".len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(pid) = digits.parse() {
                return Some(pid);
            }
        }
        None
    }

    #[cfg(target_os = "linux")]
    fn proc_pgid(pid: u32) -> Option<u32> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let rest = &stat[stat.rfind(')')? + 1..];
        let mut fields = rest.split_whitespace();
        let _state = fields.next()?;
        let _ppid = fields.next()?;
        fields.next()?.parse().ok()
    }

    #[cfg(target_os = "linux")]
    fn wait_for_exit(pid: u32, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if !process_alive(pid) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(target_os = "linux")]
    fn process_alive(pid: u32) -> bool {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        let rest = match stat.rfind(')') {
            Some(index) => &stat[index + 1..],
            None => return false,
        };
        let state = rest.split_whitespace().next().unwrap_or_default();
        state != "Z"
    }
}
