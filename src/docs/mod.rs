//! Workspace document preview/edit (SPEC-docs-preview, FR-740..INT-754).
//!
//! Pure, synchronous building blocks shared by the web API
//! (`crate::web::docs_api`) and the TUI (file links). Nothing here knows about
//! HTTP.

pub mod changes;
pub mod confine;
pub mod file;
pub mod links;
pub mod list;
pub mod markdown;
pub mod office;

use std::path::Path;

use serde::{Deserialize, Serialize};

/// NFR-752: read/save/render limit.
pub const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;
/// NFR-752 / FR-741: maximum entries returned by the document list.
pub const MAX_LIST_ENTRIES: usize = 5000;
/// NFR-752 / FR-748: workspace scan limit for per-turn changes.
pub const MAX_SCAN_FILES: usize = 20_000;
/// Directory names never listed, scanned or linked (FR-741, FR-748).
pub const IGNORED_DIRS: &[&str] = &[".git", "target", "node_modules", ".dmuxai"];

/// Document kind derived from the file extension (SPEC §3) or directory (SPEC-docs-tree).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocKind {
    Dir,
    Markdown,
    Text,
    Html,
    Pdf,
    Office,
    Binary,
}

impl DocKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            DocKind::Dir => "dir",
            DocKind::Markdown => "markdown",
            DocKind::Text => "text",
            DocKind::Html => "html",
            DocKind::Pdf => "pdf",
            DocKind::Office => "office",
            DocKind::Binary => "binary",
        }
    }
}

impl PartialEq<&str> for DocKind {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<str> for DocKind {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

const TEXT_EXTENSIONS: &[&str] = &[
    "txt",
    "text",
    "log",
    "json",
    "jsonl",
    "yaml",
    "yml",
    "toml",
    "csv",
    "tsv",
    "ini",
    "cfg",
    "conf",
    "env",
    "xml",
    "svg",
    "rs",
    "py",
    "js",
    "mjs",
    "cjs",
    "ts",
    "tsx",
    "jsx",
    "go",
    "java",
    "kt",
    "c",
    "h",
    "cc",
    "cpp",
    "hpp",
    "cs",
    "rb",
    "php",
    "swift",
    "sh",
    "bash",
    "zsh",
    "ps1",
    "bat",
    "cmd",
    "sql",
    "css",
    "scss",
    "less",
    "vue",
    "svelte",
    "lua",
    "r",
    "scala",
    "dart",
    "gradle",
    "lock",
    "gitignore",
    "dockerfile",
    "makefile",
    "mk",
    "proto",
    "graphql",
    "tex",
    "rst",
    "adoc",
    "org",
];
const OFFICE_EXTENSIONS: &[&str] = &[
    "docx", "doc", "pptx", "ppt", "xlsx", "xls", "odt", "odp", "ods", "rtf",
];
/// Extension-less file names treated as text.
const TEXT_FILE_NAMES: &[&str] = &[
    "makefile",
    "dockerfile",
    "license",
    "readme",
    "changelog",
    ".gitignore",
    ".editorconfig",
];

impl DocKind {
    pub fn for_path(path: &Path) -> DocKind {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        match ext.as_deref() {
            Some("md" | "markdown" | "mdx") => DocKind::Markdown,
            Some("html" | "htm") => DocKind::Html,
            Some("pdf") => DocKind::Pdf,
            Some(e) if OFFICE_EXTENSIONS.contains(&e) => DocKind::Office,
            Some(e) if TEXT_EXTENSIONS.contains(&e) => DocKind::Text,
            None if TEXT_FILE_NAMES.contains(&name.as_str()) => DocKind::Text,
            _ if TEXT_FILE_NAMES.contains(&name.as_str()) => DocKind::Text,
            _ => DocKind::Binary,
        }
    }

    /// Kinds whose content may be edited (FR-744/FR-745). Size and UTF-8
    /// validity are checked separately.
    pub fn editable(self) -> bool {
        matches!(self, DocKind::Markdown | DocKind::Text)
    }

    /// Kinds whose UTF-8 content is returned by the read API (FR-742).
    pub fn has_text_content(self) -> bool {
        matches!(self, DocKind::Markdown | DocKind::Text | DocKind::Html)
    }
}

/// Errors shared by all docs operations; `web::docs_api` maps them to HTTP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocsError {
    /// FR-740 -> 403 `path outside workspace`.
    OutsideWorkspace,
    /// -> 404.
    NotFound,
    /// A directory where a file was expected -> 400.
    NotAFile,
    /// NFR-752 -> 413.
    TooLarge,
    /// FR-745 -> 403 `html is preview-only`.
    HtmlPreviewOnly,
    /// FR-745 -> 415.
    NotEditable,
    /// FR-744 -> 409 with the current hash and mtime.
    Conflict { sha256: String, mtime_ms: u64 },
    /// Content is not valid UTF-8 -> 415.
    NotUtf8,
    /// I/O or other internal failure -> 500 (message is logged, not returned).
    Io(String),
}

impl From<std::io::Error> for DocsError {
    fn from(error: std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::NotFound => DocsError::NotFound,
            _ => DocsError::Io(error.to_string()),
        }
    }
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest.iter() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Milliseconds since the Unix epoch of a file's mtime (0 when unknown).
pub fn mtime_ms(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_follow_extensions() {
        assert_eq!(
            DocKind::for_path(Path::new("a/README.md")),
            DocKind::Markdown
        );
        assert_eq!(DocKind::for_path(Path::new("x.HTML")), DocKind::Html);
        assert_eq!(DocKind::for_path(Path::new("deck.pptx")), DocKind::Office);
        assert_eq!(DocKind::for_path(Path::new("src/main.rs")), DocKind::Text);
        assert_eq!(DocKind::for_path(Path::new("Makefile")), DocKind::Text);
        assert_eq!(DocKind::for_path(Path::new("img.png")), DocKind::Binary);
        assert!(DocKind::Markdown.editable());
        assert!(!DocKind::Html.editable());
        assert!(DocKind::Html.has_text_content());
    }
}
