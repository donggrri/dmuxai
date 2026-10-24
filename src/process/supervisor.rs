//! Shared runner process supervisor (PR-5).
//!
//! Spawns runner children in isolated process groups, watches their stdout/stderr
//! with idle and total timeout watchdogs, drains stderr without blocking, and ensures
//! clean group termination.

use std::time::Duration;

#[cfg(unix)]
pub use tokio::process::ChildStdin;

/// Signal to send to a process group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupSignal {
    Term,
    Kill,
    Int,
}

/// Options for configuring a spawned child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildOptions {
    /// Whether to configure `PR_SET_PDEATHSIG` on Linux so the child receives `SIGTERM`
    /// when the supervisor dies. Default is `true`.
    pub parent_death_signal: bool,
}

impl Default for ChildOptions {
    fn default() -> Self {
        Self {
            parent_death_signal: true,
        }
    }
}

/// Configuration for child process supervision and watchdogs.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    pub label: String,
    pub idle_timeout: Option<Duration>,
    pub total_timeout: Option<Duration>,
    pub stderr_tail_bytes: usize,
    pub term_grace: Duration,
    pub stderr_drain_grace: Duration,
    pub child_options: ChildOptions,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            label: String::new(),
            idle_timeout: None,
            total_timeout: None,
            stderr_tail_bytes: 64 * 1024,
            term_grace: Duration::from_secs(2),
            stderr_drain_grace: Duration::from_secs(2),
            child_options: ChildOptions::default(),
        }
    }
}

impl SupervisorConfig {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            ..Self::default()
        }
    }
}

/// Event yielded by line-oriented stdout streaming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineEvent {
    Line(String),
    Eof,
    Idle(Duration),
    Timeout(Duration),
}

/// Formats the standard idle watchdog error message.
pub fn idle_error_message(label: &str, secs: u64) -> String {
    format!(
        "{label} produced no output for {secs}s; the provider may be stuck (for example a usage limit). Check the provider, or raise idle_timeout_secs (0 disables it)."
    )
}

/// Formats the standard total timeout error message.
pub fn timeout_error_message(label: &str, secs: u64) -> String {
    format!("{label} timed out after {secs}s")
}

#[cfg(unix)]
static LIVE_GROUPS: std::sync::Mutex<std::collections::BTreeSet<u32>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

#[cfg(unix)]
fn register_group(pgid: u32) {
    if pgid > 1 {
        let mut groups = LIVE_GROUPS.lock().unwrap_or_else(|e| e.into_inner());
        groups.insert(pgid);
    }
}

#[cfg(unix)]
fn unregister_group(pgid: u32) {
    let mut groups = LIVE_GROUPS.lock().unwrap_or_else(|e| e.into_inner());
    groups.remove(&pgid);
}

/// Returns the process group IDs of all currently live supervised children.
#[cfg(unix)]
pub fn live_groups() -> Vec<u32> {
    let groups = LIVE_GROUPS.lock().unwrap_or_else(|e| e.into_inner());
    groups.iter().copied().collect()
}

/// Signals every registered process group with `SIGTERM`, waits up to `grace`
/// for group leaders to exit, then sends `SIGKILL` to groups whose leader is still alive.
#[cfg(unix)]
pub fn stop_all_live_groups(grace: Duration) {
    let groups = live_groups();
    for &pgid in &groups {
        signal_group(pgid, GroupSignal::Term);
    }

    if !grace.is_zero() {
        let start = std::time::Instant::now();
        let poll_interval = Duration::from_millis(20);
        while start.elapsed() < grace {
            let any_alive = groups.iter().any(|&pgid| is_process_alive(pgid));
            if !any_alive {
                break;
            }
            std::thread::sleep(poll_interval.min(grace.saturating_sub(start.elapsed())));
        }
    }

    for &pgid in &groups {
        if is_process_alive(pgid) {
            signal_group(pgid, GroupSignal::Kill);
        }
    }
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    if pid <= 1 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: libc::kill with signal 0 checks for process existence without sending a signal.
    let res = unsafe { libc::kill(pid as i32, 0) };
    if res == 0 {
        #[cfg(target_os = "linux")]
        if let Ok(content) = std::fs::read_to_string(format!("/proc/{pid}/status"))
            && content.lines().any(|l| l.starts_with("State:\tZ"))
        {
            return false;
        }
        true
    } else {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        errno != libc::ESRCH
    }
}

