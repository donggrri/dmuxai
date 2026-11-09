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
use crate::process::supervisor::{GroupSignal, group_alive, signal_group};
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
            if force {
                let _ = signal_group(pgid, GroupSignal::Kill);
            } else {
                signal_hup(pgid);
                let deadline = Instant::now() + Duration::from_secs(2);
                while Instant::now() < deadline {
                    if !group_alive(pgid) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                if group_alive(pgid) {
                    let _ = signal_group(pgid, GroupSignal::Kill);
                }
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
