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
    path: PathBuf,
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
    /// If the file does not exist, an empty store is returned.
    /// If the file is corrupt, a corrupt warning is saved and an empty store is returned.
    pub fn load(env: &RuntimeEnv) -> Self {
        let path = match Self::store_path(env) {
            Ok(p) => p,
            Err(_) => PathBuf::from("trusted-workspaces.json"),
        };

        if !path.exists() {
            return Self {
                path,
                data: TrustStoreFile {
                    version: 1,
                    workspaces: BTreeMap::new(),
                },
                corrupt_warning: None,
            };
        }

        match fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<TrustStoreFile>(&content) {
                Ok(data) => Self {
                    path,
                    data,
                    corrupt_warning: None,
                },
                Err(err) => Self {
                    path: path.clone(),
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
                path: path.clone(),
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
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self.data)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
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

        fs::rename(&temp_file, &self.path)?;
        Ok(())
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
}
