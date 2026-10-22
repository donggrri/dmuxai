//! Dashboard discovery file (SPEC-agent-api FR-773).
//!
//! A running dashboard writes `<workspace>/.dmuxai/web.json` so a local agent
//! can find the base URL, port, pid and token without parsing CLI output. The
//! file is owner-only on Unix and is removed on clean exit.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Content of `<workspace>/.dmuxai/web.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    pub base_url: String,
    pub port: u16,
    pub pid: u32,
    pub token: String,
}

/// `<root>/.dmuxai/web.json`
pub fn discovery_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".dmuxai").join("web.json")
}

/// Write the discovery file (pretty JSON) with owner-only permissions on Unix.
/// The token must never touch a file with wider permissions: on Unix the body
/// goes to a fresh temp file created with `create_new` + mode 0o600 in the
/// same directory, flushed, then atomically renamed over the final path (a
/// pre-existing `web.json` with mode 0o644 is replaced, not amplified).
pub fn write_discovery(workspace_root: &Path, d: &Discovery) -> io::Result<PathBuf> {
    let dir = workspace_root.join(".dmuxai");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("web.json");
    let body = serde_json::to_string_pretty(d)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    use std::os::unix::fs::OpenOptionsExt;
    let temp_path = dir.join(format!("web.json.{}.tmp", std::process::id()));
    // create_new guarantees a fresh file, so mode(0o600) applies at
    // creation; clear any stale temp left by a failed earlier attempt.
    let _ = std::fs::remove_file(&temp_path);
    let mut temp = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp_path)?;
    temp.write_all(body.as_bytes())?;
    temp.flush()?;
    drop(temp);
    std::fs::rename(&temp_path, &path)?;
    Ok(path)
}

/// Remove the discovery file if present; ignore a missing file.
pub fn remove_discovery(workspace_root: &Path) {
    let _ = std::fs::remove_file(discovery_path(workspace_root));
}

/// Read and parse the discovery file, or `None` when missing or malformed.
pub fn read_discovery(workspace_root: &Path) -> Option<Discovery> {
    let data = std::fs::read_to_string(discovery_path(workspace_root)).ok()?;
    serde_json::from_str(&data).ok()
}

/// Validate a `DMUXAI_WEB_TOKEN` value: `None`/empty means "generate a random
/// token"; otherwise the token must be 32..=128 characters of
/// `[A-Za-z0-9_-]`. An invalid value is an error (the caller warns and falls
/// back to a random token, FR-773).
pub fn token_from_env(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    let valid = (32..=128).contains(&value.len())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if valid {
        Ok(Some(value.to_owned()))
    } else {
        Err(
            "DMUXAI_WEB_TOKEN must be 32-128 characters of A-Z a-z 0-9 _ -; using a random token"
                .to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_file_and_token_env() {
        // TC-774 / FR-773: content, permissions, removal, env token rules.
        let workspace = tempfile::tempdir().unwrap();
        let discovery = Discovery {
            base_url: "http://127.0.0.1".to_owned(),
            port: 7420,
            pid: std::process::id(),
            token: "abcd1234".repeat(5), // 40 chars of [A-Za-z0-9-]
        };

        let path = write_discovery(workspace.path(), &discovery).unwrap();
        assert_eq!(path, discovery_path(workspace.path()));
        assert_eq!(read_discovery(workspace.path()).as_ref(), Some(&discovery));

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("\"token\""), "pretty JSON carries the token");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "discovery file is owner-only");

            // A pre-existing web.json with wide permissions must never leak
            // the new token: the atomic replace rewrites it owner-only.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            let rotated = Discovery {
                base_url: discovery.base_url.clone(),
                port: 7421,
                token: "deadbeef".repeat(5), // fresh 40-char token
                ..discovery.clone()
            };
            write_discovery(workspace.path(), &rotated).unwrap();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "replaced discovery file is owner-only");
            assert_eq!(
                read_discovery(workspace.path()).as_ref(),
                Some(&rotated),
                "new token content after replace"
            );
            let new_body = std::fs::read_to_string(&path).unwrap();
            assert!(
                new_body.contains("\"token\""),
                "pretty JSON carries the token"
            );
            assert!(
                new_body.contains("deadbeef") && !new_body.contains("abcd1234"),
                "old token must be gone"
            );
        }

        remove_discovery(workspace.path());
        assert!(!path.exists());
        assert!(read_discovery(workspace.path()).is_none());
        // Removing a missing file stays silent.
        remove_discovery(workspace.path());

        // token_from_env: absent/empty -> random token (None).
        assert_eq!(token_from_env(None).unwrap(), None);
        assert_eq!(token_from_env(Some("")).unwrap(), None);
        // Valid: 32..=128 chars of [A-Za-z0-9_-].
        let good = "A-z0_9".repeat(6) + "ab"; // 38 chars
        assert_eq!(
            token_from_env(Some(&good)).unwrap().as_deref(),
            Some(good.as_str())
        );
        let min = "a".repeat(32);
        assert_eq!(
            token_from_env(Some(&min)).unwrap().as_deref(),
            Some(min.as_str())
        );
        let max = "b".repeat(128);
        assert_eq!(
            token_from_env(Some(&max)).unwrap().as_deref(),
            Some(max.as_str())
        );
        // Invalid: wrong length or wrong characters -> error, random fallback.
        assert!(token_from_env(Some(&"a".repeat(31))).is_err());
        assert!(token_from_env(Some(&"a".repeat(129))).is_err());
        assert!(token_from_env(Some("short")).is_err());
        assert!(token_from_env(Some(&format!("{good}!"))).is_err());
        assert!(token_from_env(Some(&"a b".repeat(11))).is_err());
    }
}
