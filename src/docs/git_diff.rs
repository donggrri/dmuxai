use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::docs::DocsError;
use crate::docs::confine::Workspace;

pub const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileDiff {
    pub path: String,
    pub diff: String,
    pub truncated: bool,
    pub binary: bool,
    pub untracked: bool,
}

fn git_command(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(["-c", "core.fsmonitor=false"])
        .current_dir(root)
        .env("GIT_PAGER", "cat")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd
}

fn check_inside_work_tree(root: &Path) -> Result<(), DocsError> {
    let mut cmd = git_command(root);
    cmd.args(["rev-parse", "--is-inside-work-tree"]);
    match cmd.status() {
        Ok(status) if status.success() => Ok(()),
        _ => Err(DocsError::NotGitRepo),
    }
}

fn read_git_output(mut child: std::process::Child) -> Result<(String, bool), DocsError> {
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| DocsError::Io("failed to capture git stdout".into()))?;

    let mut buf = Vec::new();
    // Read up to MAX_DIFF_BYTES + 1
    let mut take = (&mut stdout).take(MAX_DIFF_BYTES as u64 + 1);
    take.read_to_end(&mut buf)
        .map_err(|e| DocsError::Io(e.to_string()))?;

    let _ = child.kill();
    let _ = child.wait();

    let (diff_bytes, truncated) = if buf.len() > MAX_DIFF_BYTES {
        let capped = &buf[..MAX_DIFF_BYTES];
        let slice = match capped.iter().rposition(|&b| b == b'\n') {
            Some(last_nl) => &capped[..=last_nl],
            None => capped,
        };
        (slice, true)
    } else {
        (buf.as_slice(), false)
    };

    Ok((String::from_utf8_lossy(diff_bytes).into_owned(), truncated))
}

fn is_binary_diff(diff: &str) -> bool {
    diff.lines()
        .any(|line| line.starts_with("Binary files ") && line.ends_with(" differ"))
}

