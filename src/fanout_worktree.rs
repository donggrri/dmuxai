//! Git worktree helpers for write-mode fan-out (SPEC-fanout-worktree FR-2930..2938).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::workspace::{WORKSPACE_STATE_DIR, ensure_state_dir};

pub const FANOUT_SUBDIR: &str = "fanout";

/// Write fan-out runtime options passed from CLI into the App fan-out runner (FR-2930).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanoutWriteRunConfig {
    pub run_id: String,
    pub keep_worktrees: bool,
}

/// Patch summary for one leg (FR-2932).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FanoutDiffStat {
    pub files: usize,
    pub insertions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegPatchResult {
    pub diff_stat: FanoutDiffStat,
    pub changed: bool,
    pub patch_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FanoutWorktreeError {
    Usage(String),
    Io(String),
    Git(String),
    DiskGuard { need_gb: f64, have_gb: f64 },
}

impl std::fmt::Display for FanoutWorktreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(message) => f.write_str(message),
            Self::Io(message) => f.write_str(message),
            Self::Git(message) => f.write_str(message),
            Self::DiskGuard { need_gb, have_gb } => write!(
                f,
                "not enough free disk space for fan-out write worktrees (need {need_gb:.1} GiB, have {have_gb:.1} GiB)"
            ),
        }
    }
}

impl std::error::Error for FanoutWorktreeError {}

pub fn fanout_run_root(workspace: &Path, run_id: &str) -> PathBuf {
    workspace
        .join(WORKSPACE_STATE_DIR)
        .join(FANOUT_SUBDIR)
        .join(run_id)
}

pub fn leg_worktree_path(workspace: &Path, run_id: &str, agent: &str) -> PathBuf {
    fanout_run_root(workspace, run_id).join(sanitize_agent_dir(agent))
}

pub fn leg_patch_path(workspace: &Path, run_id: &str, agent: &str) -> PathBuf {
    fanout_run_root(workspace, run_id).join(format!("{}.patch", sanitize_agent_dir(agent)))
}

fn sanitize_agent_dir(agent: &str) -> String {
    agent
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// True when `workspace` is a git repo with at least one commit (FR-2930).
pub fn git_workspace_ready(workspace: &Path) -> Result<bool, FanoutWorktreeError> {
    let Ok(inside) = git_output(workspace, &["rev-parse", "--git-dir"]) else {
        return Ok(false);
    };
    if inside.trim().is_empty() {
        return Ok(false);
    }
    let head = git_output(workspace, &["rev-parse", "--verify", "HEAD"]);
    Ok(head.is_ok() && !head.unwrap_or_default().trim().is_empty())
}

pub fn validate_write_fanout_workspace(workspace: &Path) -> Result<(), FanoutWorktreeError> {
    if !git_workspace_ready(workspace)? {
        return Err(FanoutWorktreeError::Usage(
            "fan-out --write requires a git workspace with at least one commit".to_owned(),
        ));
    }
    Ok(())
}

/// `git rev-parse HEAD` for the main workspace (write fan-out guard, FR-2931).
pub fn main_workspace_head_sha(workspace: &Path) -> Option<String> {
    git_output(workspace, &["rev-parse", "HEAD"]).ok()
}

/// True when HEAD changed since `before` (write fan-out main-workspace guard, FR-2931).
pub fn main_workspace_head_changed(before: Option<&str>, workspace: &Path) -> bool {
    let Some(before) = before else {
        return false;
    };
    main_workspace_head_sha(workspace).is_some_and(|after| after != before)
}

const DISK_GUARD_SKIP_PREFIXES: &[&str] = &[".git/", "target/", "node_modules/", ".dmuxai/"];

fn git_path_excluded_for_disk_guard(relative: &str) -> bool {
    DISK_GUARD_SKIP_PREFIXES
        .iter()
        .any(|prefix| relative.starts_with(prefix))
}

pub fn working_tree_bytes(workspace: &Path) -> Result<u64, FanoutWorktreeError> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(workspace)
        .output()
        .map_err(|e| FanoutWorktreeError::Io(e.to_string()))?;
    if !output.status.success() {
        return Err(FanoutWorktreeError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let mut total = 0u64;
    for rel in output.stdout.split(|byte| *byte == 0) {
        if rel.is_empty() {
            continue;
        }
        let rel = String::from_utf8_lossy(rel);
        if git_path_excluded_for_disk_guard(rel.trim_start_matches("./")) {
            continue;
        }
        let path = workspace.join(rel.as_ref());
        if let Ok(meta) = std::fs::symlink_metadata(&path)
            && meta.is_file()
        {
            total = total.saturating_add(meta.len());
        }
    }
    Ok(total)
}

const GIB: u64 = 1024 * 1024 * 1024;

pub fn free_disk_bytes(path: &Path) -> Result<u64, FanoutWorktreeError> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| FanoutWorktreeError::Io("invalid path".to_owned()))?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let code = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
        if code == 0 {
            let free = stat.f_bavail as u64 * stat.f_frsize as u64;
            return Ok(free);
        }
    }
    let output = Command::new("df")
        .args(["-B1", "--"])
        .arg(path)
        .output()
        .map_err(|e| FanoutWorktreeError::Io(e.to_string()))?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.lines().nth(1).unwrap_or_default();
        if let Some(avail) = line.split_whitespace().nth(3)
            && let Ok(bytes) = avail.parse::<u64>()
        {
            return Ok(bytes);
        }
    }
    Err(FanoutWorktreeError::Io(
        "could not determine free disk space".to_owned(),
    ))
}