/// Sends a signal to a process group via `libc::killpg`.
///
/// Returns `false` and does nothing when `pgid` is 0 or 1, exceeds `i32::MAX`,
/// or equals the supervisor's own process group (`libc::getpgrp()`).
/// Returns `true` when `libc::killpg` returns 0.
#[cfg(unix)]
pub fn signal_group(pgid: u32, signal: GroupSignal) -> bool {
    if pgid <= 1 || pgid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: libc::getpgrp is an async-signal-safe POSIX function that gets the caller process group.
    let own_pgid = unsafe { libc::getpgrp() };
    if pgid as i32 == own_pgid {
        return false;
    }

    let sig = match signal {
        GroupSignal::Term => libc::SIGTERM,
        GroupSignal::Kill => libc::SIGKILL,
        GroupSignal::Int => libc::SIGINT,
    };

    // SAFETY: libc::killpg sends the signal to the specified process group.
    // We validated pgid > 1, pgid <= i32::MAX, and pgid != own_pgid, preventing accidental suicide
    // or broadcast signaling across the system.
    let res = unsafe { libc::killpg(pgid as i32, sig) };
    res == 0
}

/// Configures a Tokio process command for supervised execution:
/// puts the child into its own process group (`process_group(0)`, so child pgid == child pid),
/// sets `kill_on_drop(true)`, and on Linux when `opts.parent_death_signal` is enabled,
/// installs a `pre_exec` hook that calls `libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM)`
/// and verifies the parent process is still alive.
///
/// **Important:** Spawn commands configured with `parent_death_signal` from an async task,
/// NOT from `tokio::task::spawn_blocking` (PDEATHSIG fires when the spawning thread exits).
#[cfg(unix)]
pub fn configure_child(cmd: &mut tokio::process::Command, opts: &ChildOptions) {
    cmd.process_group(0);
    cmd.kill_on_drop(true);

    #[cfg(target_os = "linux")]
    if opts.parent_death_signal {
        use std::os::unix::process::CommandExt;

        // SAFETY: libc::getpid is an async-signal-safe POSIX call that returns the calling PID.
        let parent_pid = unsafe { libc::getpid() };

        // SAFETY: pre_exec runs between fork and exec in the child process.
        // We only invoke async-signal-safe functions (prctl, getppid, _exit) without
        // memory allocations or locking.
        unsafe {
            cmd.as_std_mut().pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    libc::_exit(1);
                }
                if libc::getppid() != parent_pid {
                    libc::_exit(1);
                }
                Ok(())
            });
        }
    }
}

/// A supervised runner process owning the child, stdout reader, and stderr drain buffer.
#[cfg(unix)]
pub struct Supervised {
    child: tokio::process::Child,
    pgid: u32,
    reader: tokio::io::BufReader<tokio::process::ChildStdout>,
    line_buf: String,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
    stderr_abort: tokio::task::AbortHandle,
    stderr_tail_buf: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<u8>>>,
    config: SupervisorConfig,
    spawn_time: std::time::Instant,
    last_activity: std::time::Instant,
    leader_exit_status: Option<std::process::ExitStatus>,
    reaped_status: Option<std::process::ExitStatus>,
}

#[cfg(unix)]
impl Supervised {
    /// Returns the process group ID of the child.
    pub fn pgid(&self) -> u32 {
        self.pgid
    }

