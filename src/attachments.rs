use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

use uuid::Uuid;

/// Default maximum bytes per attached image file.
pub const DEFAULT_MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

const ALLOWED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAttachment {
    pub label: String,
    pub path: PathBuf,
    pub byte_size: u64,
}

#[derive(Debug, Default)]
pub struct ImageAttachmentStore {
    next_index: u32,
    items: Vec<ImageAttachment>,
    max_bytes: u64,
}

impl ImageAttachmentStore {
    pub fn new() -> Self {
        Self {
            next_index: 1,
            items: Vec::new(),
            max_bytes: DEFAULT_MAX_IMAGE_BYTES,
        }
    }

    pub fn with_max_bytes(max_bytes: u64) -> Self {
        Self {
            next_index: 1,
            items: Vec::new(),
            max_bytes,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn items(&self) -> &[ImageAttachment] {
        &self.items
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        self.items.iter().map(|item| item.path.clone()).collect()
    }

    pub fn chip_line(&self) -> String {
        self.items
            .iter()
            .map(|item| format!("[{}]", item.label))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.next_index = 1;
    }

    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.items.len() {
            return false;
        }
        self.items.remove(index);
        true
    }

    pub fn ingest_path(
        &mut self,
        workspace: &Path,
        source: &Path,
    ) -> Result<ImageAttachment, AttachmentError> {
        if !is_supported_image_path(source) {
            return Err(AttachmentError::UnsupportedType(source.to_path_buf()));
        }
        let resolved: PathBuf = if source.is_absolute() {
            source.to_path_buf()
        } else {
            workspace.join(source)
        };
        if !resolved.is_file() {
            return Err(AttachmentError::NotFound(resolved));
        }
        let byte_size = fs::metadata(&resolved)?.len();
        if byte_size > self.max_bytes {
            return Err(AttachmentError::TooLarge {
                path: resolved,
                size: byte_size,
                limit: self.max_bytes,
            });
        }
        let dir = attachments_dir(workspace);
        fs::create_dir_all(&dir)?;
        let extension = resolved
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("png");
        let dest = dir.join(format!("attach-{}.{}", Uuid::new_v4(), extension));
        fs::copy(&resolved, &dest)?;
        Ok(self.push_record(dest, byte_size))
    }

    /// Registers an already-saved file (for example a clipboard PNG written
    /// next to the other attachments) as a pending image.
    pub fn ingest_saved_file(
        &mut self,
        saved: PathBuf,
    ) -> Result<ImageAttachment, AttachmentError> {
        let byte_size = fs::metadata(&saved)?.len();
        if byte_size > self.max_bytes {
            return Err(AttachmentError::TooLarge {
                path: saved.clone(),
                size: byte_size,
                limit: self.max_bytes,
            });
        }
        Ok(self.push_record(saved, byte_size))
    }

    fn push_record(&mut self, path: PathBuf, byte_size: u64) -> ImageAttachment {
        let label = format!("Image #{}", self.next_index);
        self.next_index += 1;
        let attachment = ImageAttachment {
            label,
            path,
            byte_size,
        };
        self.items.push(attachment.clone());
        attachment
    }
}

#[derive(Debug)]
pub enum AttachmentError {
    UnsupportedType(PathBuf),
    NotFound(PathBuf),
    TooLarge {
        path: PathBuf,
        size: u64,
        limit: u64,
    },
    Io(io::Error),
}

impl fmt::Display for AttachmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedType(path) => {
                write!(
                    f,
                    "unsupported image type (use png, jpg, or webp): {}",
                    path.display()
                )
            }
            Self::NotFound(path) => {
                write!(f, "image file not found: {}", path.display())
            }
            Self::TooLarge { path, size, limit } => write!(
                f,
                "image {} is {size} bytes (limit {limit} bytes)",
                path.display()
            ),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for AttachmentError {}

impl From<io::Error> for AttachmentError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn attachments_dir(workspace: &Path) -> PathBuf {
    workspace.join(".dmuxai").join("attachments")
}

pub fn is_supported_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            ALLOWED_EXTENSIONS
                .iter()
                .any(|allowed| ext.eq_ignore_ascii_case(allowed))
        })
        .unwrap_or(false)
}

pub fn image_paths_in_text(text: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for token in text.split_whitespace() {
        let token = token.trim_matches(|ch| ch == '"' || ch == '\'' || ch == ',');
        let token = token.strip_prefix('@').unwrap_or(token);
        let path = PathBuf::from(token);
        if is_supported_image_path(&path) {
            paths.push(path);
        }
    }
    paths
}

pub fn prompt_with_image_references(prompt: &str, paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return prompt.to_owned();
    }
    let refs = paths
        .iter()
        .map(|path| format!("@{}", path.display()))
        .collect::<Vec<_>>()
        .join(" ");
    if prompt.trim().is_empty() {
        refs
    } else {
        format!("{prompt}\n\nAttached images: {refs}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerKind {
    Cursor,
    Agy,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerImageSupport {
    /// Reserved for provider CLIs that accept a separate image flag such as
    /// `--image <path>` (or `-i`); no DmuxAI runner uses one today.
    CliImageFlag,
    /// `@path` references appended to the text prompt (Cursor, AGY, Codex).
    PromptReferences,
    /// DmuxAI does not attach binaries; user sees a notice.
    Unsupported,
}

pub fn runner_image_support(agent: RunnerKind) -> RunnerImageSupport {
    match agent {
        RunnerKind::Cursor | RunnerKind::Agy | RunnerKind::Codex => {
            RunnerImageSupport::PromptReferences
        }
    }
}

pub fn runner_image_support_message(agent: RunnerKind) -> Option<&'static str> {
    match runner_image_support(agent) {
        RunnerImageSupport::Unsupported => {
            Some("This runner does not accept image attachments; images are omitted.")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn rejects_oversized_images() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("big.png");
        fs::write(&file, vec![0u8; 32]).unwrap();
        let mut store = ImageAttachmentStore::with_max_bytes(16);
        assert!(matches!(
            store.ingest_path(dir.path(), &file),
            Err(AttachmentError::TooLarge { .. })
        ));
    }

    #[test]
    fn finds_image_paths_in_prompt_tokens() {
        let paths = image_paths_in_text("see ./shots/ui.png and @docs/x.webp");
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], PathBuf::from("./shots/ui.png"));
        assert_eq!(paths[1], PathBuf::from("docs/x.webp"));
    }

    #[test]
    fn copies_supported_image_into_workspace_store() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("shot.png");
        fs::write(&file, b"\x89PNG").unwrap();
        let mut store = ImageAttachmentStore::new();
        let attachment = store.ingest_path(dir.path(), &file).unwrap();
        assert!(attachment.path.starts_with(attachments_dir(dir.path())));
    }
}
