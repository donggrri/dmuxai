use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::runtime_env::RuntimeEnv;
use crate::workspace::normalize_workspace_path;

/// Calculates the sha256 hex digest of raw bytes.
pub fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

/// Source of trust decision when trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustSource {
    Record,
    OptIn,
}

/// Decision on whether workspace configuration is trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDecision {
    Trusted(TrustSource),
    Untrusted,
}

/// Status of workspace config trust on DmuxSettings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WorkspaceTrustStatus {
    NoWorkspaceConfig,
    NotNeeded,
    Trusted(TrustSource),
    Untrusted {
        sha256: String,
        ignored: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTrustEntry {
    pub sha256: String,
    pub trusted_at: String,
}

fn default_store_version() -> u32 {
    1
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustStoreFile {
    #[serde(default = "default_store_version")]
    pub version: u32,
    #[serde(default)]
    pub workspaces: BTreeMap<String, WorkspaceTrustEntry>,
}

pub struct TrustStore {
    path: Option<PathBuf>,
    data: TrustStoreFile,
    corrupt_warning: Option<String>,
}

impl TrustStore {
    /// Resolves the trusted-workspaces.json path using RuntimeEnv.
    pub fn store_path(env: &RuntimeEnv) -> io::Result<PathBuf> {
        let base = if let Some(dir) = &env.dmuxai_state_dir {
            dir.clone()
        } else if let Some(state_home) = &env.xdg_state_home {
            state_home.join("dmuxai")
        } else if let Some(home) = env.home_dir() {
            home.join(".local").join("state").join("dmuxai")
        } else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "neither DMUXAI_STATE_DIR nor XDG_STATE_HOME nor HOME is configured",
            ));
        };
        Ok(base.join("trusted-workspaces.json"))
    }

    /// Loads the trust store file from the environment-configured state directory.
    /// If no state directory is configured, an empty store with no path is returned.
    /// If the file does not exist, an empty store is returned.
    /// If the file is corrupt or has invalid permissions, a warning is saved and an empty store is returned.
    pub fn load(env: &RuntimeEnv) -> Self {
        let path = match Self::store_path(env) {
            Ok(p) => p,
            Err(_) => {
                return Self {
                    path: None,
                    data: TrustStoreFile {
                        version: 1,
                        workspaces: BTreeMap::new(),
                    },
                    corrupt_warning: None,
                };
            }
        };

        if !path.exists() {
            return Self {
                path: Some(path),
                data: TrustStoreFile {
                    version: 1,
                    workspaces: BTreeMap::new(),
                },
                corrupt_warning: None,
            };
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(meta) = fs::metadata(&path) {
                let euid = unsafe { libc::geteuid() };
                if meta.uid() != euid {
                    return Self {
                        path: Some(path.clone()),
                        data: TrustStoreFile {
                            version: 1,
                            workspaces: BTreeMap::new(),
                        },
                        corrupt_warning: Some(format!(
                            "trust store {} is not owned by current user; ignoring",
                            path.display()
                        )),
                    };
                }
                if (meta.mode() & 0o022) != 0 {
                    return Self {
                        path: Some(path.clone()),
                        data: TrustStoreFile {
                            version: 1,
                            workspaces: BTreeMap::new(),
                        },
                        corrupt_warning: Some(format!(
                            "trust store {} has unsafe group/world-writable permissions (mode {:04o}); ignoring",
                            path.display(),
                            meta.mode() & 0o777
                        )),
                    };
                }
            }
        }

        match fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<TrustStoreFile>(&content) {
                Ok(data) => Self {
                    path: Some(path),
                    data,
                    corrupt_warning: None,
                },
                Err(err) => Self {
                    path: Some(path.clone()),
                    data: TrustStoreFile {
                        version: 1,
                        workspaces: BTreeMap::new(),
                    },
                    corrupt_warning: Some(format!(
                        "trust store {} is corrupt ({err}); treating as untrusted",
                        path.display()
                    )),
                },
            },
            Err(err) => Self {
                path: Some(path.clone()),
                data: TrustStoreFile {
                    version: 1,
                    workspaces: BTreeMap::new(),
                },
                corrupt_warning: Some(format!(
                    "failed to read trust store {}: {err}",
                    path.display()
                )),
            },
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn corrupt_warning(&self) -> Option<&str> {
        self.corrupt_warning.as_deref()
    }

    fn normalize_key(workspace: &Path) -> String {
        normalize_workspace_path(workspace)
            .unwrap_or_else(|_| workspace.to_string_lossy().into_owned())
    }

    pub fn is_trusted(&self, workspace: &Path, sha256: &str) -> bool {
        let key = Self::normalize_key(workspace);
        self.data
            .workspaces
            .get(&key)
            .is_some_and(|entry| entry.sha256 == sha256)
    }

    pub fn trust(&mut self, workspace: &Path, sha256: &str) {
        let key = Self::normalize_key(workspace);
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.data.workspaces.insert(
            key,
            WorkspaceTrustEntry {
                sha256: sha256.to_ascii_lowercase(),
                trusted_at: now.to_string(),
            },
        );
    }

    pub fn revoke(&mut self, workspace: &Path) -> bool {
        let key = Self::normalize_key(workspace);
        self.data.workspaces.remove(&key).is_some()
    }

    pub fn save(&self) -> io::Result<()> {
        let store_path = self.path.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "trust store directory not configured; unable to persist trust",
            )
        })?;

        if let Some(warning) = &self.corrupt_warning {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("refusing to overwrite trust store with warning: {warning}"),
            ));
        }

        if let Some(parent) = store_path.parent() {
            create_state_dir(parent)?;
        }
        let json = serde_json::to_string_pretty(&self.data)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let parent = store_path.parent().unwrap_or_else(|| Path::new("."));
        let temp_file = parent.join(format!(
            ".trusted-workspaces-{}-{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));

        #[cfg(unix)]
        {
            use std::fs::OpenOptions;
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp_file)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
        }

        #[cfg(not(unix))]
        {
            use std::io::Write;
            let mut file = fs::File::create(&temp_file)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
        }

        fs::rename(&temp_file, store_path)?;
        Ok(())
    }
}