    /// Returns the process ID of the child leader, if still available from Tokio.
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Takes the child's stdin handle if it was configured as piped.
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    /// Reads the next stdout line or watchdog event.
    ///
    /// Yields `LineEvent::Idle` if no line arrives within `idle_timeout` since spawn
    /// or since the last line. Each line resets the idle timer.
    /// Yields `LineEvent::Timeout` if the child's total lifetime exceeds `total_timeout`.
    pub async fn next_line(&mut self) -> std::io::Result<LineEvent> {
        use tokio::io::AsyncBufReadExt;

        let now = std::time::Instant::now();

        if let Some(dur) = self.config.total_timeout
            && now >= self.spawn_time + dur
        {
            return Ok(LineEvent::Timeout(dur));
        }

        if let Some(dur) = self.config.idle_timeout
            && now >= self.last_activity + dur
        {
            return Ok(LineEvent::Idle(dur));
        }

        if self.leader_exit_status.is_none()
            && let Ok(Some(st)) = self.child.try_wait()
        {
            self.leader_exit_status = Some(st);
        }

        if self.leader_exit_status.is_some() && !self.has_unread_stdout() {
            return Ok(LineEvent::Eof);
        }

        let total_fut = self
            .config
            .total_timeout
            .map(|dur| tokio::time::sleep_until((self.spawn_time + dur).into()));
        let idle_fut = self
            .config
            .idle_timeout
            .map(|dur| tokio::time::sleep_until((self.last_activity + dur).into()));

        match (total_fut, idle_fut) {
            (Some(total_sleep), Some(idle_sleep)) => {
                tokio::select! {
                    biased;
                    _ = total_sleep => Ok(LineEvent::Timeout(self.config.total_timeout.unwrap())),
                    _ = idle_sleep => Ok(LineEvent::Idle(self.config.idle_timeout.unwrap())),
                    res = self.reader.read_line(&mut self.line_buf) => {
                        self.handle_line_read(res)
                    }
                    child_res = self.child.wait() => {
                        let st = match child_res {
                            Ok(s) => s,
                            Err(_) => exit_status_from_raw(0),
                        };
                        self.leader_exit_status = Some(st);
                        if self.has_unread_stdout() {
                            let res = self.reader.read_line(&mut self.line_buf).await;
                            self.handle_line_read(res)
                        } else {
                            Ok(LineEvent::Eof)
                        }
                    }
                }
            }
            (Some(total_sleep), None) => {
                tokio::select! {
                    biased;
                    _ = total_sleep => Ok(LineEvent::Timeout(self.config.total_timeout.unwrap())),
                    res = self.reader.read_line(&mut self.line_buf) => {
                        self.handle_line_read(res)
                    }
                    child_res = self.child.wait() => {
                        let st = match child_res {
                            Ok(s) => s,
                            Err(_) => exit_status_from_raw(0),
                        };
                        self.leader_exit_status = Some(st);
                        if self.has_unread_stdout() {
                            let res = self.reader.read_line(&mut self.line_buf).await;
                            self.handle_line_read(res)
                        } else {
                            Ok(LineEvent::Eof)
                        }
                    }
                }
            }
            (None, Some(idle_sleep)) => {
                tokio::select! {
                    biased;
                    _ = idle_sleep => Ok(LineEvent::Idle(self.config.idle_timeout.unwrap())),
                    res = self.reader.read_line(&mut self.line_buf) => {
                        self.handle_line_read(res)
                    }
                    child_res = self.child.wait() => {
                        let st = match child_res {
                            Ok(s) => s,
                            Err(_) => exit_status_from_raw(0),
                        };
                        self.leader_exit_status = Some(st);
                        if self.has_unread_stdout() {
                            let res = self.reader.read_line(&mut self.line_buf).await;
                            self.handle_line_read(res)
                        } else {
                            Ok(LineEvent::Eof)
                        }
                    }
                }
            }
            (None, None) => {
                tokio::select! {
                    biased;
                    res = self.reader.read_line(&mut self.line_buf) => {
                        self.handle_line_read(res)
                    }
                    child_res = self.child.wait() => {
                        let st = match child_res {
                            Ok(s) => s,
                            Err(_) => exit_status_from_raw(0),
                        };
                        self.leader_exit_status = Some(st);
                        if self.has_unread_stdout() {
                            let res = self.reader.read_line(&mut self.line_buf).await;
                            self.handle_line_read(res)
                        } else {
                            Ok(LineEvent::Eof)
                        }
                    }
                }
            }
        }
    }

    fn has_unread_stdout(&self) -> bool {
        if !self.reader.buffer().is_empty() {
            return true;
        }
        use std::os::unix::io::AsRawFd;
        let fd = self.reader.get_ref().as_raw_fd();
        let mut bytes: libc::c_int = 0;
        // SAFETY: FIONREAD is a standard POSIX query on pipes returning readable byte count.
        if unsafe { libc::ioctl(fd, libc::FIONREAD, &mut bytes) } == 0 {
            bytes > 0
        } else {
            false
        }
    }

