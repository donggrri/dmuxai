//! Unit/integration tests for proposed `dmuxai::run_journal` storage API (FR-2560..FR-2566).
//!
//! # Proposed `dmuxai::run_journal` Minimal Clean API:
//!
//! ```rust,ignore
//! pub struct JournalConfig {
//!     pub max_age_ms: u64,           // Default: 90 days (90 * 86_400 * 1000)
//!     pub max_total_bytes: u64,      // Default: 1 GiB (1024 * 1024 * 1024)
//! }
//!
//! pub struct PruneReport {
//!     pub deleted_runs: usize,
//!     pub reclaimed_bytes: u64,
//! }
//!
//! pub struct JournalStore {
//!     root: PathBuf,
//! }
//!
//! impl JournalStore {
//!     pub fn new(root: PathBuf) -> Self;
//!     pub fn save(&self, manifest: &RunManifest) -> Result<(), JournalError>;
//!     pub fn load(&self, id: &str) -> Result<RunManifest, JournalError>;
//!     pub fn write_diagnostics(&self, id: &str, tail: &str) -> Result<(), JournalError>;
//!     pub fn load_diagnostics(&self, id: &str) -> Result<String, JournalError>;
//!     pub fn prune(&self, config: &JournalConfig, now_ms: u64) -> Result<PruneReport, JournalError>;
//! }
//! ```
//!
//! Design Notes:
//! - Complete snapshot diagnostics API (`write_diagnostics` replaces/creates bounded raw tail up to 64 KiB)
//!   which avoids unbounded append chunk semantics and keeps split-token mask boundary handling safe and deterministic.
//! - Manifest read bounded at 128 KiB; files larger than 128 KiB fail to load with `JournalError::CorruptManifest` / `SizeExceeded`.
//! - Masks secrets before writing to disk (both in manifest and diagnostics).
//! - Symlinks at root, run directory, or manifest/diagnostics files are rejected without reading or modifying symlink targets.
//! - Retention pruning deletes oldest terminal runs until within age and size limits; active and corrupt runs are preserved.

use serde_json::json;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

// Before dmuxai::run_journal is implemented, this import will COMPILE-FAIL as required.
use dmuxai::run_journal::{JournalConfig, JournalStore, RunManifest};

fn sample_manifest(
    run_id: &str,
    status: &str,
    created_at_ms: u64,
    completed_at_ms: Option<u64>,
) -> RunManifest {
    let val = json!({
        "schema_version": 1,
        "run_id": run_id,
        "origin": "cli_headless",
        "status": status,
        "created_at_ms": created_at_ms,
        "completed_at_ms": completed_at_ms,
        "duration_ms": completed_at_ms.map(|c| c.saturating_sub(created_at_ms)),
        "requested_runner": "codex",
        "resolved_runner": "codex",
        "requested_model": null,
        "resolved_model": null,
        "dmuxai_version": "0.3.0",
        "build_commit": null,
        "child_exit_code": if status == "completed" { Some(0) } else { None },
        "child_signal": null,
        "error_kind": null,
        "error_summary": null,
        "links": {
            "conversation_id": null,
            "turn_id": null,
            "graph_id": null,
            "node_id": null,
            "attempt_index": null
        },
        "diagnostics": {
            "stderr_bytes": 0,
            "tail_sample": null
        },
        "end_summary": null
    });
    serde_json::from_value(val).expect("valid manifest fixture")
}