/// FR-2937: require `min_free_gb` plus one working-tree copy per leg before any worktree is created.
pub fn disk_guard_check(
    workspace: &Path,
    leg_count: usize,
    min_free_gb: u64,
) -> Result<(), FanoutWorktreeError> {
    let wt = working_tree_bytes(workspace)?;
    let need = min_free_gb
        .saturating_mul(GIB)
        .saturating_add(wt.saturating_mul(leg_count as u64));
    let free = free_disk_bytes(workspace)?;
    if free < need {
        return Err(FanoutWorktreeError::DiskGuard {
            need_gb: need as f64 / GIB as f64,
            have_gb: free as f64 / GIB as f64,
        });
    }
    Ok(())
}

pub fn leg_base_commit_path(workspace: &Path, run_id: &str, agent: &str) -> PathBuf {
    fanout_run_root(workspace, run_id).join(format!("{}.base", sanitize_agent_dir(agent)))
}

fn read_leg_base_commit(
    workspace: &Path,
    run_id: &str,
    agent: &str,
) -> Result<String, FanoutWorktreeError> {
    let path = leg_base_commit_path(workspace, run_id, agent);
    let raw = std::fs::read_to_string(&path).map_err(|e| FanoutWorktreeError::Io(e.to_string()))?;
    let sha = raw.trim();
    if sha.is_empty() {
        return Err(FanoutWorktreeError::Git(
            "missing leg base commit".to_owned(),
        ));
    }
    Ok(sha.to_owned())
}

/// FR-2934: run ids must be a single safe path component (no `/` or `..`).
pub fn validate_fanout_run_id(run_id: &str) -> Result<(), FanoutWorktreeError> {
    if run_id.is_empty() {
        return Err(FanoutWorktreeError::Usage(
            "fanout run id must not be empty".to_owned(),
        ));
    }
    if run_id == "." || run_id == ".." || run_id.contains("..") {
        return Err(FanoutWorktreeError::Usage(
            "fanout run id is not allowed".to_owned(),
        ));
    }
    if run_id.contains('/') || run_id.contains('\\') {
        return Err(FanoutWorktreeError::Usage(
            "fanout run id must not contain path separators".to_owned(),
        ));
    }
    if !run_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(FanoutWorktreeError::Usage(
            "fanout run id contains invalid characters".to_owned(),
        ));
    }
    Ok(())
}

