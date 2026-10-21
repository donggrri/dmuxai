//! Read and save (FR-742, FR-744, FR-745). OWNER: AGY.
//!
//! Contract:
//! - `pub struct DocFile { pub path: String, pub kind: DocKind, pub editable: bool, pub size: u64,
//!    pub mtime_ms: u64, pub sha256: String, pub content: Option<String>,
//!    pub code_html: Option<String>, pub language: Option<String> }` (Serialize)
//! - `pub fn read_file(ws: &Workspace, path: &str) -> Result<DocFile, DocsError>`
//! - `pub fn read_bytes(ws: &Workspace, path: &str, max: u64) -> Result<(ResolvedPath, Vec<u8>), DocsError>`
//! - `pub struct SaveOutcome { pub sha256: String, pub mtime_ms: u64, pub size: u64 }` (Serialize)
//! - `pub fn save_file(ws: &Workspace, path: &str, content: &str, base_sha256: &str) -> Result<SaveOutcome, DocsError>`
//!   order of checks: resolve (404/403) -> html (HtmlPreviewOnly) -> !editable (NotEditable)
//!   -> size (TooLarge) -> hash (Conflict) -> atomic temp-file + rename in the same dir of the
//!   canonical target, keeping permissions. A process-wide mutex serializes saves.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::docs::confine::{ResolvedPath, Workspace};
use crate::docs::{DocKind, DocsError, MAX_TEXT_BYTES, mtime_ms, sha256_hex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocFile {
    pub path: String,
    pub kind: DocKind,
    pub editable: bool,
    pub size: u64,
    pub mtime_ms: u64,
    pub sha256: String,
    pub content: Option<String>,
    /// Text kinds only: highlighted, line-split HTML for the code view
    /// (`crate::docs::highlight`). Absent above the size cap or for non-text
    /// kinds; `content` stays the raw text used for editing and saving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_html: Option<String>,
    /// syntect syntax name for `code_html` (`None` for plain text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveOutcome {
    pub sha256: String,
    pub mtime_ms: u64,
    pub size: u64,
}

static SAVE_LOCK: Mutex<()> = Mutex::new(());

fn hash_file_stream(path: &Path) -> Result<String, DocsError> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest.iter() {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    Ok(out)
}

/// Reads a document file's metadata and content (FR-742).
pub fn read_file(ws: &Workspace, path: &str) -> Result<DocFile, DocsError> {
    let resolved = ws.resolve(path)?;
    let metadata = std::fs::metadata(&resolved.absolute)?;
    if metadata.is_dir() {
        return Err(DocsError::NotAFile);
    }

    let size = metadata.len();
    let mtime = mtime_ms(&metadata);
    let kind = DocKind::for_path(&resolved.absolute);

    let (content, sha256, is_valid_utf8) = if size > MAX_TEXT_BYTES {
        let sha256 = hash_file_stream(&resolved.absolute)?;
        (None, sha256, false)
    } else {
        let bytes = std::fs::read(&resolved.absolute)?;
        let sha256 = sha256_hex(&bytes);
        if kind.has_text_content() {
            match String::from_utf8(bytes) {
                Ok(text) => (Some(text), sha256, true),
                Err(_) => (None, sha256, false),
            }
        } else {
            (None, sha256, false)
        }
    };

    let editable = kind.editable() && size <= MAX_TEXT_BYTES && is_valid_utf8;
    let code_view = match (&kind, &content) {
        (DocKind::Text, Some(text)) => {
            crate::docs::highlight::highlight_file(&resolved.relative, text)
        }
        _ => None,
    };
    let (code_html, language) = match code_view {
        Some(view) => (Some(view.html), view.language),
        None => (None, None),
    };

    Ok(DocFile {
        path: resolved.relative,
        kind,
        editable,
        size,
        mtime_ms: mtime,
        sha256,
        content,
        code_html,
        language,
    })
}

/// Reads raw bytes for a file up to `max` bytes.
pub fn read_bytes(
    ws: &Workspace,
    path: &str,
    max: u64,
) -> Result<(ResolvedPath, Vec<u8>), DocsError> {
    let resolved = ws.resolve(path)?;
    let metadata = std::fs::metadata(&resolved.absolute)?;
    if metadata.is_dir() {
        return Err(DocsError::NotAFile);
    }
    if metadata.len() > max {
        return Err(DocsError::TooLarge);
    }
    let bytes = std::fs::read(&resolved.absolute)?;
    if bytes.len() as u64 > max {
        return Err(DocsError::TooLarge);
    }
    Ok((resolved, bytes))
}

struct TempFileCleanup(Option<PathBuf>);

impl Drop for TempFileCleanup {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Saves an editable document file with concurrency check and atomic rename (FR-744, FR-745).
pub fn save_file(
    ws: &Workspace,
    path: &str,
    content: &str,
    base_sha256: &str,
) -> Result<SaveOutcome, DocsError> {
    let _guard = SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // 1. Resolve (404/403) and check it's a file
    let resolved = ws.resolve(path)?;
    let metadata = std::fs::metadata(&resolved.absolute)?;
    if metadata.is_dir() {
        return Err(DocsError::NotAFile);
    }

    // 2. HTML check -> 403 HtmlPreviewOnly
    // Both the requested name and the symlink target must be non-HTML.
    let kind = DocKind::for_path(&resolved.absolute);
    if kind == DocKind::Html || DocKind::for_path(Path::new(&resolved.relative)) == DocKind::Html {
        return Err(DocsError::HtmlPreviewOnly);
    }
    let kind = if DocKind::for_path(Path::new(&resolved.relative)).editable() {
        kind
    } else {
        DocKind::Binary
    };

    // 3. Editable check -> 415 NotEditable
    if !kind.editable() {
        return Err(DocsError::NotEditable);
    }

    // 4. Size check -> 413 TooLarge
    let content_bytes = content.as_bytes();
    if content_bytes.len() as u64 > MAX_TEXT_BYTES {
        return Err(DocsError::TooLarge);
    }

    // 5. Concurrency hash check -> 409 Conflict
    let current_bytes = std::fs::read(&resolved.absolute)?;
    let current_sha256 = sha256_hex(&current_bytes);
    if current_sha256 != base_sha256 {
        let current_meta = std::fs::metadata(&resolved.absolute).unwrap_or(metadata);
        return Err(DocsError::Conflict {
            sha256: current_sha256,
            mtime_ms: mtime_ms(&current_meta),
        });
    }

    // 6. Atomic temp-file + rename in same dir of canonical target, keeping permissions
    let parent = resolved
        .absolute
        .parent()
        .ok_or(DocsError::OutsideWorkspace)?;
    let permissions = metadata.permissions();

    let temp_file_name = format!(".dmux-save-tmp-{}", uuid::Uuid::new_v4().simple());
    let temp_path = parent.join(temp_file_name);
    let mut cleanup = TempFileCleanup(Some(temp_path.clone()));

    std::fs::write(&temp_path, content_bytes)?;
    let _ = std::fs::set_permissions(&temp_path, permissions);

    // Narrow the window for writers outside this process: re-check the hash
    // right before replacing (residual race documented in REVIEW).
    let latest = std::fs::read(&resolved.absolute)?;
    if sha256_hex(&latest) != base_sha256 {
        let latest_meta = std::fs::metadata(&resolved.absolute)?;
        return Err(DocsError::Conflict {
            sha256: sha256_hex(&latest),
            mtime_ms: mtime_ms(&latest_meta),
        });
    }
    std::fs::rename(&temp_path, &resolved.absolute)?;
    cleanup.0 = None; // disarm cleanup on success

    let new_metadata = std::fs::metadata(&resolved.absolute)?;
    let new_sha256 = sha256_hex(content_bytes);

    Ok(SaveOutcome {
        sha256: new_sha256,
        mtime_ms: mtime_ms(&new_metadata),
        size: content_bytes.len() as u64,
    })
}
