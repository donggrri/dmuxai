use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::attachments::attachments_dir;

/// First eight bytes that every PNG file starts with.
pub const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// One clipboard fallback probe. The command must write raw PNG bytes (or
/// base64-encoded PNG bytes for the WSL probe) to stdout and exit with status
/// zero on success; DmuxAI only accepts output starting with the PNG signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardCmd {
    pub program: String,
    pub args: Vec<String>,
}

impl ClipboardCmd {
    pub fn new(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

/// Fallback command list for reading a PNG from the clipboard: Wayland first,
/// then X11. The WSL PowerShell probe (base64 output) runs separately in
/// [`read_clipboard_png`].
pub fn default_clipboard_commands() -> Vec<ClipboardCmd> {
    vec![
        ClipboardCmd::new("wl-paste", ["--type", "image/png"]),
        ClipboardCmd::new(
            "xclip",
            ["-selection", "clipboard", "-t", "image/png", "-o"],
        ),
    ]
}

/// Reads PNG bytes from the clipboard through the default command list.
pub fn read_clipboard_png() -> Option<Vec<u8>> {
    if let Some(bytes) = read_clipboard_png_with(&default_clipboard_commands()) {
        return Some(bytes);
    }
    if is_wsl() {
        return run_powershell_clipboard_png();
    }
    None
}

/// Runs each clipboard command in order and returns the first stdout that
/// starts with the PNG signature. This fallback list is injectable so tests can
/// exercise the whole negotiation with fake commands.
pub fn read_clipboard_png_with(cmds: &[ClipboardCmd]) -> Option<Vec<u8>> {
    cmds.iter().find_map(|cmd| {
        let output = Command::new(&cmd.program)
            .args(&cmd.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if output.status.success() && stdout_starts_with_png(&output.stdout) {
            Some(output.stdout)
        } else {
            None
        }
    })
}

/// Saves clipboard PNG bytes in the DmuxAI workspace attachment directory.
pub fn save_attachment(workspace_root: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let dir = attachments_dir(workspace_root);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("clip-{}.png", uuid::Uuid::new_v4()));
    fs::write(&path, bytes)?;
    Ok(path)
}

fn stdout_starts_with_png(stdout: &[u8]) -> bool {
    stdout.starts_with(&PNG_SIGNATURE)
}

fn is_wsl() -> bool {
    cfg!(target_os = "linux")
        && fs::read_to_string("/proc/version")
            .map(|version| version.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

fn powershell_clipboard_command() -> ClipboardCmd {
    ClipboardCmd::new(
        "powershell.exe",
        [
            "-NoProfile",
            "-Command",
            "Add-Type -AssemblyName System.Windows.Forms; \
             Add-Type -AssemblyName System.Drawing; \
             $img = [System.Windows.Forms.Clipboard]::GetImage(); \
             if ($img -ne $null) { \
             $ms = New-Object System.IO.MemoryStream; \
             $img.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png); \
             [Console]::Out.Write([Convert]::ToBase64String($ms.ToArray())) }",
        ],
    )
}

fn run_powershell_clipboard_png() -> Option<Vec<u8>> {
    let rust_paste_cmd = powershell_clipboard_command();
    let output = Command::new(&rust_paste_cmd.program)
        .args(&rust_paste_cmd.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let decoded = decode_base64(&String::from_utf8_lossy(&output.stdout));
    if stdout_starts_with_png(&decoded) {
        Some(decoded)
    } else {
        None
    }
}

/// Minimal base64 decoder (RFC 4648 alphabet, optional padding) so no new
/// dependency is needed for the WSL probe.
pub fn decode_base64(input: &str) -> Vec<u8> {
    fn digit(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for &byte in input.as_bytes() {
        let Some(value) = digit(byte) else {
            continue;
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const PNG_BYTES: &[u8; 8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    /// FR-CLIP-1 / TC-CLIP-1: a failing probe does not abort the fallback
    /// chain; the next command is tried.
    #[test]
    fn first_failed_command_falls_back_to_next() {
        let cmds = [
            ClipboardCmd::new("sh", ["-c", "exit 1"]),
            ClipboardCmd::new("sh", ["-c", "printf '\\211PNG\\015\\012\\032\\012'"]),
        ];
        assert_eq!(read_clipboard_png_with(&cmds), Some(PNG_BYTES.to_vec()));
    }

    /// FR-CLIP-3 / TC-CLIP-3: only output starting with the PNG signature is
    /// accepted.
    #[test]
    fn non_png_output_is_rejected() {
        let cmds = [ClipboardCmd::new("sh", ["-c", "printf 'not a png'"])];
        assert_eq!(read_clipboard_png_with(&cmds), None);
    }

    /// INP-CLIP-1 / TC-CLIP-5: an empty (or no PNG) full fallback chain yields
    /// no attachment bytes.
    #[test]
    fn all_failed_commands_return_none() {
        let cmds = [
            ClipboardCmd::new("sh", ["-c", "exit 1"]),
            ClipboardCmd::new("sh", ["-c", "printf 'text only'"]),
        ];
        assert_eq!(read_clipboard_png_with(&cmds), None);
    }

    /// INP-CLIP-2 / TC-CLIP-6: the WSL PowerShell probe returns base64-encoded
    /// PNG bytes which must decode back to the PNG signature.
    #[test]
    fn base64_powershell_output_decodes_to_png() {
        assert_eq!(decode_base64("iVBORw0KGgo="), PNG_SIGNATURE.to_vec());
    }

    /// INP-CLIP-2: a probe with empty stdout (image is not a clipboard PNG)
    /// must not be reported as success.
    #[test]
    fn empty_output_is_rejected() {
        let cmds = [ClipboardCmd::new("sh", ["-c", "exit 0"])];
        assert_eq!(read_clipboard_png_with(&cmds), None);
    }

    /// BR-CLIP-1 / TC-CLIP-4: attachments live under
    /// `<workspace>/.dmuxai/attachments/clip-<uuid>.png`.
    #[test]
    fn save_attachment_writes_png_into_workspace_attachments() {
        let dir = tempdir().unwrap();
        let path = save_attachment(dir.path(), PNG_BYTES).unwrap();
        assert!(path.starts_with(attachments_dir(dir.path())));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("clip-")
        );
        assert_eq!(path.extension(), Some(std::ffi::OsStr::new("png")));
        assert_eq!(fs::read(&path).unwrap(), PNG_BYTES.to_vec());
    }
}