/// TC-2560: Atomic save/load, private modes at actual created paths, overwritten manifest complete JSON.
#[test]
fn tc_2560_store_atomic_save_load_permissions_and_overwrite() {
    let tmp = TempDir::new().expect("tempdir");
    let store_root = tmp.path().join("runs");
    let store = JournalStore::new(store_root.clone());

    let run_id = "0123456789abcdef";
    let manifest = sample_manifest(run_id, "running", 1_000_000, None);

    // Initial save
    store.save(&manifest).expect("save initial manifest");

    let run_dir = store_root.join(run_id);
    let manifest_path = run_dir.join("manifest.json");
    assert!(run_dir.is_dir());
    assert!(manifest_path.is_file());

    #[cfg(unix)]
    {
        let dir_mode = fs::metadata(&run_dir).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(&manifest_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "run directory mode must be 0700");
        assert_eq!(file_mode, 0o600, "manifest file mode must be 0600");
    }

    // Load initial - running status is normalized to authoritative active
    let loaded = store.load(run_id).expect("load manifest");
    assert_eq!(loaded.run_id, run_id);
    assert_eq!(loaded.status, "active");

    // Overwrite manifest with terminal state
    let mut updated = manifest.clone();
    updated.status = "completed".to_string();
    updated.completed_at_ms = Some(1_005_000);
    updated.duration_ms = Some(5_000);
    store.save(&updated).expect("overwrite manifest atomically");

    // Check complete valid JSON on disk and no temp files left behind
    let raw = fs::read_to_string(&manifest_path).expect("read manifest");
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).expect("must be valid complete json");
    assert_eq!(parsed["status"], "completed");
    assert_eq!(parsed["completed_at_ms"], 1_005_000);

    let entries: Vec<_> = fs::read_dir(&run_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(
        entries,
        vec!["manifest.json"],
        "atomic tmp files must not linger"
    );

    let loaded2 = store.load(run_id).expect("load overwritten manifest");
    assert_eq!(loaded2.status, "completed");
}

/// TC-2561: Whole metadata and diagnostic pre-disk masking.
/// Ensures raw saved manifest and diagnostics bytes do NOT contain synthetic secrets.
#[test]
fn tc_2561_store_pre_disk_masking_secrets_and_urls() {
    let tmp = TempDir::new().expect("tempdir");
    let store_root = tmp.path().join("runs");
    let store = JournalStore::new(store_root.clone());

    let run_id = "0123456789abcdef";

    // Secrets to test
    let secret_bearer = "sk-1234567890abcdef";
    let secret_api_key_space = "my super secret key with space";
    let secret_basic = "dXNlcjpwYXNz";
    let secret_url_token = "tok_query_param_999";
    let secret_bearer_short = "shorttoken";
    let secret_json = "json_secret_pass";
    let secret_ansi_split = "secret_ansi_token_part";

    let raw_error_summary = format!(
        "Failed with Authorization: Bearer {secret_bearer} and API_KEY=\"{secret_api_key_space}\" and Authorization: Basic {secret_basic} and Bearer {secret_bearer_short} and \"api_key\": \"{secret_json}\" and API_TO\x1b[31mKEN={secret_ansi_split}"
    );
    let raw_diag_tail = format!(
        "URL was https://example.com/api?token={secret_url_token}&other=1 with ANSI API_TO\x1b[31mKEN={secret_ansi_split}\x1b[0m"
    );

    let val = json!({
        "schema_version": 1,
        "run_id": run_id,
        "origin": "cli_headless",
        "status": "failed",
        "created_at_ms": 1_000_000,
        "completed_at_ms": 1_001_000,
        "duration_ms": 1_000,
        "requested_runner": "codex",
        "resolved_runner": "codex",
        "requested_model": null,
        "resolved_model": null,
        "dmuxai_version": "0.3.0",
        "build_commit": null,
        "child_exit_code": 1,
        "child_signal": null,
        "error_kind": "auth",
        "error_summary": raw_error_summary,
        "links": {
            "conversation_id": null,
            "turn_id": null,
            "graph_id": null,
            "node_id": null,
            "attempt_index": null
        },
        "diagnostics": {
            "stderr_bytes": raw_diag_tail.len() as u64,
            "tail_sample": raw_diag_tail
        },
        "end_summary": null
    });

    let manifest: RunManifest = serde_json::from_value(val).expect("manifest fixture");
    store.save(&manifest).expect("save manifest with secrets");
    store
        .write_diagnostics(run_id, &raw_diag_tail)
        .expect("write diagnostics with secrets");

    let manifest_raw = fs::read_to_string(store_root.join(run_id).join("manifest.json"))
        .expect("read raw manifest");
    let diag_raw = fs::read_to_string(store_root.join(run_id).join("diagnostics.txt"))
        .expect("read raw diagnostics");

    // Manifest raw content must NOT contain any of the synthetic secrets
    assert!(
        !manifest_raw.contains(secret_bearer),
        "manifest contains raw bearer secret"
    );
    assert!(
        !manifest_raw.contains(secret_api_key_space),
        "manifest contains raw API_KEY with space"
    );
    assert!(
        !manifest_raw.contains(secret_basic),
        "manifest contains raw basic auth"
    );
    assert!(
        !manifest_raw.contains(secret_url_token),
        "manifest contains raw query token"
    );
    assert!(
        !manifest_raw.contains(secret_bearer_short),
        "manifest contains raw short bearer"
    );
    assert!(
        !manifest_raw.contains(secret_json),
        "manifest contains raw json key secret"
    );
    assert!(
        !manifest_raw.contains(secret_ansi_split),
        "manifest contains raw ANSI-split secret"
    );

    // Diagnostics raw content must NOT contain the secrets
    assert!(
        !diag_raw.contains(secret_url_token),
        "diagnostics contains raw url token"
    );
    assert!(
        !diag_raw.contains(secret_ansi_split),
        "diagnostics contains raw ANSI-split token"
    );
}