    fn handle_line_read(&mut self, res: std::io::Result<usize>) -> std::io::Result<LineEvent> {
        let n = res?;
        if n == 0 {
            return Ok(LineEvent::Eof);
        }
        if self.line_buf.ends_with('\n') {
            self.line_buf.pop();
            if self.line_buf.ends_with('\r') {
                self.line_buf.pop();
            }
        }
        let line = std::mem::take(&mut self.line_buf);
        self.last_activity = std::time::Instant::now();
        Ok(LineEvent::Line(line))
    }

    /// Stops the child process group: sends `SIGTERM` to the group, waits up to `term_grace`
    /// for the child leader, then escalates with `SIGKILL` to the group and kills the leader,
    /// then reaps. Idempotent.
    pub async fn stop(&mut self) -> std::process::ExitStatus {
        if let Some(status) = self.reaped_status {
            return status;
        }

        signal_group(self.pgid, GroupSignal::Term);

        let status = if let Some(st) = self.leader_exit_status {
            st
        } else {
            let wait_res = tokio::time::timeout(self.config.term_grace, self.child.wait()).await;
            match wait_res {
                Ok(Ok(st)) => st,
                _ => {
                    signal_group(self.pgid, GroupSignal::Kill);
                    let _ = self.child.start_kill();
                    match self.child.wait().await {
                        Ok(st) => st,
                        Err(_) => exit_status_from_raw(0),
                    }
                }
            }
        };

        unregister_group(self.pgid);
        self.reaped_status = Some(status);
        status
    }

    /// Waits up to `exit_grace` for the leader to exit on its own, then always sends `SIGTERM`
    /// to the group to reap leftovers (grandchildren holding stdout/stderr), then escalates like `stop`.
    pub async fn finish(&mut self, exit_grace: Duration) -> std::process::ExitStatus {
        if let Some(status) = self.reaped_status {
            return status;
        }

        let leader_status = if let Some(st) = self.leader_exit_status {
            Some(st)
        } else {
            match tokio::time::timeout(exit_grace, self.child.wait()).await {
                Ok(Ok(st)) => Some(st),
                _ => None,
            }
        };

        signal_group(self.pgid, GroupSignal::Term);

        let status = if let Some(st) = leader_status {
            st
        } else {
            match tokio::time::timeout(self.config.term_grace, self.child.wait()).await {
                Ok(Ok(st)) => st,
                _ => {
                    signal_group(self.pgid, GroupSignal::Kill);
                    let _ = self.child.start_kill();
                    match self.child.wait().await {
                        Ok(st) => st,
                        Err(_) => exit_status_from_raw(0),
                    }
                }
            }
        };

        unregister_group(self.pgid);
        self.reaped_status = Some(status);
        status
    }

    /// Waits at most `stderr_drain_grace` for the stderr drain task to complete,
    /// and returns the lossy UTF-8 tail string.
    pub async fn stderr_tail(&mut self) -> String {
        if let Some(mut task) = self.stderr_task.take() {
            let _ = tokio::time::timeout(self.config.stderr_drain_grace, &mut task).await;
        }
        let buf = self
            .stderr_tail_buf
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (s1, s2) = buf.as_slices();
        let mut bytes = Vec::with_capacity(buf.len());
        bytes.extend_from_slice(s1);
        bytes.extend_from_slice(s2);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

#[cfg(unix)]
impl Drop for Supervised {
    fn drop(&mut self) {
        if self.reaped_status.is_none() {
            signal_group(self.pgid, GroupSignal::Term);
            signal_group(self.pgid, GroupSignal::Kill);
            let _ = self.child.start_kill();
            unregister_group(self.pgid);
        }
        self.stderr_abort.abort();
    }
}

/// Spawns a supervised child process with isolated process group, piped stdout/stderr,
/// and background stderr draining.
#[cfg(unix)]
pub fn spawn(
    mut cmd: tokio::process::Command,
    config: SupervisorConfig,
) -> std::io::Result<Supervised> {
    configure_child(&mut cmd, &config.child_options);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| std::io::Error::other("spawned child did not return a pid"))?;
    let pgid = pid;
    register_group(pgid);

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("failed to capture child stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("failed to capture child stderr"))?;

    let tail_limit = config.stderr_tail_bytes;
    let stderr_tail_buf = std::sync::Arc::new(std::sync::Mutex::new(
        std::collections::VecDeque::with_capacity(tail_limit.min(64 * 1024)),
    ));
    let buf_clone = std::sync::Arc::clone(&stderr_tail_buf);

    let stderr_task = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut chunk = [0u8; 8192];
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tail_limit > 0 {
                        let to_add = &chunk[..n];
                        let mut buf = buf_clone.lock().unwrap_or_else(|e| e.into_inner());
                        if to_add.len() >= tail_limit {
                            buf.clear();
                            buf.extend(&to_add[to_add.len() - tail_limit..]);
                        } else {
                            let overflow = (buf.len() + to_add.len()).saturating_sub(tail_limit);
                            for _ in 0..overflow {
                                buf.pop_front();
                            }
                            buf.extend(to_add);
                        }
                    }
                }
            }
        }
    });

    let stderr_abort = stderr_task.abort_handle();
    let now = std::time::Instant::now();
    Ok(Supervised {
        child,
        pgid,
        reader: tokio::io::BufReader::new(stdout),
        line_buf: String::new(),
        stderr_task: Some(stderr_task),
        stderr_abort,
        stderr_tail_buf,
        config,
        spawn_time: now,
        last_activity: now,
        leader_exit_status: None,
        reaped_status: None,
    })
}