pub fn file_diff(ws: &Workspace, requested: &str) -> Result<FileDiff, DocsError> {
    let relative = match ws.resolve(requested) {
        Ok(resolved) => {
            if std::fs::metadata(&resolved.absolute)?.is_dir() {
                return Err(DocsError::NotAFile);
            }
            resolved.relative
        }
        Err(DocsError::NotFound) => {
            let norm = Workspace::normalize_relative(requested)?;
            // If requested is empty/dot, normalize_relative could return empty or path
            // Check if it exists as a directory
            let abs = ws.root().join(&norm);
            if abs.is_dir() {
                return Err(DocsError::NotAFile);
            }
            norm
        }
        Err(e) => return Err(e),
    };

    check_inside_work_tree(ws.root())?;

    let abs_path = ws.root().join(&relative);
    let file_exists = abs_path.is_file();

    let mut rev_cmd = git_command(ws.root());
    rev_cmd.args(["rev-parse", "--verify", "--quiet", "HEAD"]);
    let has_head = rev_cmd
        .status()
        .map(|status| status.success())
        .unwrap_or(false);

    let mut is_untracked = false;
    let mut final_diff = String::new();
    let mut final_truncated = false;

    if has_head {
        // Try diff against HEAD
        let mut head_cmd = git_command(ws.root());
        head_cmd.args([
            "-c",
            "core.quotepath=false",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "HEAD",
            "--",
            &relative,
        ]);

        let child = head_cmd.spawn().map_err(|e| DocsError::Io(e.to_string()))?;
        let (diff, truncated) = read_git_output(child)?;
        final_diff = diff;
        final_truncated = truncated;

        // If output is empty and file exists, check ls-files
        if final_diff.is_empty() && file_exists {
            let mut ls_cmd = git_command(ws.root());
            ls_cmd.args(["ls-files", "--error-unmatch", "--", &relative]);
            let is_not_tracked = match ls_cmd.status() {
                Ok(status) => !status.success(),
                Err(_) => false,
            };
            if is_not_tracked {
                is_untracked = true;
            }
        }
    } else if file_exists {
        is_untracked = true;
    }

    if is_untracked {
        let mut no_index_cmd = git_command(ws.root());
        no_index_cmd.args([
            "-c",
            "core.quotepath=false",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-index",
            "--",
            "/dev/null",
            &relative,
        ]);
        let child = no_index_cmd
            .spawn()
            .map_err(|e| DocsError::Io(e.to_string()))?;
        let (untracked_diff, untracked_truncated) = read_git_output(child)?;
        final_diff = untracked_diff;
        final_truncated = untracked_truncated;
    }

    let binary = is_binary_diff(&final_diff);

    Ok(FileDiff {
        path: relative,
        diff: final_diff,
        truncated: final_truncated,
        binary,
        untracked: is_untracked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn has_git() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    // TC-2261 (FR-2261)
    #[test]
    fn git_diff_text_unknown_extension() {
        if !has_git() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        // git init -q
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["config", "user.name", "t"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["config", "user.email", "t@e"])
                .status()
                .unwrap()
                .success()
        );

        let filename = "openssh_%.bbappend";
        let file_path = root.join(filename);
        fs::write(&file_path, "FILESEXTRAPATHS_prepend := \"x\"\n").unwrap();

        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["add", filename])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["commit", "-m", "init"])
                .status()
                .unwrap()
                .success()
        );

        fs::write(&file_path, "FILESEXTRAPATHS_prepend := \"y\"\n").unwrap();

        let ws = Workspace::new(root).unwrap();
        let diff = file_diff(&ws, filename).unwrap();

        assert_eq!(diff.path, filename);
        assert!(!diff.truncated);
        assert!(!diff.binary);
        assert!(!diff.untracked);
        assert!(
            diff.diff
                .contains("diff --git a/openssh_%.bbappend b/openssh_%.bbappend")
        );
        assert!(diff.diff.contains("@@"));
        assert!(diff.diff.contains("-FILESEXTRAPATHS_prepend := \"x\""));
        assert!(diff.diff.contains("+FILESEXTRAPATHS_prepend := \"y\""));
    }

    // TC-2262 (FR-2261)
    #[test]
    fn git_diff_binary_reports_git_line() {
        if !has_git() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["config", "user.name", "t"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["config", "user.email", "t@e"])
                .status()
                .unwrap()
                .success()
        );

        let bin_name = "blob.bin";
        let bin_path = root.join(bin_name);
        fs::write(&bin_path, b"hello\0world").unwrap();

        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["add", bin_name])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["commit", "-m", "init"])
                .status()
                .unwrap()
                .success()
        );

        fs::write(&bin_path, b"hello\0modified").unwrap();

        let ws = Workspace::new(root).unwrap();
        let diff = file_diff(&ws, bin_name).unwrap();

        assert_eq!(diff.path, bin_name);
        assert!(diff.binary);
        assert!(
            diff.diff
                .contains("Binary files a/blob.bin and b/blob.bin differ")
        );
    }

    // TC-2263 (FR-2261)
    #[test]
    fn git_diff_untracked_and_deleted() {
        if !has_git() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["config", "user.name", "t"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["config", "user.email", "t@e"])
                .status()
                .unwrap()
                .success()
        );

        // Track a file and commit
        let del_file = "deleted.txt";
        fs::write(root.join(del_file), "line1\nline2\n").unwrap();
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["add", del_file])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["commit", "-m", "add del"])
                .status()
                .unwrap()
                .success()
        );

        // Delete tracked file
        fs::remove_file(root.join(del_file)).unwrap();

        // Add untracked new_%.bb
        let untracked_name = "new_%.bb";
        fs::write(root.join(untracked_name), "DESCRIPTION = \"test\"\n").unwrap();

        let ws = Workspace::new(root).unwrap();

        // Untracked check
        let untracked_diff = file_diff(&ws, untracked_name).unwrap();
        assert!(untracked_diff.untracked);
        assert!(untracked_diff.diff.contains("+++ b/new_%.bb"));

        // Deleted check
        let del_diff = file_diff(&ws, del_file).unwrap();
        assert!(!del_diff.untracked);
        assert!(del_diff.diff.contains("-line1"));
        assert!(del_diff.diff.contains("-line2"));
    }

    #[test]
    fn git_diff_repo_without_commits() {
        if !has_git() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );

        let filename = "a_%.inc";
        fs::write(root.join(filename), "X = \"1\"\n").unwrap();

        let ws = Workspace::new(root).unwrap();
        let diff = file_diff(&ws, filename).unwrap();

        assert!(diff.untracked);
        assert!(diff.diff.contains("+X = \"1\""));
    }

    // TC-2267 (FR-2261, FR-740)
    #[test]
    fn git_diff_rejects_outside_paths() {
        if !has_git() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        let ws = Workspace::new(root).unwrap();

        assert_eq!(file_diff(&ws, "../x"), Err(DocsError::OutsideWorkspace));
    }
}