/// TC-2563: Symlink targets at root, run directory, and leaf files.
/// Must return error without reading or modifying outside files; prune must not delete outside files or follow symlinks.
#[test]
fn tc_2563_store_symlink_defense_and_outside_file_protection() {
    #[cfg(unix)]
    {
        let tmp = TempDir::new().expect("tempdir");
        let store_root = tmp.path().join("runs");
        fs::create_dir_all(&store_root).unwrap();

        let outside_dir = tmp.path().join("outside");
        fs::create_dir_all(&outside_dir).unwrap();
        let outside_marker = outside_dir.join("marker.txt");
        fs::write(&outside_marker, "LEAVE ME ALONE").unwrap();

        let store = JournalStore::new(store_root.clone());

        // 1. Run directory is a symlink pointing outside
        let symlink_run_id = "00000000000000a1";
        let symlink_path = store_root.join(symlink_run_id);
        std::os::unix::fs::symlink(&outside_dir, &symlink_path).expect("symlink");

        let res_load = store.load(symlink_run_id);
        assert!(res_load.is_err(), "loading run dir symlink must fail");

        let manifest = sample_manifest(symlink_run_id, "completed", 1_000_000, Some(1_001_000));
        let res_save = store.save(&manifest);
        assert!(res_save.is_err(), "saving to run dir symlink must fail");
        assert_eq!(
            fs::read_to_string(&outside_marker).unwrap(),
            "LEAVE ME ALONE"
        );

        // 2. Leaf file is a symlink
        let real_run_id = "00000000000000a2";
        let real_run_dir = store_root.join(real_run_id);
        fs::create_dir_all(&real_run_dir).unwrap();
        let manifest_symlink = real_run_dir.join("manifest.json");
        std::os::unix::fs::symlink(&outside_marker, &manifest_symlink).expect("symlink leaf");

        let res_load_leaf = store.load(real_run_id);
        assert!(
            res_load_leaf.is_err(),
            "loading manifest leaf symlink must fail"
        );

        let res_save_leaf = store.save(&manifest);
        assert!(
            res_save_leaf.is_err(),
            "saving over manifest leaf symlink must fail"
        );
        assert_eq!(
            fs::read_to_string(&outside_marker).unwrap(),
            "LEAVE ME ALONE"
        );

        // 3. Pruning must never delete outside target or follow symlink
        let config = JournalConfig {
            max_age_ms: 10,
            max_total_bytes: 1,
        };
        let _prune_res = store.prune(&config, 2_000_000).expect("prune execution");
        assert_eq!(
            fs::read_to_string(&outside_marker).unwrap(),
            "LEAVE ME ALONE"
        );
        assert!(outside_marker.exists());
    }
}

