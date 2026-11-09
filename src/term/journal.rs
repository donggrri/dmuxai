//! Terminal session I/O journal (SPEC-terminal-sessions FR-2620, FR-2621): an
//! append-only, masked JSONL record of every session I/O event under
//! `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/terms/<16hex>/io.jsonl`, beside a
//! `manifest.json` (schema_version 1).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

use regex::Regex;
use serde::Serialize;

use crate::run_journal::{generate_run_id, is_symlink, mask_sensitive_tokens, verify_no_symlinks};

/// Manifest schema version written beside `io.jsonl` (FR-2620).
pub const TERM_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Output is coalesced into journal chunks of at most 8 KiB (FR-2620).
pub const TERM_IO_COALESCE_MAX_BYTES: usize = 8 * 1024;

/// Output pending in memory is flushed at most 50 ms after its first byte (FR-2620).
pub const TERM_IO_COALESCE_MAX_DELAY_MS: u64 = 50;

/// `io.jsonl` is rotated once it reaches 8 MiB; one previous generation is kept.
pub const TERM_IO_ROTATE_BYTES: u64 = 8 * 1024 * 1024;

/// Replacement recorded in place of secret input (FR-2621).
pub const REDACTED_INPUT: &str = "[REDACTED input]";

static SECRET_PROMPT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[Pp]assword:|passphrase|PIN:").unwrap());

/// True when output matches `[Pp]assword:|passphrase|PIN:` (FR-2621): the next
/// input line must be journaled as `[REDACTED input]`.
pub fn output_prompts_secret(output: &str) -> bool {
    SECRET_PROMPT_RE.is_match(output)
}

/// `$XDG_DATA_HOME/dmuxai/repos/<repo-key>/terms` (FR-2620).
///
/// Sibling of the runs tree: the final `runs` component of
/// [`crate::run_journal::repo_runs_dir`] is replaced with `terms`.
pub fn repo_terms_dir(workspace_root: &Path) -> io::Result<PathBuf> {
    let mut path = crate::run_journal::repo_runs_dir(workspace_root)?;
    if path.file_name() != Some(std::ffi::OsStr::new("runs")) || !path.pop() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "runs directory path does not end in runs",
        ));
    }
    path.push("terms");
    Ok(path)
}

/// Direction of a journaled I/O event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TermJournalDir {
    In,
    Out,
}

/// Per-session manifest written as `manifest.json` (schema_version 1, FR-2620).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermSessionManifest {
    pub schema_version: u32,
    pub session_id: String,
    pub name: String,
    pub kind: String,
    pub created_ms: u64,
}

#[derive(Debug, Serialize)]
struct TermJournalLine<'a> {
    seq: u64,
    t_ms: u64,
    dir: TermJournalDir,
    by: &'a str,
    run_id: Option<&'a str>,
    data: &'a str,
    bytes: usize,
}

#[derive(Debug)]
struct PendingOutput {
    seq: u64,
    t_ms: u64,
    by: String,
    run_id: Option<String>,
    data: String,
}

/// Masked JSONL journal for one terminal session (FR-2620).
///
/// Input, key, resize, and state-change events are written immediately;
/// output chunks are coalesced into lines of at most
/// [`TERM_IO_COALESCE_MAX_BYTES`] flushed at most
/// [`TERM_IO_COALESCE_MAX_DELAY_MS`] after their first byte. Write errors are
/// captured in [`TermJournal::last_error`] and never abort the caller.
#[derive(Debug)]
pub struct TermJournal {
    session_id: String,
    dir: PathBuf,
    io_path: PathBuf,
    io_file: Option<File>,
    io_size: u64,
    rotate_bytes: u64,
    pending: Option<PendingOutput>,
    awaiting_secret_input: bool,
    last_secret_text: Option<String>,
    last_error: Option<String>,
}

