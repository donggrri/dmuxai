//! Office -> PDF via LibreOffice headless (FR-747). OWNER: AGY.
//!
//! Contract:
//! - `pub fn find_soffice() -> Option<PathBuf>`: `DMUXAI_SOFFICE` (must exist; if set but
//!   missing -> None), else `soffice` / `libreoffice` on PATH (`.exe`/`.com` on Windows).
//! - `pub enum OfficePreview { Pdf(PathBuf), Unavailable { reason: String } }`
//! - `pub fn convert_to_pdf(soffice: Option<&Path>, source: &Path, sha256: &str, timeout: Duration)
//!    -> OfficePreview`: cache `temp_dir()/dmuxai-office/<sha256>.pdf`; fresh temp dir per run,
//!   isolated `-env:UserInstallation=file:///...`, `--headless --norestore --nolockcheck
//!   --convert-to pdf --outdir <tmp>`; kill on timeout; on Linux wrap with `unshare -rn` only when
//!   a probe `unshare -rn true` succeeds.

use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfficePreview {
    Pdf(PathBuf),
    Unavailable { reason: String },
}

/// Locates the `soffice` / `libreoffice` binary.
///
/// If `DMUXAI_SOFFICE` is set in the environment, it must exist; if missing, returns None.
/// Otherwise searches `PATH` for `soffice` or `libreoffice` (`.exe` / `.com` on Windows).
pub fn find_soffice() -> Option<PathBuf> {
    if let Ok(val) = std::env::var("DMUXAI_SOFFICE") {
        let path = PathBuf::from(&val);
        if path.is_file() {
            return Some(path);
        } else {
            return None;
        }
    }

    let names = if cfg!(windows) {
        &[
            "soffice.exe",
            "soffice.com",
            "libreoffice.exe",
            "libreoffice.com",
        ][..]
    } else {
        &["soffice", "libreoffice"][..]
    };

    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for name in names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

struct WorkDirCleanup(Option<PathBuf>);

impl Drop for WorkDirCleanup {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

/// Per-user PDF cache directory (not a shared `/tmp` path another user could
/// pre-seed): `<dmuxai state dir>/office-cache`, else the temp dir.
pub fn cache_dir() -> PathBuf {
    crate::state_path::state_base()
        .map(|base| base.join("office-cache"))
        .unwrap_or_else(|_| std::env::temp_dir().join("dmuxai-office"))
}

/// A cached file counts only if it is a regular file that starts like a PDF.
fn is_pdf_file(path: &Path) -> bool {
    use std::io::Read;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.file_type().is_file() {
        return false;
    }
    let mut head = [0u8; 5];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .map(|_| &head == b"%PDF-")
        .unwrap_or(false)
}

/// Kills the conversion process and, on unix, its whole process group.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{}", child.id())])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        // soffice.exe spawns soffice.bin; kill the whole tree.
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Creates `dir` (and parents); on unix the leaf is restricted to the user.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Converts an Office document to PDF using LibreOffice headless.
pub fn convert_to_pdf(
    soffice: Option<&Path>,
    source: &Path,
    sha256: &str,
    timeout: Duration,
) -> OfficePreview {
    // Per-user cache (not a shared /tmp path another user could pre-seed).
    let cache_dir = cache_dir();
    let cached_pdf = cache_dir.join(format!("{sha256}.pdf"));
    if is_pdf_file(&cached_pdf) {
        return OfficePreview::Pdf(cached_pdf);
    }

    let soffice_path = match soffice {
        Some(path) => path,
        None => {
            return OfficePreview::Unavailable {
                reason: "LibreOffice not available".to_string(),
            };
        }
    };

    if !soffice_path.is_file() {
        return OfficePreview::Unavailable {
            reason: "LibreOffice executable not found".to_string(),
        };
    }

    // One conversion at a time: LibreOffice is heavy and requests may pile up.
    static CONVERT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _convert_guard = CONVERT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if is_pdf_file(&cached_pdf) {
        return OfficePreview::Pdf(cached_pdf);
    }
    if let Err(e) = create_private_dir(&cache_dir) {
        return OfficePreview::Unavailable {
            reason: format!("failed to create cache dir: {e}"),
        };
    }

    let run_id = uuid::Uuid::new_v4().simple();
    let work_dir = std::env::temp_dir().join(format!("dmuxai-office-run-{}", run_id));
    let profile_dir = work_dir.join("profile");
    let out_dir = work_dir.join("out");

    if let Err(e) =
        create_private_dir(&work_dir).and_then(|_| std::fs::create_dir_all(&profile_dir))
    {
        return OfficePreview::Unavailable {
            reason: format!("failed to create profile dir: {e}"),
        };
    }
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        let _ = std::fs::remove_dir_all(&work_dir);
        return OfficePreview::Unavailable {
            reason: format!("failed to create output dir: {e}"),
        };
    }

    let cleanup = WorkDirCleanup(Some(work_dir.clone()));

    let profile_str = profile_dir.to_string_lossy().replace('\\', "/");
    let profile_arg = if profile_str.starts_with('/') {
        format!("-env:UserInstallation=file://{}", profile_str)
    } else {
        format!("-env:UserInstallation=file:///{}", profile_str)
    };

    let can_unshare = {
        #[cfg(target_os = "linux")]
        {
            static CAN_UNSHARE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *CAN_UNSHARE.get_or_init(|| {
                std::process::Command::new("unshare")
                    .args(["-rn", "true"])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    };

    let mut cmd = if can_unshare {
        let mut c = std::process::Command::new("unshare");
        c.args(["-rn", "--"]);
        c.arg(soffice_path);
        c
    } else {
        std::process::Command::new(soffice_path)
    };

    cmd.arg(&profile_arg)
        .arg("--headless")
        .arg("--norestore")
        .arg("--nolockcheck")
        .arg("--convert-to")
        .arg("pdf")
        .arg("--outdir")
        .arg(&out_dir)
        .arg(source);

    #[cfg(unix)]
    {
        // Own process group so a timeout kills soffice.bin (forked by the
        // `soffice` wrapper script) and not just the wrapper.
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return OfficePreview::Unavailable {
                reason: format!("failed to spawn LibreOffice: {e}"),
            };
        }
    };

    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(50);
    let mut timed_out = false;

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    timed_out = true;
                    kill_tree(&mut child);
                    break Err("conversion timed out".to_string());
                }
                std::thread::sleep(poll_interval);
            }
            Err(e) => {
                kill_tree(&mut child);
                break Err(e.to_string());
            }
        }
    };

    if timed_out {
        return OfficePreview::Unavailable {
            reason: "conversion timed out".to_string(),
        };
    }

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            return OfficePreview::Unavailable {
                reason: format!("LibreOffice exited with status {s}"),
            };
        }
        Err(e) => {
            return OfficePreview::Unavailable { reason: e };
        }
    }

    // Locate the converted pdf file in out_dir
    let expected_pdf_name = source
        .file_stem()
        .map(|s| format!("{}.pdf", s.to_string_lossy()));
    let mut found_pdf: Option<PathBuf> = None;

    if let Some(ref name) = expected_pdf_name {
        let p = out_dir.join(name);
        if p.is_file() {
            found_pdf = Some(p);
        }
    }

    if found_pdf.is_none()
        && let Ok(entries) = std::fs::read_dir(&out_dir)
    {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("pdf") {
                found_pdf = Some(p);
                break;
            }
        }
    }

    let Some(pdf_file) = found_pdf else {
        return OfficePreview::Unavailable {
            reason: "no PDF output produced".to_string(),
        };
    };

    let temp_cache_file = cache_dir.join(format!(".tmp-{}", run_id));
    if let Err(e) = std::fs::copy(&pdf_file, &temp_cache_file) {
        return OfficePreview::Unavailable {
            reason: format!("failed to cache PDF: {e}"),
        };
    }
    if let Err(e) = std::fs::rename(&temp_cache_file, &cached_pdf) {
        let _ = std::fs::remove_file(&temp_cache_file);
        if !cached_pdf.is_file() {
            return OfficePreview::Unavailable {
                reason: format!("failed to finalize cached PDF: {e}"),
            };
        }
    }

    drop(cleanup);

    OfficePreview::Pdf(cached_pdf)
}