#[cfg(unix)]
fn exit_status_from_raw(code: i32) -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(code)
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Instant;

    use super::*;

    async fn wait_pid_gone(pid: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            // SAFETY: libc::kill with signal 0 checks process existence.
            let res = unsafe { libc::kill(pid as i32, 0) };
            if res != 0 {
                let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                if err == libc::ESRCH {
                    return true;
                }
            } else {
                #[cfg(target_os = "linux")]
                if let Ok(content) = std::fs::read_to_string(format!("/proc/{pid}/status"))
                    && content.lines().any(|l| l.starts_with("State:\tZ"))
                {
                    return true;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // SAFETY: final existence check.
        if unsafe { libc::kill(pid as i32, 0) != 0 }
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return true;
        }
        #[cfg(target_os = "linux")]
        if let Ok(content) = std::fs::read_to_string(format!("/proc/{pid}/status"))
            && content.lines().any(|l| l.starts_with("State:\tZ"))
        {
            return true;
        }
        false
    }

    #[tokio::test]
    async fn tc_1779_signal_group_refuses_zero_one_and_own_group() {
        assert!(!signal_group(0, GroupSignal::Term));
        assert!(!signal_group(0, GroupSignal::Kill));
        assert!(!signal_group(0, GroupSignal::Int));
        assert!(!signal_group(1, GroupSignal::Term));
        assert!(!signal_group(1, GroupSignal::Kill));
        assert!(!signal_group(1, GroupSignal::Int));

        // SAFETY: getpgrp returns current process group.
        let own_pgid = unsafe { libc::getpgrp() } as u32;
        assert!(!signal_group(own_pgid, GroupSignal::Term));
        assert!(!signal_group(own_pgid, GroupSignal::Kill));
        assert!(!signal_group(own_pgid, GroupSignal::Int));
    }

    #[tokio::test]
    async fn tc_1780_spawn_puts_child_in_own_group_and_registers_it() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let config = SupervisorConfig::new("tc_1780");
        let mut supervised = spawn(cmd, config).expect("spawn succeeds");

        let child_pid = supervised.pid().expect("pid exists");
        let pgid = supervised.pgid();
        assert_eq!(pgid, child_pid);

        // SAFETY: getpgrp returns caller process group.
        let our_pgrp = unsafe { libc::getpgrp() } as u32;
        assert_ne!(pgid, our_pgrp);

        assert!(live_groups().contains(&pgid));

        let _status = supervised.stop().await;
        assert!(!live_groups().contains(&pgid));
        assert!(wait_pid_gone(child_pid, Duration::from_secs(3)).await);
    }

    #[tokio::test]
    async fn tc_1781_next_line_streams_lines_then_eof() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", "echo hello; echo world"]);
        let config = SupervisorConfig::new("tc_1781");
        let mut supervised = spawn(cmd, config).expect("spawn succeeds");

        match supervised.next_line().await.expect("line 1") {
            LineEvent::Line(l) => assert_eq!(l, "hello"),
            other => panic!("expected Line, got {:?}", other),
        }
        match supervised.next_line().await.expect("line 2") {
            LineEvent::Line(l) => assert_eq!(l, "world"),
            other => panic!("expected Line, got {:?}", other),
        }
        match supervised.next_line().await.expect("eof") {
            LineEvent::Eof => {}
            other => panic!("expected Eof, got {:?}", other),
        }

        supervised.stop().await;
    }

    #[tokio::test]
    async fn tc_1782_idle_watchdog_fires_for_silent_child() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let mut config = SupervisorConfig::new("tc_1782");
        config.idle_timeout = Some(Duration::from_secs(1));
        let mut supervised = spawn(cmd, config).expect("spawn succeeds");
        let pid = supervised.pid().expect("pid exists");

        let start = Instant::now();
        let event = tokio::time::timeout(Duration::from_secs(5), supervised.next_line())
            .await
            .expect("did not timeout before 5s")
            .expect("next_line io ok");

        match event {
            LineEvent::Idle(d) => assert_eq!(d, Duration::from_secs(1)),
            other => panic!("expected Idle(1s), got {:?}", other),
        }
        assert!(start.elapsed() >= Duration::from_millis(900));

        let _status = supervised.stop().await;
        assert!(wait_pid_gone(pid, Duration::from_secs(3)).await);
    }

    #[tokio::test]
    async fn tc_1783_each_line_resets_idle_timer() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args([
            "-c",
            "for i in 1 2 3 4 5; do sleep 0.3; echo \"line $i\"; done",
        ]);
        let mut config = SupervisorConfig::new("tc_1783");
        config.idle_timeout = Some(Duration::from_secs(1));
        let mut supervised = spawn(cmd, config).expect("spawn succeeds");

        for i in 1..=5 {
            match supervised.next_line().await.expect("read line") {
                LineEvent::Line(l) => assert_eq!(l, format!("line {i}")),
                other => panic!("expected Line, got {:?}", other),
            }
        }
        match supervised.next_line().await.expect("read eof") {
            LineEvent::Eof => {}
            other => panic!("expected Eof, got {:?}", other),
        }

        supervised.stop().await;
    }

    #[tokio::test]
    async fn tc_1784_total_timeout_fires() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", "while true; do echo ping; sleep 0.2; done"]);
        let mut config = SupervisorConfig::new("tc_1784");
        config.idle_timeout = Some(Duration::from_secs(5));
        config.total_timeout = Some(Duration::from_secs(1));
        let mut supervised = spawn(cmd, config).expect("spawn succeeds");

        let mut got_timeout = false;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(4) {
            match supervised.next_line().await.expect("read event") {
                LineEvent::Line(l) => assert_eq!(l, "ping"),
                LineEvent::Timeout(d) => {
                    assert_eq!(d, Duration::from_secs(1));
                    got_timeout = true;
                    break;
                }
                LineEvent::Idle(_) => panic!("unexpected Idle event"),
                LineEvent::Eof => panic!("unexpected Eof"),
            }
        }
        assert!(got_timeout, "expected LineEvent::Timeout");

        supervised.stop().await;
    }

    #[tokio::test]
    async fn tc_1785_finish_reaps_grandchild_holding_stdout() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pid_file = tmp.path().join("child.pid");
        let pid_path_str = pid_file.to_str().expect("valid path");

        let mut cmd = tokio::process::Command::new("sh");
        cmd.args([
            "-c",
            &format!("sleep 30 & echo $! > '{pid_path_str}'; echo done"),
        ]);
        let config = SupervisorConfig::new("tc_1785");
        let mut supervised = spawn(cmd, config).expect("spawn succeeds");

        match supervised.next_line().await.expect("read done") {
            LineEvent::Line(l) => assert_eq!(l, "done"),
            other => panic!("expected Line(done), got {:?}", other),
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut grandchild_pid = None;
        while Instant::now() < deadline {
            if let Ok(content) = std::fs::read_to_string(&pid_file) {
                let trimmed = content.trim();
                if let Ok(pid) = trimmed.parse::<u32>() {
                    grandchild_pid = Some(pid);
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let grandchild_pid = grandchild_pid.expect("grandchild pid file was not written in time");

        let start = Instant::now();
        let _status = supervised.finish(Duration::from_millis(500)).await;
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(6),
            "finish took too long: {elapsed:?}"
        );

        assert!(
            wait_pid_gone(grandchild_pid, Duration::from_secs(5)).await,
            "grandchild pid {grandchild_pid} should be gone"
        );
    }

    #[tokio::test]
    async fn tc_1786_stderr_tail_keeps_last_bytes() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args([
            "-c",
            "head -c 204800 /dev/zero | tr '\\0' 'x' >&2; printf 'TAIL-MARK' >&2; echo done",
        ]);
        let mut config = SupervisorConfig::new("tc_1786");
        config.stderr_tail_bytes = 1024;
        let mut supervised = spawn(cmd, config).expect("spawn succeeds");

        match supervised.next_line().await.expect("read done") {
            LineEvent::Line(l) => assert_eq!(l, "done"),
            other => panic!("expected Line(done), got {:?}", other),
        }

        let _status = tokio::time::timeout(
            Duration::from_secs(5),
            supervised.finish(Duration::from_millis(500)),
        )
        .await
        .expect("finish did not hang (child was not blocked)");

        let tail = supervised.stderr_tail().await;
        assert!(tail.len() <= 1024, "tail len {} > 1024", tail.len());
        assert!(
            tail.ends_with("TAIL-MARK"),
            "tail should end with TAIL-MARK, got: {:?}",
            &tail[tail.len().saturating_sub(20)..]
        );
    }

    #[tokio::test]
    async fn tc_1787_stop_escalates_to_kill() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", "trap '' TERM; echo ready; sleep 30"]);
        let mut config = SupervisorConfig::new("tc_1787");
        config.term_grace = Duration::from_millis(300);
        let mut supervised = spawn(cmd, config).expect("spawn succeeds");
        let pid = supervised.pid().expect("pid exists");

        match supervised.next_line().await.expect("ready") {
            LineEvent::Line(l) => assert_eq!(l, "ready"),
            other => panic!("expected Line(ready), got {:?}", other),
        }

        let start = Instant::now();
        let _status = tokio::time::timeout(Duration::from_secs(5), supervised.stop())
            .await
            .expect("stop within 5 s");
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(250),
            "stop returned too quickly: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "stop took too long: {elapsed:?}"
        );

        assert!(wait_pid_gone(pid, Duration::from_secs(3)).await);
    }

    #[tokio::test]
    async fn tc_1788_drop_kills_group() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let config = SupervisorConfig::new("tc_1788");
        let supervised = spawn(cmd, config).expect("spawn succeeds");
        let pid = supervised.pid().expect("pid exists");
        let pgid = supervised.pgid();

        assert!(live_groups().contains(&pgid));

        drop(supervised);

        assert!(wait_pid_gone(pid, Duration::from_secs(3)).await);
        assert!(!live_groups().contains(&pgid));
    }

    #[tokio::test]
    async fn tc_1789_child_options_parent_death_signal_spawns() {
        let mut cmd_true = tokio::process::Command::new("sh");
        cmd_true.args(["-c", "echo hello_true"]);
        let mut cfg_true = SupervisorConfig::new("tc_1789_true");
        cfg_true.child_options.parent_death_signal = true;
        let mut sup_true = spawn(cmd_true, cfg_true).expect("spawn true succeeds");
        match sup_true.next_line().await.expect("read true") {
            LineEvent::Line(l) => assert_eq!(l, "hello_true"),
            other => panic!("expected Line, got {:?}", other),
        }
        sup_true.stop().await;

        let mut cmd_false = tokio::process::Command::new("sh");
        cmd_false.args(["-c", "echo hello_false"]);
        let mut cfg_false = SupervisorConfig::new("tc_1789_false");
        cfg_false.child_options.parent_death_signal = false;
        let mut sup_false = spawn(cmd_false, cfg_false).expect("spawn false succeeds");
        match sup_false.next_line().await.expect("read false") {
            LineEvent::Line(l) => assert_eq!(l, "hello_false"),
            other => panic!("expected Line, got {:?}", other),
        }
        sup_false.stop().await;
    }

    #[test]
    fn error_messages_format_correctly() {
        assert_eq!(
            idle_error_message("opencode", 300),
            "opencode produced no output for 300s; the provider may be stuck (for example a usage limit). Check the provider, or raise idle_timeout_secs (0 disables it)."
        );
        assert_eq!(
            timeout_error_message("opencode", 300),
            "opencode timed out after 300s"
        );
    }
}