/// TC-2564: Bounded manifest read (>128 KiB rejected), bounded diag write (<=64 KiB) after repeated calls multibyte text,
/// and whole-tail snapshot semantics.
#[test]
fn tc_2564_store_bounded_manifest_read_and_bounded_diagnostics_write() {
    let tmp = TempDir::new().expect("tempdir");
    let store_root = tmp.path().join("runs");
    let store = JournalStore::new(store_root.clone());

    let run_id = "0123456789abcdef";
    let manifest = sample_manifest(run_id, "completed", 1_000_000, Some(1_001_000));
    store.save(&manifest).expect("save initial");

    let manifest_path = store_root.join(run_id).join("manifest.json");

    // 1. Manifest file > 128 KiB must be rejected on load
    let large_pad = " ".repeat(130 * 1024);
    let large_json = format!("{{\"pad\":\"{large_pad}\"}}");
    fs::write(&manifest_path, large_json).expect("write large manifest");
    let load_err = store.load(run_id);
    assert!(load_err.is_err(), "manifest > 128 KiB must be rejected");

    // Restore valid manifest
    store.save(&manifest).expect("save restored");

    // 2. Diagnostics write bounded <= 64 KiB with multibyte text
    // E.g. 100 KiB of multibyte emojis and text "🦀 DmuxAI Run Diagnostics \n"
    let chunk = "🦀 DmuxAI Run Diagnostics line \n";
    let repeated = chunk.repeat(4000); // ~120 KiB
    assert!(repeated.len() > 64 * 1024);

    // Call write_diagnostics multiple times (whole-tail snapshot semantics)
    store
        .write_diagnostics(run_id, &repeated)
        .expect("write diagnostics 1");
    store
        .write_diagnostics(run_id, &repeated)
        .expect("write diagnostics 2");

    let diag_path = store_root.join(run_id).join("diagnostics.txt");
    let diag_meta = fs::metadata(&diag_path).expect("diag metadata");
    assert!(
        diag_meta.len() <= 64 * 1024,
        "diagnostics file size {} exceeds 64 KiB limit",
        diag_meta.len()
    );

    // Bounded read check: loaded diagnostics is valid UTF-8 and bounded <= 64 KiB
    let loaded_diag = store.load_diagnostics(run_id).expect("load diagnostics");
    assert!(loaded_diag.len() <= 64 * 1024);
    assert!(!loaded_diag.is_empty());
}