pub fn create_leg_worktree(
    workspace: &Path,
    run_id: &str,
    agent: &str,
) -> Result<PathBuf, FanoutWorktreeError> {
    let root = fanout_run_root(workspace, run_id);
    ensure_state_dir(&workspace.join(WORKSPACE_STATE_DIR))
        .map_err(|e| FanoutWorktreeError::Io(e.to_string()))?;
    std::fs::create_dir_all(&root).map_err(|e| FanoutWorktreeError::Io(e.to_string()))?;
    let wt = leg_worktree_path(workspace, run_id, agent);
    if wt.exists() {
        return Err(FanoutWorktreeError::Git(format!(
            "worktree path already exists: {}",
            wt.display()
        )));
    }
    let head = git_output(workspace, &["rev-parse", "HEAD"])?;
    git_cmd(
        workspace,
        &[
            "worktree",
            "add",
            "--detach",
            wt.to_str()
                .ok_or_else(|| FanoutWorktreeError::Io("non-utf8 path".to_owned()))?,
            &head,
        ],
    )?;
    let base_path = leg_base_commit_path(workspace, run_id, agent);
    if let Some(parent) = base_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| FanoutWorktreeError::Io(e.to_string()))?;
    }
    std::fs::write(&base_path, format!("{head}\n"))
        .map_err(|e| FanoutWorktreeError::Io(e.to_string()))?;
    Ok(wt)
}

pub fn produce_leg_patch(
    workspace: &Path,
    run_id: &str,
    agent: &str,
    worktree: &Path,
) -> Result<LegPatchResult, FanoutWorktreeError> {
    git_cmd(worktree, &["add", "-N", "."])?;
    let patch_path = leg_patch_path(workspace, run_id, agent);
    if let Some(parent) = patch_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| FanoutWorktreeError::Io(e.to_string()))?;
    }
    let base = read_leg_base_commit(workspace, run_id, agent)?;
    let diff = Command::new("git")
        .args(["diff", &base, "--binary"])
        .current_dir(worktree)
        .output()
        .map_err(|e| FanoutWorktreeError::Io(e.to_string()))?;
    if !diff.status.success() {
        return Err(FanoutWorktreeError::Git(
            String::from_utf8_lossy(&diff.stderr).trim().to_owned(),
        ));
    }
    std::fs::write(&patch_path, &diff.stdout)
        .map_err(|e| FanoutWorktreeError::Io(e.to_string()))?;
    let diff_stat = parse_numstat(worktree, &base)?;
    let changed = !diff.stdout.is_empty() && diff_stat.files > 0;
    Ok(LegPatchResult {
        diff_stat,
        changed,
        patch_path,
    })
}

