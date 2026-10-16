use std::process::ExitStatus;

use super::ExecutionMode;

pub(crate) struct ImplementTurnTracker {
    mode: ExecutionMode,
    writes_observed: bool,
}

impl ImplementTurnTracker {
    pub(crate) fn new(mode: ExecutionMode) -> Self {
        Self {
            mode,
            writes_observed: false,
        }
    }

    pub(crate) fn observe_provider_line(&mut self, line: &str) {
        if self.mode != ExecutionMode::Implement {
            return;
        }
        if provider_line_suggests_write(line) {
            self.writes_observed = true;
        }
    }

    pub(crate) fn finish(
        self,
        status: &ExitStatus,
        stderr: &str,
        protocol_completed: bool,
    ) -> Result<(), String> {
        if self.mode != ExecutionMode::Implement {
            if !status.success() && !stderr.trim().is_empty() {
                return Err(stderr.trim().to_owned());
            }
            if !status.success() {
                return Err(format!("provider exited with {status}"));
            }
            return Ok(());
        }

        if stderr_indicates_permission_denial(stderr) {
            return Err(stderr.trim().to_owned());
        }
        if !status.success() {
            return Err(if stderr.trim().is_empty() {
                format!("implement mode failed: provider exited with {status}")
            } else {
                stderr.trim().to_owned()
            });
        }
        if !protocol_completed {
            return Err(if stderr.trim().is_empty() {
                "implement mode failed: provider exited without a terminal result".to_owned()
            } else {
                stderr.trim().to_owned()
            });
        }
        if !self.writes_observed {
            return Err(
                "implement mode completed without any observed file changes; stderr had no writes"
                    .to_owned(),
            );
        }
        Ok(())
    }
}

pub(crate) fn stderr_indicates_permission_denial(stderr: &str) -> bool {
    let haystack = stderr.to_ascii_lowercase();
    [
        "permission denied",
        "not permitted",
        "approval required",
        "requires approval",
        "read-only",
        "read only",
        "sandbox",
        "eacces",
        "operation not permitted",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

fn provider_line_suggests_write(line: &str) -> bool {
    let haystack = line.to_ascii_lowercase();
    [
        "\"filechange\"",
        "file_change",
        "edit_file",
        "write_file",
        "apply_patch",
        "strreplace",
        "create_file",
        "\"type\":\"write\"",
        "accept-edits",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn permission_denial_is_detected_in_stderr() {
        assert!(stderr_indicates_permission_denial(
            "error: permission denied: sandbox"
        ));
        assert!(!stderr_indicates_permission_denial("ok"));
    }

    #[test]
    #[cfg(unix)]
    fn implement_without_writes_fails() {
        let tracker = ImplementTurnTracker::new(ExecutionMode::Implement);
        let status = ExitStatus::from_raw(0);
        let error = tracker
            .finish(&status, "", true)
            .expect_err("should require writes");
        assert!(error.contains("without any observed file changes"));
    }

    #[test]
    #[cfg(unix)]
    fn ask_mode_does_not_require_writes() {
        let tracker = ImplementTurnTracker::new(ExecutionMode::Ask);
        let status = ExitStatus::from_raw(0);
        assert!(tracker.finish(&status, "", true).is_ok());
    }
}