/// TC-2565: Actual retention pruning by age and total size.
/// Oldest terminal runs removed; active runs, corrupt runs, and schema_version 99 runs preserved.
/// Symlinks skipped and deleted count reflects actual deleted runs. Deterministic now_ms, no sleep.
#[test]
fn tc_2565_store_retention_prune_preserves_active_corrupt_and_new_schemas() {
    let tmp = TempDir::new().expect("tempdir");
    let store_root = tmp.path().join("runs");
    let store = JournalStore::new(store_root.clone());

    // 1. Old completed run: created at 1000, completed at 2000
    let id_old = "0000000000000001";
    let m_old = sample_manifest(id_old, "completed", 1_000, Some(2_000));
    store.save(&m_old).unwrap();
    store.write_diagnostics(id_old, "old diagnostics").unwrap();

    // 2. Active run: created at 1000, no completed_at
    let id_active = "0000000000000002";
    let m_active = sample_manifest(id_active, "running", 1_000, None);
    store.save(&m_active).unwrap();

    // 3. Corrupt manifest directory: invalid JSON
    let id_corrupt = "0000000000000003";
    let corrupt_dir = store_root.join(id_corrupt);
    fs::create_dir_all(&corrupt_dir).unwrap();
    fs::write(corrupt_dir.join("manifest.json"), "{ invalid json }}}").unwrap();

    // 4. Schema version 99 run: must be preserved (future version protection)
    let id_schema99 = "0000000000000004";
    let mut val_s99 = serde_json::to_value(sample_manifest(
        id_schema99,
        "completed",
        1_000,
        Some(2_000),
    ))
    .unwrap();
    val_s99["schema_version"] = json!(99);
    let s99_dir = store_root.join(id_schema99);
    fs::create_dir_all(&s99_dir).unwrap();
    fs::write(
        s99_dir.join("manifest.json"),
        serde_json::to_string(&val_s99).unwrap(),
    )
    .unwrap();

    // 5. Recent completed run: created at 50_000, completed at 51_000
    let id_recent = "0000000000000005";
    let m_recent = sample_manifest(id_recent, "completed", 50_000, Some(51_000));
    store.save(&m_recent).unwrap();

    // Deterministic prune test with age cutoff:
    // now_ms = 60_000. max_age_ms = 20_000.
    // Cutoff time = 40_000.
    // id_old (completed at 2000 < 40_000) is expired and must be pruned.
    // id_active, id_corrupt, id_schema99, id_recent must remain.
    let config_age = JournalConfig {
        max_age_ms: 20_000,
        max_total_bytes: 10 * 1024 * 1024, // Plenty of bytes
    };
    let report = store.prune(&config_age, 60_000).expect("prune by age");
    assert_eq!(
        report.deleted_runs, 1,
        "exactly 1 expired terminal run deleted"
    );
    assert!(
        !store_root.join(id_old).exists(),
        "old run directory must be deleted"
    );
    assert!(
        store_root.join(id_active).exists(),
        "active run must be preserved"
    );
    assert!(
        store_root.join(id_corrupt).exists(),
        "corrupt run must be preserved"
    );
    assert!(
        store_root.join(id_schema99).exists(),
        "future schema run must be preserved"
    );
    assert!(
        store_root.join(id_recent).exists(),
        "recent run must be preserved"
    );

    // Size limit prune test:
    // Tiny size limit (1 byte) -> forces oldest terminal runs to be removed until within limit.
    let config_size = JournalConfig {
        max_age_ms: 1_000_000, // No age expiry
        max_total_bytes: 1,    // Force pruning terminal runs
    };
    let report2 = store.prune(&config_size, 60_000).expect("prune by size");
    assert_eq!(
        report2.deleted_runs, 1,
        "recent terminal run pruned to satisfy size limit"
    );
    assert!(
        !store_root.join(id_recent).exists(),
        "recent terminal run pruned"
    );
    assert!(
        store_root.join(id_active).exists(),
        "active run preserved despite size limit"
    );
    assert!(
        store_root.join(id_corrupt).exists(),
        "corrupt run preserved despite size limit"
    );
    assert!(
        store_root.join(id_schema99).exists(),
        "schema 99 run preserved despite size limit"
    );
}