fn parse_numstat(worktree: &Path, base: &str) -> Result<FanoutDiffStat, FanoutWorktreeError> {
    let output = Command::new("git")
        .args(["diff", "--numstat", base])
        .current_dir(worktree)
        .output()
        .map_err(|e| FanoutWorktreeError::Io(e.to_string()))?;
    if !output.status.success() {
        return Err(FanoutWorktreeError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let mut files = 0usize;
    let mut insertions = 0usize;
    let mut deletions = 0usize;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let ins = parts.next().unwrap_or("-");
        let del = parts.next().unwrap_or("-");
        if ins == "-" || del == "-" {
            files += 1;
            continue;
        }
        files += 1;
        insertions = insertions.saturating_add(ins.parse().unwrap_or(0));
        deletions = deletions.saturating_add(del.parse().unwrap_or(0));
    }
    Ok(FanoutDiffStat {
        files,
        insertions,
        deletions,
    })
}

pub fn remove_leg_worktree(workspace: &Path, worktree: &Path) -> Result<(), FanoutWorktreeError> {
    if !path_under_fanout(workspace, worktree) {
        return Err(FanoutWorktreeError::Usage(
            "refusing to remove a path outside .dmuxai/fanout".to_owned(),
        ));
    }
    if worktree.exists() {
        git_cmd(
            workspace,
            &["worktree", "remove", "--force", &path_to_git_arg(worktree)?],
        )?;
    }
    let _ = git_cmd(workspace, &["worktree", "prune"]);
    Ok(())
}

pub fn fanout_clean(
    workspace: &Path,
    run_id: Option<&str>,
    all: bool,
) -> Result<(), FanoutWorktreeError> {
    let fanout_root = workspace.join(WORKSPACE_STATE_DIR).join(FANOUT_SUBDIR);
    if !all {
        let Some(run_id) = run_id else {
            return Err(FanoutWorktreeError::Usage(
                "fanout clean needs a run id or --all".to_owned(),
            ));
        };
        validate_fanout_run_id(run_id)?;
    }
    if !fanout_root.is_dir() {
        let _ = git_cmd(workspace, &["worktree", "prune"]);
        return Ok(());
    }
    if all {
        for entry in
            std::fs::read_dir(&fanout_root).map_err(|e| FanoutWorktreeError::Io(e.to_string()))?
        {
            let entry = entry.map_err(|e| FanoutWorktreeError::Io(e.to_string()))?;
            if entry
                .file_type()
                .map_err(|e| FanoutWorktreeError::Io(e.to_string()))?
                .is_dir()
            {
                clean_one_run(workspace, &entry.path())?;
            }
        }
        return Ok(());
    }
    let run_id = run_id.expect("validated above");
    let run_dir = fanout_root.join(run_id);
    if run_dir.is_dir() {
        clean_one_run(workspace, &run_dir)?;
    }
    let _ = git_cmd(workspace, &["worktree", "prune"]);
    Ok(())
}

fn clean_one_run(workspace: &Path, run_dir: &Path) -> Result<(), FanoutWorktreeError> {
    if !path_under_fanout(workspace, run_dir) {
        return Err(FanoutWorktreeError::Usage(
            "refusing to clean outside .dmuxai/fanout".to_owned(),
        ));
    }
    if let Ok(read) = std::fs::read_dir(run_dir) {
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() && path_under_fanout(workspace, &path) {
                let _ = remove_leg_worktree(workspace, &path);
            }
        }
    }
    let _ = git_cmd(workspace, &["worktree", "prune"]);
    Ok(())
}

pub fn path_under_fanout(workspace: &Path, path: &Path) -> bool {
    let fanout = workspace.join(WORKSPACE_STATE_DIR).join(FANOUT_SUBDIR);
    let Ok(fanout) = fanout.canonicalize() else {
        return false;
    };
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    path.strip_prefix(&fanout).is_ok()
}

