use std::{error::Error, fmt};

/// Copies text through the native clipboard implementation on Windows and Linux.
///
/// A clipboard connection is deliberately opened for each copy. This keeps the
/// TUI's application state independent of platform clipboard handles and makes
/// clipboard failures recoverable at the call site.
pub fn copy_text(text: impl Into<String>) -> Result<(), ClipboardError> {
    let mut clipboard = arboard::Clipboard::new().map_err(ClipboardError::new)?;
    clipboard.set_text(text.into()).map_err(ClipboardError::new)
}

/// A platform-neutral clipboard error suitable for display in the TUI.
#[derive(Debug)]
pub struct ClipboardError {
    message: String,
}

impl ClipboardError {
    fn new(error: arboard::Error) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

impl fmt::Display for ClipboardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "clipboard error: {}", self.message)
    }
}

impl Error for ClipboardError {}