/// TC-2567: Regression tests for storage bugs 1-6.
#[test]
fn tc_2567_storage_hardening_regressions() {
    let tmp = TempDir::new().expect("tempdir");
    let store_root = tmp.path().join("runs");
    let store = JournalStore::new(store_root.clone());

    // 1. Oversized (>128KiB) and ID-mismatch records preserved by actual prune
    let id_mismatch = "0000000000000010";
    let mismatch_dir = store_root.join(id_mismatch);
    fs::create_dir_all(&mismatch_dir).unwrap();
    let mut m_mismatch_val =
        serde_json::to_value(sample_manifest(id_mismatch, "completed", 1000, Some(2000))).unwrap();
    m_mismatch_val["run_id"] = json!("0000000000000099"); // ID mismatch
    fs::write(
        mismatch_dir.join("manifest.json"),
        serde_json::to_vec(&m_mismatch_val).unwrap(),
    )
    .unwrap();

    let id_oversized = "0000000000000020";
    let oversized_dir = store_root.join(id_oversized);
    fs::create_dir_all(&oversized_dir).unwrap();
    let pad = "x".repeat(130 * 1024);
    let mut m_oversized_val =
        serde_json::to_value(sample_manifest(id_oversized, "completed", 1000, Some(2000))).unwrap();
    m_oversized_val["error_summary"] = json!(pad);
    fs::write(
        oversized_dir.join("manifest.json"),
        serde_json::to_vec(&m_oversized_val).unwrap(),
    )
    .unwrap();

    // Unknown status should fail load as corrupt rather than accepted and be preserved
    let id_unknown_status = "0000000000000030";
    let unknown_st_dir = store_root.join(id_unknown_status);
    fs::create_dir_all(&unknown_st_dir).unwrap();
    let mut m_unknown_st = serde_json::to_value(sample_manifest(
        id_unknown_status,
        "completed",
        1000,
        Some(2000),
    ))
    .unwrap();
    m_unknown_st["status"] = json!("some_bizarre_status");
    fs::write(
        unknown_st_dir.join("manifest.json"),
        serde_json::to_vec(&m_unknown_st).unwrap(),
    )
    .unwrap();

    // Malformed but superficially {schema_version:1,status:completed} MUST NOT be pruned!
    let id_malformed = "0000000000000040";
    let malformed_dir = store_root.join(id_malformed);
    fs::create_dir_all(&malformed_dir).unwrap();
    fs::write(
        malformed_dir.join("manifest.json"),
        r#"{"schema_version":1,"status":"completed"}"#,
    )
    .unwrap();

    // Prune with aggressive age cutoff
    let prune_cfg = JournalConfig {
        max_age_ms: 10,
        max_total_bytes: 1,
    };
    let report = store.prune(&prune_cfg, 5_000_000).expect("prune");
    assert_eq!(
        report.deleted_runs, 0,
        "no invalid/corrupt/oversized/mismatched runs may be pruned"
    );
    assert!(mismatch_dir.exists(), "mismatch dir must be preserved");
    assert!(oversized_dir.exists(), "oversized dir must be preserved");
    assert!(
        unknown_st_dir.exists(),
        "unknown status dir must be preserved"
    );
    assert!(
        malformed_dir.exists(),
        "malformed superficial dir must be preserved"
    );

    // Load on unknown status should fail as corrupt
    assert!(
        store.load(id_unknown_status).is_err(),
        "unknown status must fail load"
    );

    // 2. Secret key masking in end_summary (nested API_TOKEN / password)
    let id_mask = "0000000000000050";
    let mut m_mask = sample_manifest(id_mask, "completed", 1000, Some(2000));
    m_mask.end_summary = Some(json!({
        "API_TOKEN": "syntheticsecret123",
        "nested": {
            "password": "my_hidden_password",
            "safe_field": "hello world"
        }
    }));
    store.save(&m_mask).expect("save masked manifest");
    let raw_saved = fs::read_to_string(store_root.join(id_mask).join("manifest.json")).unwrap();
    assert!(
        !raw_saved.contains("syntheticsecret123"),
        "API_TOKEN value leaked into disk!"
    );
    assert!(
        !raw_saved.contains("my_hidden_password"),
        "password value leaked into disk!"
    );
    assert!(raw_saved.contains("[REDACTED]"), "values must be redacted");
    assert!(
        raw_saved.contains("hello world"),
        "non-secret fields preserved"
    );

    // 3. Save rejects payload > 128 KiB before write, no partial overwrite
    let id_huge = "0000000000000060";
    let mut m_huge = sample_manifest(id_huge, "completed", 1000, Some(2000));
    store.save(&m_huge).expect("save initial small");
    let orig_raw = fs::read_to_string(store_root.join(id_huge).join("manifest.json")).unwrap();

    m_huge.error_summary = Some("A".repeat(130 * 1024));
    let save_huge_res = store.save(&m_huge);
    assert!(save_huge_res.is_err(), "saving > 128KiB manifest must fail");
    let after_failed_save =
        fs::read_to_string(store_root.join(id_huge).join("manifest.json")).unwrap();
    assert_eq!(
        orig_raw, after_failed_save,
        "manifest must not be partially overwritten or modified"
    );

    // Clean up test runs from previous parts of the test before testing symlink child pruning
    let _ = fs::remove_dir_all(store_root.join(id_mask));
    let _ = fs::remove_dir_all(store_root.join(id_huge));

    // 4 & 6. Symlink inside run dir -> safe_delete_run_dir must not touch external file and whole run preserved
    #[cfg(unix)]
    {
        let outside_marker = tmp.path().join("external_marker.txt");
        fs::write(&outside_marker, "ORIGINAL OUTSIDE CONTENT").unwrap();

        let id_sym_child = "0000000000000070";
        let m_sym = sample_manifest(id_sym_child, "completed", 5_000_000, Some(5_000_000));
        store.save(&m_sym).unwrap();
        // Add diagnostics.txt as symlink to outside marker
        let diag_symlink = store_root.join(id_sym_child).join("diagnostics.txt");
        std::os::unix::fs::symlink(&outside_marker, &diag_symlink).unwrap();

        // Pruning should skip/fail to delete this run rather than touching external marker or partially deleting
        // With now_ms = 6_000_000 and max_age_ms = 10, m_sym would be eligible for age deletion,
        // but because of the symlink child, safe_delete_run_dir must fail, and deleted_runs must be 0
        let report_sym = store
            .prune(&prune_cfg, 6_000_000)
            .expect("prune with symlink child");
        assert_eq!(
            report_sym.deleted_runs, 0,
            "prune must not count error/skipped run as deleted"
        );
        assert!(
            store_root.join(id_sym_child).exists(),
            "run with symlink child must be preserved"
        );
        assert_eq!(
            fs::read_to_string(&outside_marker).unwrap(),
            "ORIGINAL OUTSIDE CONTENT"
        );
        // Clean up symlink child run before next steps
        let _ = fs::remove_file(&diag_symlink);
        let _ = fs::remove_dir_all(store_root.join(id_sym_child));
    }

    // 5. save_with_retention propagates prune errors / report and prunes old while active survives
    let id_old_run = "0000000000000080";
    let m_old_run = sample_manifest(id_old_run, "completed", 1000, Some(2000));
    store.save(&m_old_run).unwrap();

    let id_active_run = "0000000000000081";
    let m_active_run = sample_manifest(id_active_run, "running", 1000, None);
    store.save(&m_active_run).unwrap();

    let id_new_terminal = "0000000000000082";
    let m_new_terminal = sample_manifest(id_new_terminal, "completed", 5000, Some(6000));
    let retention_cfg = JournalConfig {
        max_age_ms: 1000,
        max_total_bytes: 10 * 1024 * 1024,
    };
    let prune_rep = store
        .save_with_retention(&m_new_terminal, Some(&retention_cfg), 6000)
        .expect("save_with_retention");
    assert!(prune_rep.is_some(), "prune report must be returned");
    assert_eq!(prune_rep.unwrap().deleted_runs, 1, "old run pruned");
    assert!(
        !store_root.join(id_old_run).exists(),
        "old run must be pruned"
    );
    assert!(
        store_root.join(id_active_run).exists(),
        "active run must survive"
    );
    assert!(
        store_root.join(id_new_terminal).exists(),
        "new run must survive"
    );
}