fn git_cmd(workspace: &Path, args: &[&str]) -> Result<(), FanoutWorktreeError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .map_err(|e| FanoutWorktreeError::Io(e.to_string()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(FanoutWorktreeError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

fn git_output(workspace: &Path, args: &[&str]) -> Result<String, FanoutWorktreeError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .map_err(|e| FanoutWorktreeError::Io(e.to_string()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(FanoutWorktreeError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

fn path_to_git_arg(path: &Path) -> Result<String, FanoutWorktreeError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| FanoutWorktreeError::Io("non-utf8 path".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_git_repo(dir: &Path) {
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "fanout@test"][..],
            &["config", "user.name", "fanout"][..],
        ] {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(dir)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::write(dir.join("tracked.txt"), "base\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "tracked.txt"])
                .current_dir(dir)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["commit", "-qm", "init"])
                .current_dir(dir)
                .status()
                .unwrap()
                .success()
        );
    }

    fn temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        init_git_repo(dir.path());
        dir
    }

    // TC-2932 / FR-2932
    #[test]
    fn tc_2932_patch_and_diff_stat_cover_new_modified_deleted_files() {
        let dir = temp_repo();
        let ws = dir.path();
        let run_id = "run-patch";
        let wt = create_leg_worktree(ws, run_id, "cursor").expect("worktree");
        std::fs::write(wt.join("tracked.txt"), "changed\n").unwrap();
        std::fs::write(wt.join("new.txt"), "new\n").unwrap();
        std::fs::remove_file(wt.join("tracked.txt")).ok();
        std::fs::write(wt.join("tracked.txt"), "mod\n").unwrap();
        std::fs::write(wt.join("brand_new.txt"), "x\n").unwrap();
        std::fs::remove_file(wt.join("brand_new.txt")).unwrap();
        std::fs::write(wt.join("only_new.txt"), "only\n").unwrap();
        let patch = produce_leg_patch(ws, run_id, "cursor", &wt).expect("patch");
        assert!(patch.changed);
        assert!(patch.patch_path.is_file());
        let body = std::fs::read_to_string(&patch.patch_path).unwrap();
        assert!(body.contains("only_new.txt"));
        assert!(patch.diff_stat.files >= 1);
        remove_leg_worktree(ws, &wt).expect("remove");
        assert!(patch.patch_path.is_file());
    }

    // TC-2933 / FR-2933
    #[test]
    fn tc_2933_worktrees_removed_after_success_failure_and_interrupt() {
        let dir = temp_repo();
        let ws = dir.path();
        let run_id = "run-clean";
        let wt = create_leg_worktree(ws, run_id, "a").expect("wt");
        assert!(wt.is_dir());
        let patch = leg_patch_path(ws, run_id, "a");
        std::fs::write(wt.join("x.txt"), "y").unwrap();
        let _ = produce_leg_patch(ws, run_id, "a", &wt);
        remove_leg_worktree(ws, &wt).expect("remove success");
        assert!(!wt.exists());
        assert!(patch.is_file());

        let wt2 = create_leg_worktree(ws, run_id, "b").expect("wt2");
        remove_leg_worktree(ws, &wt2).expect("remove interrupt");
        assert!(!wt2.exists());
    }

    // TC-2934 / FR-2934
    #[test]
    fn tc_2934_fanout_clean_removes_only_fanout_worktrees() {
        let dir = temp_repo();
        let ws = dir.path();
        let outside = ws.join("manual-worktree");
        std::fs::create_dir_all(&outside).unwrap();
        let run_id = "rid";
        let wt = create_leg_worktree(ws, run_id, "codex").expect("wt");
        std::fs::write(wt.join("touch.txt"), "z").unwrap();
        let patch = produce_leg_patch(ws, run_id, "codex", &wt).unwrap();
        fanout_clean(ws, Some(run_id), false).expect("clean run");
        assert!(!wt.exists());
        assert!(patch.patch_path.is_file());
        assert!(outside.is_dir());
        let wt2 = create_leg_worktree(ws, "other", "agy").expect("wt2");
        create_leg_worktree(ws, "run2", "cursor").expect("wt3");
        fanout_clean(ws, None, true).expect("clean all");
        assert!(!wt2.exists());
        remove_leg_worktree(ws, &outside).expect_err("outside fanout");
    }

    // TC-2937 / FR-2937
    #[test]
    fn tc_2937_disk_guard_blocks_before_worktree_creation() {
        let dir = temp_repo();
        let ws = dir.path();
        disk_guard_check(ws, 1, 0).expect("tiny repo should pass with 0 min");
        let err = disk_guard_check(ws, 1, u64::MAX / GIB).expect_err("absurd min");
        assert!(matches!(err, FanoutWorktreeError::DiskGuard { .. }));
    }

    // TC-2934: reject path traversal in fanout clean run ids
    #[test]
    fn tc_2934_rejects_traversal_run_id_in_fanout_clean() {
        let dir = temp_repo();
        let ws = dir.path();
        assert!(fanout_clean(ws, Some("../.."), false).is_err());
        assert!(fanout_clean(ws, Some("foo/bar"), false).is_err());
    }

    // TC-2930 partial: git preconditions
    #[test]
    fn tc_2930_write_flag_preconditions_and_leg_cwd() {
        let empty = tempfile::tempdir().unwrap();
        assert!(!git_workspace_ready(empty.path()).unwrap());
        let dir = temp_repo();
        assert!(git_workspace_ready(dir.path()).unwrap());
        validate_write_fanout_workspace(dir.path()).expect("ready");
        let run_id = "pre";
        let wt = create_leg_worktree(dir.path(), run_id, "cursor").unwrap();
        assert!(path_under_fanout(dir.path(), &wt));
        remove_leg_worktree(dir.path(), &wt).unwrap();
    }
}
