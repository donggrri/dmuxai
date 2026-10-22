//! Optional ACP trace log (SPEC-acp-runners NFR-1302). Never writes to stderr.

use std::io::Write as _;
use std::sync::Mutex;

static LOG_PATH: Mutex<Option<String>> = Mutex::new(None);

fn log_path() -> Option<String> {
    let mut guard = LOG_PATH.lock().unwrap();
    if guard.is_none() {
        *guard = std::env::var("DMUXAI_ACP_LOG")
            .ok()
            .filter(|p| !p.is_empty());
    }
    guard.clone()
}

pub fn acp_trace(message: impl AsRef<str>) {
    if std::env::var_os("DMUXAI_ACP_LOG").is_none() {
        return;
    }
    let Some(path) = log_path() else {
        return;
    };
    let line = format!(
        "{} {}\n",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
        message.as_ref()
    );
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}