impl TermJournal {
    /// Creates the session directory with its manifest and an empty `io.jsonl`
    /// under the workspace's `terms/` tree.
    pub fn create(
        workspace_root: &Path,
        name: &str,
        kind: &str,
        created_ms: u64,
    ) -> io::Result<Self> {
        let terms_root = repo_terms_dir(workspace_root)?;
        verify_no_symlinks(&terms_root).map_err(|err| io::Error::other(err.to_string()))?;
        ensure_dir_0700(&terms_root)?;

        let session_id = generate_run_id();
        let dir = terms_root.join(&session_id);
        if is_symlink(&dir)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("refusing symlinked session directory at {}", dir.display()),
            ));
        }
        ensure_dir_0700(&dir)?;

        write_manifest(
            &dir,
            &TermSessionManifest {
                schema_version: TERM_MANIFEST_SCHEMA_VERSION,
                session_id: session_id.clone(),
                name: name.to_owned(),
                kind: kind.to_owned(),
                created_ms,
            },
        )?;

        let io_path = dir.join("io.jsonl");
        if is_symlink(&io_path)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("refusing symlinked journal at {}", io_path.display()),
            ));
        }
        let io_file = open_io_jsonl(&io_path)?;
        let io_size = io_file.metadata()?.len();

        Ok(Self {
            session_id,
            dir,
            io_path,
            io_file: Some(io_file),
            io_size,
            rotate_bytes: TERM_IO_ROTATE_BYTES,
            pending: None,
            awaiting_secret_input: false,
            last_secret_text: None,
            last_error: None,
        })
    }

    /// The 16-hex session ID, used as the session directory name.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The session directory holding `io.jsonl` and `manifest.json`.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The last journal write error, if any; `show` surfaces this as
    /// `journal_error` (FR-2620). Sticky until the next error replaces it.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Journals one input (text or keys) event. Input is written immediately,
    /// never coalesced. When `secret` is set, or the latest output was a
    /// password/passphrase/PIN prompt, the line records `[REDACTED input]`
    /// instead of the text (FR-2621).
    pub fn record_input(
        &mut self,
        seq: u64,
        t_ms: u64,
        by: &str,
        run_id: Option<&str>,
        text: &str,
        secret: bool,
    ) -> io::Result<()> {
        let result = self.do_record_input(seq, t_ms, by, run_id, text, secret);
        self.capture(result)
    }

    /// Journals one output chunk. Output is coalesced (FR-2620) and any
    /// occurrence of the last secret input is replaced before masking
    /// (FR-2621).
    pub fn record_output(
        &mut self,
        seq: u64,
        t_ms: u64,
        by: &str,
        run_id: Option<&str>,
        text: &str,
    ) -> io::Result<()> {
        let result = self.do_record_output(seq, t_ms, by, run_id, text);
        self.capture(result)
    }

    /// Journals a resize or state-change event as a single immediate line.
    pub fn record_event(
        &mut self,
        seq: u64,
        t_ms: u64,
        dir: TermJournalDir,
        by: &str,
        run_id: Option<&str>,
        data: &str,
    ) -> io::Result<()> {
        let result = self.do_record_event(seq, t_ms, dir, by, run_id, data);
        self.capture(result)
    }

    /// Flushes pending output when it has been quiet for at least
    /// [`TERM_IO_COALESCE_MAX_DELAY_MS`] since its first byte (FR-2620).
    pub fn flush_output_due(&mut self, now_ms: u64) -> io::Result<()> {
        let due = self.pending.as_ref().is_some_and(|pending| {
            now_ms.saturating_sub(pending.t_ms) >= TERM_IO_COALESCE_MAX_DELAY_MS
        });
        let result = if due { self.flush_pending() } else { Ok(()) };
        self.capture(result)
    }

    /// Flushes any pending coalesced output immediately.
    pub fn flush_output(&mut self) -> io::Result<()> {
        let result = self.flush_pending();
        self.capture(result)
    }

    fn capture(&mut self, result: io::Result<()>) -> io::Result<()> {
        if let Err(err) = &result {
            self.last_error = Some(err.to_string());
        }
        result
    }

    fn scrub_secret_text(&self, text: &str) -> String {
        match &self.last_secret_text {
            Some(secret) if !secret.is_empty() => text.replace(secret.as_str(), REDACTED_INPUT),
            _ => text.to_owned(),
        }
    }

    fn do_record_input(
        &mut self,
        seq: u64,
        t_ms: u64,
        by: &str,
        run_id: Option<&str>,
        text: &str,
        secret: bool,
    ) -> io::Result<()> {
        let redact = secret || self.awaiting_secret_input;
        self.awaiting_secret_input = false;
        if redact {
            if !text.is_empty() {
                self.last_secret_text = Some(text.to_owned());
            }
            return self.append_now(TermJournalDir::In, seq, t_ms, by, run_id, REDACTED_INPUT);
        }
        let masked = mask_sensitive_tokens(text);
        if masked.is_empty() {
            return Ok(());
        }
        self.append_now(TermJournalDir::In, seq, t_ms, by, run_id, &masked)
    }

    fn do_record_output(
        &mut self,
        seq: u64,
        t_ms: u64,
        by: &str,
        run_id: Option<&str>,
        text: &str,
    ) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let scrubbed = self.scrub_secret_text(text);
        let masked = mask_sensitive_tokens(&scrubbed);
        if masked.is_empty() {
            return Ok(());
        }
        if output_prompts_secret(&masked) {
            self.awaiting_secret_input = true;
        }
        match &mut self.pending {
            Some(pending) => pending.data.push_str(&masked),
            None => {
                self.pending = Some(PendingOutput {
                    seq,
                    t_ms,
                    by: by.to_owned(),
                    run_id: run_id.map(str::to_owned),
                    data: masked,
                });
            }
        }
        self.split_oversized_pending()
    }

    fn do_record_event(
        &mut self,
        seq: u64,
        t_ms: u64,
        dir: TermJournalDir,
        by: &str,
        run_id: Option<&str>,
        data: &str,
    ) -> io::Result<()> {
        let masked = mask_sensitive_tokens(data);
        if masked.is_empty() {
            return Ok(());
        }
        self.append_now(dir, seq, t_ms, by, run_id, &masked)
    }

    fn append_now(
        &mut self,
        dir: TermJournalDir,
        seq: u64,
        t_ms: u64,
        by: &str,
        run_id: Option<&str>,
        data: &str,
    ) -> io::Result<()> {
        if let Err(err) = self.flush_pending() {
            self.last_error = Some(err.to_string());
        }
        let line = TermJournalLine {
            seq,
            t_ms,
            dir,
            by,
            run_id,
            data,
            bytes: data.len(),
        };
        self.append_line(&line)
    }

    fn flush_pending(&mut self) -> io::Result<()> {
        let Some(pending) = self.pending.take() else {
            return Ok(());
        };
        let line = TermJournalLine {
            seq: pending.seq,
            t_ms: pending.t_ms,
            dir: TermJournalDir::Out,
            by: &pending.by,
            run_id: pending.run_id.as_deref(),
            data: &pending.data,
            bytes: pending.data.len(),
        };
        self.append_line(&line)
    }

    fn split_oversized_pending(&mut self) -> io::Result<()> {
        loop {
            let popped = {
                let Some(pending) = &mut self.pending else {
                    break;
                };
                if pending.data.len() <= TERM_IO_COALESCE_MAX_BYTES {
                    break;
                }
                let head_len = floor_char_boundary(&pending.data, TERM_IO_COALESCE_MAX_BYTES);
                let head: String = pending.data.drain(..head_len).collect();
                Some((
                    head,
                    pending.seq,
                    pending.t_ms,
                    pending.by.clone(),
                    pending.run_id.clone(),
                ))
            };
            let Some((head, seq, t_ms, by, run_id)) = popped else {
                break;
            };
            let line = TermJournalLine {
                seq,
                t_ms,
                dir: TermJournalDir::Out,
                by: &by,
                run_id: run_id.as_deref(),
                data: &head,
                bytes: head.len(),
            };
            self.append_line(&line)?;
        }
        Ok(())
    }

    fn append_line(&mut self, line: &TermJournalLine<'_>) -> io::Result<()> {
        let mut payload = serde_json::to_string(line)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
        payload.push('\n');
        if self.io_size >= self.rotate_bytes {
            self.rotate()?;
        }
        match self.io_file.as_mut() {
            Some(file) => {
                file.write_all(payload.as_bytes())?;
                file.flush()?;
            }
            None => {
                return Err(io::Error::other("term journal io.jsonl is not open"));
            }
        }
        self.io_size += payload.len() as u64;
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.io_file = None;
        let mut backup_name = self.io_path.as_os_str().to_owned();
        backup_name.push(".1");
        let backup = PathBuf::from(backup_name);
        fs::rename(&self.io_path, &backup)?;
        let file = open_io_jsonl(&self.io_path)?;
        self.io_size = 0;
        self.io_file = Some(file);
        Ok(())
    }
}