fn create_state_dir(path: &Path) -> io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        create_state_dir(parent)?;
    }
    match fs::create_dir(path) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// Decides whether the workspace config is trusted.
pub fn decide(env: &RuntimeEnv, workspace: &Path, sha256: &str) -> TrustDecision {
    let store = TrustStore::load(env);
    if store.is_trusted(workspace, sha256) {
        TrustDecision::Trusted(TrustSource::Record)
    } else if env.trust_workspace_config {
        TrustDecision::Trusted(TrustSource::OptIn)
    } else {
        TrustDecision::Untrusted
    }
}

/// Outcome of a dashboard save of the workspace `dmuxai.toml` (FR-2226..FR-2229).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustRefresh {
    /// Folder was trusted by a persistent record matching the on-disk file right before the write; record now points at the new hash.
    Refreshed,
    /// Refresh not attempted (not allowed, no matching record, file changed since the caller read it, store unusable).
    NotEligible,
    /// File was written but saving the store failed; trust is left as-is (so the folder is now untrusted).
    StoreSaveFailed(String),
}

/// Writes `updated` to `<workspace>/dmuxai.toml` and, when strictly eligible, refreshes the persistent trust record to the new content hash.
pub fn write_workspace_config_refreshing_trust(
    env: &RuntimeEnv,
    workspace: &Path,
    based_on: &str,
    updated: &str,
    allow_refresh: bool,
) -> io::Result<TrustRefresh> {
    let path = workspace.join("dmuxai.toml");
    let on_disk = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };

    let mut eligible_store = None;
    if allow_refresh
        && let Some(bytes) = &on_disk
        && content_hash(bytes) == content_hash(based_on.as_bytes())
    {
        let store = TrustStore::load(env);
        if store.path().is_some()
            && store.corrupt_warning().is_none()
            && store.is_trusted(workspace, &content_hash(bytes))
        {
            eligible_store = Some(store);
        }
    }

    fs::write(&path, updated)?;

    if let Some(mut store) = eligible_store {
        store.trust(workspace, &content_hash(updated.as_bytes()));
        match store.save() {
            Ok(()) => Ok(TrustRefresh::Refreshed),
            Err(e) => Ok(TrustRefresh::StoreSaveFailed(e.to_string())),
        }
    } else {
        Ok(TrustRefresh::NotEligible)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn tc_2190_content_hash_stable_and_hex() {
        let h1 = content_hash(b"hello world\n");
        let h2 = content_hash(b"hello world\n");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
        // Known sha256 for "hello world\n"
        assert_eq!(
            h1,
            "a948904f2f0f479b8f8197694b30184b0d2ed1c1cd2a1ec0fb85d299a192a447"
        );
    }

    #[test]
    fn tc_2191_store_path_resolution_precedence() {
        let dir = tempdir().unwrap();
        let state_dir = dir.path().join("custom-state");
        let xdg_state = dir.path().join("xdg-state");
        let home_dir = dir.path().join("home");

        // 1. DMUXAI_STATE_DIR takes precedence
        let env1 = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir.clone()),
            xdg_state_home: Some(xdg_state.clone()),
            home: Some(home_dir.clone()),
            ..RuntimeEnv::default()
        };
        assert_eq!(
            TrustStore::store_path(&env1).unwrap(),
            state_dir.join("trusted-workspaces.json")
        );

        // 2. XDG_STATE_HOME takes precedence when DMUXAI_STATE_DIR is None
        let env2 = RuntimeEnv {
            dmuxai_state_dir: None,
            xdg_state_home: Some(xdg_state.clone()),
            home: Some(home_dir.clone()),
            ..RuntimeEnv::default()
        };
        assert_eq!(
            TrustStore::store_path(&env2).unwrap(),
            xdg_state.join("dmuxai").join("trusted-workspaces.json")
        );

        // 3. HOME/.local/state/dmuxai fallback
        let env3 = RuntimeEnv {
            dmuxai_state_dir: None,
            xdg_state_home: None,
            home: Some(home_dir.clone()),
            ..RuntimeEnv::default()
        };
        assert_eq!(
            TrustStore::store_path(&env3).unwrap(),
            home_dir
                .join(".local")
                .join("state")
                .join("dmuxai")
                .join("trusted-workspaces.json")
        );

        // 4. Neither configured -> error
        let env4 = RuntimeEnv {
            dmuxai_state_dir: None,
            xdg_state_home: None,
            home: None,
            ..RuntimeEnv::default()
        };
        assert!(TrustStore::store_path(&env4).is_err());
    }

    #[test]
    fn tc_2192_trust_save_load_round_trip() {
        let dir = tempdir().unwrap();
        let ws = dir.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(dir.path().join("state")),
            ..RuntimeEnv::default()
        };

        let mut store = TrustStore::load(&env);
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        store.trust(&ws, hash);
        store.save().expect("save store");

        let loaded = TrustStore::load(&env);
        assert!(loaded.is_trusted(&ws, hash));
        assert!(loaded.corrupt_warning().is_none());
    }

    #[test]
    fn tc_2193_is_trusted_false_when_hash_or_path_differs() {
        let dir = tempdir().unwrap();
        let ws1 = dir.path().join("ws1");
        let ws2 = dir.path().join("ws2");
        fs::create_dir_all(&ws1).unwrap();
        fs::create_dir_all(&ws2).unwrap();
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(dir.path().join("state")),
            ..RuntimeEnv::default()
        };

        let mut store = TrustStore::load(&env);
        let hash1 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let hash2 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        store.trust(&ws1, hash1);

        assert!(store.is_trusted(&ws1, hash1));
        // Different hash
        assert!(!store.is_trusted(&ws1, hash2));
        // Different path
        assert!(!store.is_trusted(&ws2, hash1));
    }

    #[test]
    fn tc_2194_revoke() {
        let dir = tempdir().unwrap();
        let ws = dir.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(dir.path().join("state")),
            ..RuntimeEnv::default()
        };

        let mut store = TrustStore::load(&env);
        let hash = "1111111111111111111111111111111111111111111111111111111111111111";
        store.trust(&ws, hash);
        assert!(store.is_trusted(&ws, hash));

        let revoked = store.revoke(&ws);
        assert!(revoked);
        assert!(!store.is_trusted(&ws, hash));

        let revoked_again = store.revoke(&ws);
        assert!(!revoked_again);
    }

    #[test]
    fn tc_2195_corrupt_json_handling() {
        let dir = tempdir().unwrap();
        let state_dir = dir.path().join("state");
        fs::create_dir_all(&state_dir).unwrap();
        let store_file = state_dir.join("trusted-workspaces.json");
        fs::write(&store_file, "INVALID JSON {{{{").unwrap();

        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            ..RuntimeEnv::default()
        };

        let store = TrustStore::load(&env);
        assert!(store.corrupt_warning().is_some());
        assert!(store.corrupt_warning().unwrap().contains("is corrupt"));
        let ws = dir.path().join("ws");
        assert!(!store.is_trusted(&ws, "anyhash"));
    }

    #[cfg(unix)]
    #[test]
    fn tc_2196_saved_file_mode_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir.clone()),
            ..RuntimeEnv::default()
        };

        let mut store = TrustStore::load(&env);
        store.trust(
            &dir.path().join("ws"),
            "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        );
        store.save().unwrap();

        let store_path = state_dir.join("trusted-workspaces.json");
        let metadata = fs::metadata(&store_path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn tc_2197_decide_logic() {
        let dir = tempdir().unwrap();
        let ws = dir.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let state_dir = dir.path().join("state");
        let store_file = state_dir.join("trusted-workspaces.json");
        let hash = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

        // 1. Untrusted initially
        let mut env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir.clone()),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };
        assert_eq!(decide(&env, &ws, hash), TrustDecision::Untrusted);

        // 2. Opt-in -> Trusted(OptIn) and does not create the store file
        env.trust_workspace_config = true;
        assert_eq!(
            decide(&env, &ws, hash),
            TrustDecision::Trusted(TrustSource::OptIn)
        );
        assert!(!store_file.exists());

        // 3. Record in store -> Trusted(Record)
        env.trust_workspace_config = false;
        let mut store = TrustStore::load(&env);
        store.trust(&ws, hash);
        store.save().unwrap();
        assert!(store_file.exists());

        assert_eq!(
            decide(&env, &ws, hash),
            TrustDecision::Trusted(TrustSource::Record)
        );
    }

    #[test]
    fn tc_2217_unset_state_and_home_returns_untrusted_and_no_store_path() {
        let env = RuntimeEnv {
            dmuxai_state_dir: None,
            xdg_state_home: None,
            home: None,
            userprofile: None,
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };
        let ws = Path::new("/some/workspace");
        let hash = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

        let store = TrustStore::load(&env);
        assert!(store.path().is_none());
        assert!(!store.is_trusted(ws, hash));
        assert_eq!(decide(&env, ws, hash), TrustDecision::Untrusted);

        // Opt-in env still works
        let opt_in_env = RuntimeEnv {
            trust_workspace_config: true,
            ..env
        };
        assert_eq!(
            decide(&opt_in_env, ws, hash),
            TrustDecision::Trusted(TrustSource::OptIn)
        );
    }

    #[cfg(unix)]
    #[test]
    fn tc_2218_state_dir_created_with_0700() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let state_dir = dir.path().join("new_state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir.clone()),
            ..RuntimeEnv::default()
        };

        let mut store = TrustStore::load(&env);
        store.trust(
            &dir.path().join("ws"),
            "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        );
        store.save().unwrap();

        assert!(state_dir.is_dir());
        let meta = fs::metadata(&state_dir).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn tc_2219_group_writable_trust_store_ignored_with_warning() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let state_dir = dir.path().join("state");
        fs::create_dir_all(&state_dir).unwrap();
        let store_file = state_dir.join("trusted-workspaces.json");

        // Write a valid trust store
        let ws = dir.path().join("ws");
        let hash = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let valid_json = format!(
            r#"{{"version": 1, "workspaces": {{"{}": {{"sha256": "{}", "trusted_at": "123"}}}}}}"#,
            ws.display(),
            hash
        );
        fs::write(&store_file, valid_json).unwrap();

        // Make it group-writable (e.g. 0660)
        fs::set_permissions(&store_file, fs::Permissions::from_mode(0o660)).unwrap();

        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            ..RuntimeEnv::default()
        };

        let store = TrustStore::load(&env);
        assert!(store.corrupt_warning().is_some());
        assert!(
            store
                .corrupt_warning()
                .unwrap()
                .contains("unsafe group/world-writable permissions")
        );
        // Treated as empty/untrusted
        assert!(!store.is_trusted(&ws, hash));

        // Saving should refuse with an error rather than overwrite it
        assert!(store.save().is_err());
    }

    #[test]
    fn tc_2226_trusted_save_refreshes_record() {
        let dir = tempdir().unwrap();
        let ws = dir.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            ..RuntimeEnv::default()
        };

        let initial = "a = 1\n";
        fs::write(ws.join("dmuxai.toml"), initial).unwrap();
        let mut store = TrustStore::load(&env);
        store.trust(&ws, &content_hash(initial.as_bytes()));
        store.save().unwrap();

        let updated = "a = 2\n";
        let res = write_workspace_config_refreshing_trust(&env, &ws, initial, updated, true)
            .expect("write config");
        assert_eq!(res, TrustRefresh::Refreshed);

        let reloaded = TrustStore::load(&env);
        assert!(reloaded.is_trusted(&ws, &content_hash(updated.as_bytes())));
        assert!(!reloaded.is_trusted(&ws, &content_hash(initial.as_bytes())));
        assert_eq!(
            fs::read_to_string(ws.join("dmuxai.toml")).unwrap(),
            "a = 2\n"
        );
    }

    #[test]
    fn tc_2227_untrusted_save_stays_untrusted() {
        let dir = tempdir().unwrap();
        let ws = dir.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let state_dir = dir.path().join("state");
        let store_file = state_dir.join("trusted-workspaces.json");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            ..RuntimeEnv::default()
        };

        let initial = "a = 1\n";
        fs::write(ws.join("dmuxai.toml"), initial).unwrap();

        let updated = "a = 2\n";
        let res = write_workspace_config_refreshing_trust(&env, &ws, initial, updated, true)
            .expect("write config");
        assert_eq!(res, TrustRefresh::NotEligible);
        assert_eq!(
            fs::read_to_string(ws.join("dmuxai.toml")).unwrap(),
            "a = 2\n"
        );
        assert!(!store_file.exists());
    }

    #[test]
    fn tc_2228_opt_in_flag_does_not_persist_trust() {
        let dir = tempdir().unwrap();
        let ws = dir.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let state_dir = dir.path().join("state");
        let store_file = state_dir.join("trusted-workspaces.json");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir.clone()),
            trust_workspace_config: true,
            ..RuntimeEnv::default()
        };

        let initial = "a = 1\n";
        fs::write(ws.join("dmuxai.toml"), initial).unwrap();

        let updated = "a = 2\n";
        let res = write_workspace_config_refreshing_trust(&env, &ws, initial, updated, true)
            .expect("write config");
        assert_eq!(res, TrustRefresh::NotEligible);
        assert!(!store_file.exists());

        let flag_off_env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            trust_workspace_config: false,
            ..RuntimeEnv::default()
        };
        assert_eq!(
            decide(&flag_off_env, &ws, &content_hash(updated.as_bytes())),
            TrustDecision::Untrusted
        );
    }

    #[test]
    fn tc_2229_external_edit_blocks_refresh() {
        let dir = tempdir().unwrap();
        let ws = dir.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let state_dir = dir.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state_dir),
            ..RuntimeEnv::default()
        };

        let initial = "a = 1\n";
        let edited = "a = 1\n# edited\n";
        let updated = "a = 2\n";

        // (i) Trust hash of "a = 1\n", overwrite with edited on disk.
        // Call with based_on = "a = 1\n# edited\n" -> NotEligible (stored record doesn't match on-disk file).
        fs::write(ws.join("dmuxai.toml"), edited).unwrap();
        let mut store = TrustStore::load(&env);
        store.trust(&ws, &content_hash(initial.as_bytes()));
        store.save().unwrap();

        let res1 = write_workspace_config_refreshing_trust(&env, &ws, edited, updated, true)
            .expect("write config");
        assert_eq!(res1, TrustRefresh::NotEligible);
        let store1 = TrustStore::load(&env);
        assert!(!store1.is_trusted(&ws, &content_hash(updated.as_bytes())));
        assert!(store1.is_trusted(&ws, &content_hash(initial.as_bytes())));

        // (ii) Separately, based_on = "a = 1\n" but disk has the edited text -> NotEligible.
        fs::write(ws.join("dmuxai.toml"), edited).unwrap();
        let res2 = write_workspace_config_refreshing_trust(&env, &ws, initial, updated, true)
            .expect("write config");
        assert_eq!(res2, TrustRefresh::NotEligible);
        let store2 = TrustStore::load(&env);
        assert!(!store2.is_trusted(&ws, &content_hash(updated.as_bytes())));
        assert!(store2.is_trusted(&ws, &content_hash(initial.as_bytes())));

        // (iii) allow_refresh = false with a matching record -> NotEligible and old record unchanged.
        fs::write(ws.join("dmuxai.toml"), initial).unwrap();
        let res3 = write_workspace_config_refreshing_trust(&env, &ws, initial, updated, false)
            .expect("write config");
        assert_eq!(res3, TrustRefresh::NotEligible);
        let store3 = TrustStore::load(&env);
        assert!(!store3.is_trusted(&ws, &content_hash(updated.as_bytes())));
        assert!(store3.is_trusted(&ws, &content_hash(initial.as_bytes())));
        assert_eq!(
            fs::read_to_string(ws.join("dmuxai.toml")).unwrap(),
            "a = 2\n"
        );
    }

    #[test]
    fn tc_2233_write_failure_leaves_trust_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("proj");
        fs::create_dir_all(&ws).unwrap();
        // create ws/dmuxai.toml as a DIRECTORY so fs::write fails
        fs::create_dir_all(ws.join("dmuxai.toml")).unwrap();

        let state = tmp.path().join("state");
        let env = RuntimeEnv {
            dmuxai_state_dir: Some(state),
            ..Default::default()
        };

        let mut s = TrustStore::load(&env);
        s.trust(&ws, "x");
        s.save().unwrap();

        let store_file = s.path().expect("store path");
        let recorded_bytes = fs::read(store_file).unwrap();

        let res = write_workspace_config_refreshing_trust(&env, &ws, "initial", "updated", true);
        assert!(res.is_err());

        let current_bytes = fs::read(store_file).unwrap();
        assert_eq!(recorded_bytes, current_bytes);
    }
}
