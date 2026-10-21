use std::path::{Path, PathBuf};

use crate::attachments::attachments_dir;

pub const MAX_WEB_IMAGE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_PROMPT_ATTACHMENTS: usize = 10;

pub fn sniff_image(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.len() >= 8 && bytes[..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        return Some(("png", "image/png"));
    }
    if bytes.len() >= 3 && bytes[..3] == [0xFF, 0xD8, 0xFF] {
        return Some(("jpg", "image/jpeg"));
    }
    if bytes.len() >= 6 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return Some(("gif", "image/gif"));
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && bytes[8..12] == *b"WEBP" {
        return Some(("webp", "image/webp"));
    }
    None
}

fn is_lower_hex(b: u8) -> bool {
    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
}

fn valid_uuid(s: &str) -> bool {
    if s.len() == 32 {
        return s.bytes().all(is_lower_hex);
    }
    if s.len() == 36 {
        for (i, b) in s.bytes().enumerate() {
            if i == 8 || i == 13 || i == 18 || i == 23 {
                if b != b'-' {
                    return false;
                }
            } else if !is_lower_hex(b) {
                return false;
            }
        }
        return true;
    }
    false
}

pub fn is_valid_attachment_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let rest = if let Some(r) = name.strip_prefix("web-") {
        r
    } else if let Some(r) = name.strip_prefix("clip-") {
        r
    } else if let Some(r) = name.strip_prefix("attach-") {
        r
    } else {
        return false;
    };

    let dot = match rest.find('.') {
        Some(i) => i,
        None => return false,
    };
    if rest[dot + 1..].contains('.') {
        return false;
    }
    let uuid_part = &rest[..dot];
    let ext = &rest[dot + 1..];
    if !matches!(ext, "png" | "jpg" | "jpeg" | "gif" | "webp") {
        return false;
    }
    if uuid_part.is_empty() {
        return false;
    }
    valid_uuid(uuid_part)
}

pub fn mime_for_name(name: &str) -> Option<&'static str> {
    let ext = name.rsplit('.').next()?;
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum UploadError {
    Empty,
    TooLarge,
    UnsupportedType,
    Io(String),
}

pub struct SavedAttachment {
    pub name: String,
    pub path: PathBuf,
    pub bytes: usize,
    pub mime: &'static str,
}

pub fn save_web_attachment(workspace: &Path, bytes: &[u8]) -> Result<SavedAttachment, UploadError> {
    if bytes.is_empty() {
        return Err(UploadError::Empty);
    }
    if bytes.len() > MAX_WEB_IMAGE_BYTES {
        return Err(UploadError::TooLarge);
    }
    let (ext, mime) = sniff_image(bytes).ok_or(UploadError::UnsupportedType)?;
    let dir = attachments_dir(workspace);
    std::fs::create_dir_all(&dir).map_err(|e| UploadError::Io(e.to_string()))?;
    let name = format!("web-{}.{}", uuid::Uuid::new_v4().simple(), ext);
    let path = dir.join(&name);
    std::fs::write(&path, bytes).map_err(|e| UploadError::Io(e.to_string()))?;
    Ok(SavedAttachment {
        name,
        path,
        bytes: bytes.len(),
        mime,
    })
}

pub fn resolve_attachment(workspace: &Path, name: &str) -> Option<PathBuf> {
    if !workspace.is_absolute() || !is_valid_attachment_name(name) {
        return None;
    }
    let dir = attachments_dir(workspace);
    let path = dir.join(name);
    // `symlink_metadata` so a symlink planted in the attachments dir is never followed.
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() {
        return None;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn sniff_each_type() {
        assert_eq!(
            sniff_image(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
                .unwrap()
                .0,
            "png"
        );
        assert_eq!(sniff_image(&[0xFF, 0xD8, 0xFF, 0x00]).unwrap().0, "jpg");
        assert_eq!(sniff_image(b"GIF89a").unwrap().0, "gif");
        let mut webp = vec![0u8; 12];
        webp[..4].copy_from_slice(b"RIFF");
        webp[8..12].copy_from_slice(b"WEBP");
        assert_eq!(sniff_image(&webp).unwrap().0, "webp");
        assert!(sniff_image(b"hello").is_none());
    }

    #[test]
    fn valid_and_invalid_names() {
        let u = uuid::Uuid::new_v4();
        let simple = u.simple().to_string();
        let hyph = u.to_string();
        assert!(is_valid_attachment_name(&format!("web-{}.png", simple)));
        assert!(is_valid_attachment_name(&format!("clip-{}.jpg", hyph)));
        assert!(is_valid_attachment_name(&format!("attach-{}.webp", simple)));
        assert!(!is_valid_attachment_name("../x.png"));
        assert!(!is_valid_attachment_name(&format!(
            "web-{}.png/../a",
            simple
        )));
        assert!(!is_valid_attachment_name(&format!("web-{}.exe", simple)));
        assert!(!is_valid_attachment_name(&format!("WEB-{}.png", simple)));
        assert!(!is_valid_attachment_name("a.png"));
        assert!(!is_valid_attachment_name(""));
    }

    #[test]
    fn save_writes_generated_name() {
        let dir = tempdir().unwrap();
        let bytes = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let saved = save_web_attachment(dir.path(), &bytes).unwrap();
        assert!(saved.name.starts_with("web-"));
        assert!(saved.name.ends_with(".png"));
        assert!(
            saved
                .path
                .starts_with(dir.path().join(".dmuxai").join("attachments"))
        );
        assert!(saved.path.is_file());
    }

    #[test]
    fn too_large() {
        let dir = tempdir().unwrap();
        let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.resize(MAX_WEB_IMAGE_BYTES + 1, 0);
        assert!(matches!(
            save_web_attachment(dir.path(), &bytes),
            Err(UploadError::TooLarge)
        ));
    }

    #[test]
    fn unsupported_type() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            save_web_attachment(dir.path(), b"hello"),
            Err(UploadError::UnsupportedType)
        ));
    }

    #[test]
    fn resolve_missing_and_invalid() {
        let dir = tempdir().unwrap();
        assert!(resolve_attachment(dir.path(), "nope.png").is_none());
        let u = uuid::Uuid::new_v4().simple().to_string();
        assert!(resolve_attachment(dir.path(), &format!("../{}.png", u)).is_none());
        // A valid name that does not exist yet.
        assert!(resolve_attachment(dir.path(), &format!("web-{u}.png")).is_none());
        // A relative workspace is never resolved.
        assert!(resolve_attachment(Path::new("rel"), &format!("web-{u}.png")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_refuses_symlinks() {
        let dir = tempdir().unwrap();
        let attach = attachments_dir(dir.path());
        std::fs::create_dir_all(&attach).unwrap();
        let target = dir.path().join("secret.png");
        std::fs::write(&target, b"x").unwrap();
        let u = uuid::Uuid::new_v4().simple().to_string();
        let link = attach.join(format!("web-{u}.png"));
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(resolve_attachment(dir.path(), &format!("web-{u}.png")).is_none());
    }
}