impl Drop for TermJournal {
    fn drop(&mut self) {
        let _ = self.flush_pending();
    }
}

/// Secure permissions of an existing directory without following symlinks.
#[cfg(unix)]
fn secure_dir_permissions_0700(path: &Path) -> Result<(), io::Error> {
    if is_symlink(path)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to chmod symlink at {}", path.display()),
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    let mode = file.metadata()?.permissions().mode() & 0o777;
    if mode != 0o700 {
        file.set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn secure_dir_permissions_0700(_path: &Path) -> Result<(), io::Error> {
    Ok(())
}

/// Fsync a directory after a rename or write without following symlinks.
#[cfg(unix)]
fn sync_dir(path: &Path) -> Result<(), io::Error> {
    if is_symlink(path)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to sync symlink directory at {}", path.display()),
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    file.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_path: &Path) -> Result<(), io::Error> {
    Ok(())
}

/// Open a file for writing with `O_NOFOLLOW` and mode 0600.
#[cfg(unix)]
fn open_file_write_nofollow(path: &Path, create_new: bool) -> Result<File, io::Error> {
    let mut opts = OpenOptions::new();
    opts.write(true);
    if create_new {
        opts.create_new(true);
    } else {
        opts.create(true).truncate(true);
    }
    opts.mode(0o600);
    opts.custom_flags(libc::O_NOFOLLOW);
    opts.open(path)
}

#[cfg(not(unix))]
fn open_file_write_nofollow(path: &Path, create_new: bool) -> Result<File, io::Error> {
    let mut opts = OpenOptions::new();
    opts.write(true);
    if create_new {
        opts.create_new(true);
    } else {
        opts.create(true).truncate(true);
    }
    opts.open(path)
}

fn ensure_dir_0700(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        if !path.exists() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)?;
        }
        secure_dir_permissions_0700(path)?;
    }
    #[cfg(not(unix))]
    {
        if !path.exists() {
            fs::create_dir_all(path)?;
        }
        secure_dir_permissions_0700(path)?;
    }
    Ok(())
}

fn write_manifest(dir: &Path, manifest: &TermSessionManifest) -> io::Result<()> {
    let manifest_path = dir.join("manifest.json");
    if is_symlink(&manifest_path)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to replace symlink at {}", manifest_path.display()),
        ));
    }
    let payload = serde_json::to_vec_pretty(manifest)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
    let tmp_name = format!(".manifest-{}.tmp", uuid::Uuid::new_v4().simple());
    let tmp_path = dir.join(tmp_name);
    let write_result = (|| -> io::Result<()> {
        let mut tmp_file = open_file_write_nofollow(&tmp_path, true)?;
        tmp_file.write_all(&payload)?;
        tmp_file.sync_all()?;
        Ok(())
    })();
    if let Err(err) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    fs::rename(&tmp_path, &manifest_path)?;
    sync_dir(dir)?;
    Ok(())
}

#[cfg(unix)]
fn open_io_jsonl(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_io_jsonl(path: &Path) -> io::Result<File> {
    OpenOptions::new().append(true).create(true).open(path)
}

fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut index = max;
    while index > 0 && !s.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    use crate::run_journal::valid_run_id;

    fn lock_env() -> MutexGuard<'static, ()> {
        crate::state_path::test_env_lock()
    }

    fn set_env(key: &str, value: impl AsRef<std::ffi::OsStr>) {
        // SAFETY: callers hold test_env_lock, so no other test reads the
        // environment concurrently while the variable is in flux.
        unsafe { std::env::set_var(key, value) };
    }

    fn remove_env(key: &str) {
        // SAFETY: same exclusion as `set_env`.
        unsafe { std::env::remove_var(key) };
    }

    struct EnvGuard {
        _guard: MutexGuard<'static, ()>,
        data_dir: PathBuf,
        workspace: PathBuf,
    }

    impl EnvGuard {
        fn new(tag: &str) -> Self {
            let guard = lock_env();
            let base = std::env::temp_dir().join(format!(
                "dmuxai-term-journal-{tag}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4().simple()
            ));
            let data_dir = base.join("data");
            let workspace = base.join("workspace");
            fs::create_dir_all(&workspace).unwrap();
            set_env("XDG_DATA_HOME", &data_dir);
            Self {
                _guard: guard,
                data_dir,
                workspace,
            }
        }

        fn terms_root(&self) -> PathBuf {
            repo_terms_dir(&self.workspace).unwrap()
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            remove_env("XDG_DATA_HOME");
            if let Some(base) = self.data_dir.parent() {
                let _ = fs::remove_dir_all(base);
            }
        }
    }

    fn parse_lines(path: &Path) -> Vec<serde_json::Value> {
        let content = fs::read_to_string(path).unwrap();
        content
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    // TC-2620 / SPEC-terminal-sessions FR-2620: io.jsonl holds in/out lines in
    // {seq, t_ms, dir, by, run_id, data, bytes} shape under the repo terms/
    // tree, with tokens masked and a schema_version 1 manifest beside it.
    #[test]
    fn tc_2620_io_jsonl_records_masked_in_and_out_lines() {
        let env = EnvGuard::new("lines");
        let mut journal = TermJournal::create(&env.workspace, "t0", "shell", 1000).unwrap();

        assert_eq!(journal.dir().parent().unwrap(), env.terms_root().as_path());
        assert!(valid_run_id(journal.session_id()));
        let io_path = journal.dir().join("io.jsonl");
        assert!(io_path.is_file());
        assert!(journal.dir().join("manifest.json").is_file());

        journal
            .record_input(
                1,
                1111,
                "agent",
                Some("0123456789abcdef"),
                "echo sk-test-AAAAAAAAAAAAAAAAAAAAAAAA",
                false,
            )
            .unwrap();
        journal
            .record_output(2, 1112, "child", None, "sk-test-AAAAAAAAAAAAAAAAAAAAAAAA\n")
            .unwrap();
        journal.flush_output().unwrap();

        let lines = parse_lines(&io_path);
        assert_eq!(lines.len(), 2);
        let input = &lines[0];
        assert_eq!(input.as_object().unwrap().len(), 7);
        assert_eq!(input["seq"].as_u64(), Some(1));
        assert_eq!(input["t_ms"].as_u64(), Some(1111));
        assert_eq!(input["dir"], "in");
        assert_eq!(input["by"], "agent");
        assert_eq!(input["run_id"], "0123456789abcdef");
        assert_eq!(input["data"], "echo [REDACTED]");
        assert_eq!(
            input["bytes"].as_u64(),
            Some("echo [REDACTED]".len() as u64)
        );

        let output = &lines[1];
        assert_eq!(output.as_object().unwrap().len(), 7);
        assert_eq!(output["dir"], "out");
        assert!(output["run_id"].is_null());
        assert_eq!(output["data"], "[REDACTED]\n");
        assert_eq!(output["bytes"].as_u64(), Some("[REDACTED]\n".len() as u64));

        let manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(journal.dir().join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["schema_version"].as_u64(), Some(1));
        assert_eq!(manifest["session_id"], journal.session_id());
        assert_eq!(manifest["name"], "t0");
        assert_eq!(manifest["kind"], "shell");
    }

    // TC-2620 / SPEC-terminal-sessions FR-2620: the journal tree is private:
    // dirs 0700, files 0600.
    #[cfg(unix)]
    #[test]
    fn tc_2620_dirs_and_files_use_0700_and_0600_modes() {
        use std::os::unix::fs::PermissionsExt;

        let env = EnvGuard::new("modes");
        let journal = TermJournal::create(&env.workspace, "t0", "shell", 1000).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&env.terms_root()), 0o700);
        assert_eq!(mode(journal.dir()), 0o700);
        assert_eq!(mode(&journal.dir().join("io.jsonl")), 0o600);
        assert_eq!(mode(&journal.dir().join("manifest.json")), 0o600);
    }

    // TC-2620 / SPEC-terminal-sessions FR-2620: output is coalesced into
    // chunks of at most 50 ms / 8 KiB.
    #[test]
    fn tc_2620_output_is_coalesced_within_50ms_and_8kib() {
        let env = EnvGuard::new("coalesce");
        let mut journal = TermJournal::create(&env.workspace, "t0", "shell", 1000).unwrap();
        let io_path = journal.dir().join("io.jsonl");

        journal
            .record_output(1, 1000, "child", None, "abc")
            .unwrap();
        journal
            .record_output(2, 1020, "child", None, "def")
            .unwrap();
        assert_eq!(fs::read_to_string(&io_path).unwrap(), "");

        journal.flush_output_due(1030).unwrap();
        assert_eq!(fs::read_to_string(&io_path).unwrap(), "");

        journal.flush_output_due(1050).unwrap();
        let lines = parse_lines(&io_path);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["seq"].as_u64(), Some(1));
        assert_eq!(lines[0]["t_ms"].as_u64(), Some(1000));
        assert_eq!(lines[0]["data"], "abcdef");
        assert_eq!(lines[0]["bytes"].as_u64(), Some(6));

        journal
            .record_output(3, 2000, "child", None, &"x".repeat(9000))
            .unwrap();
        journal.flush_output().unwrap();
        let lines = parse_lines(&io_path);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1]["data"], "x".repeat(8192));
        assert_eq!(lines[1]["bytes"].as_u64(), Some(8192));
        assert_eq!(lines[2]["data"], "x".repeat(808));
        assert_eq!(lines[2]["seq"].as_u64(), Some(3));
    }

    // TC-2620 / SPEC-terminal-sessions FR-2620 boundary: io.jsonl rotates at
    // the 8 MiB limit (exercised with a small limit), keeping io.jsonl.1.
    #[test]
    fn tc_2620_io_jsonl_rotates_at_limit() {
        let env = EnvGuard::new("rotate");
        let mut journal = TermJournal::create(&env.workspace, "t0", "shell", 1000).unwrap();
        journal.rotate_bytes = 100;
        let io_path = journal.dir().join("io.jsonl");
        let backup = PathBuf::from(format!("{}.1", io_path.display()));

        journal
            .record_input(1, 1000, "agent", None, &"a".repeat(200), false)
            .unwrap();
        assert!(!backup.exists());
        journal
            .record_input(2, 1001, "agent", None, &"b".repeat(200), false)
            .unwrap();
        assert!(backup.is_file());
        assert_eq!(fs::read_to_string(&backup).unwrap().lines().count(), 1);
        assert_eq!(fs::read_to_string(&io_path).unwrap().lines().count(), 1);
    }

    // TC-2620 / SPEC-terminal-sessions FR-2620 failure: a journal write error
    // is reported via last_error and journaling recovers; it never panics.
    #[test]
    fn tc_2620_write_error_is_reported_and_journal_recovers() {
        let env = EnvGuard::new("error");
        let mut journal = TermJournal::create(&env.workspace, "t0", "shell", 1000).unwrap();
        journal.rotate_bytes = 100;
        let io_path = journal.dir().join("io.jsonl");
        let backup = PathBuf::from(format!("{}.1", io_path.display()));
        fs::create_dir_all(&backup).unwrap();

        journal
            .record_input(1, 1000, "agent", None, &"a".repeat(200), false)
            .unwrap();
        assert!(journal.last_error().is_none());

        let err = journal
            .record_input(2, 1001, "agent", None, "b", false)
            .unwrap_err();
        assert!(!err.to_string().is_empty());
        assert!(journal.last_error().is_some());

        fs::remove_dir(&backup).unwrap();
        journal
            .record_input(3, 1002, "agent", None, "c", false)
            .unwrap();
        assert!(
            fs::read_to_string(&io_path)
                .unwrap()
                .contains("\"data\":\"c\"")
        );
        assert!(journal.last_error().is_some());
    }

    // TC-2620 / SPEC-terminal-sessions FR-2620: symlinks in the journal tree
    // are refused.
    #[cfg(unix)]
    #[test]
    fn tc_2620_symlinked_terms_dir_is_refused() {
        let env = EnvGuard::new("symlink");
        let terms_root = env.terms_root();
        fs::create_dir_all(terms_root.parent().unwrap()).unwrap();
        let elsewhere = env.data_dir.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &terms_root).unwrap();

        let err = TermJournal::create(&env.workspace, "t0", "shell", 1000).unwrap_err();
        assert!(err.to_string().contains("symlink"));
    }

    // TC-2621 / SPEC-terminal-sessions FR-2621: a --secret input is recorded
    // as [REDACTED input] and no reply echoes the typed text.
    #[test]
    fn tc_2621_secret_input_recorded_as_redacted_and_reply_does_not_echo_it() {
        let env = EnvGuard::new("secret");
        let mut journal = TermJournal::create(&env.workspace, "t0", "shell", 1000).unwrap();
        journal
            .record_input(1, 1000, "agent", None, "hunter2", true)
            .unwrap();
        journal
            .record_output(2, 1001, "child", None, "welcome hunter2\n")
            .unwrap();
        journal.flush_output().unwrap();

        let lines = parse_lines(&journal.dir().join("io.jsonl"));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["dir"], "in");
        assert_eq!(lines[0]["data"], REDACTED_INPUT);
        assert_eq!(lines[1]["dir"], "out");
        assert_eq!(lines[1]["data"], "welcome [REDACTED input]\n");
    }

    // TC-2621 / SPEC-terminal-sessions FR-2621: after output matching
    // [Pp]assword:|passphrase|PIN:, the next input is journaled as
    // [REDACTED input]; the following input is recorded normally again.
    #[test]
    fn tc_2621_input_after_password_prompt_recorded_as_redacted() {
        let env = EnvGuard::new("prompt");
        let mut journal = TermJournal::create(&env.workspace, "t0", "shell", 1000).unwrap();
        assert!(output_prompts_secret("Password: "));
        assert!(output_prompts_secret("enter PIN: "));
        assert!(output_prompts_secret("passphrase required"));
        assert!(!output_prompts_secret("hello world"));

        journal
            .record_output(1, 1000, "child", None, "Password: ")
            .unwrap();
        journal
            .record_input(2, 1001, "agent", None, "hunter2", false)
            .unwrap();
        journal
            .record_input(3, 1002, "agent", None, "ls -la\n", false)
            .unwrap();

        let lines = parse_lines(&journal.dir().join("io.jsonl"));
        assert_eq!(lines[0]["data"], "Password: ");
        assert_eq!(lines[1]["dir"], "in");
        assert_eq!(lines[1]["data"], REDACTED_INPUT);
        assert_eq!(lines[2]["data"], "ls -la\n");
    }
}
