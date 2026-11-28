//! Pending graph plan attach/edit (FR-3106, FR-3107).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{BoardError, BoardPlan, PLAN_HISTORY_MAX, mask_board_plan};
use crate::docs::sha256_hex;
use crate::graph::{
    GraphPlan, PlanContext, PlanDoc, PlanStep, PlanTask, expand_plan_steps, is_spec_first_step,
    parse_plan, validate_plan_with_limit,
};
use crate::settings::DmuxSettings;
use crate::state_path::valid_pending_plan_hash;

/// `graph-pending/<hash>.json`, shared with the headless planner
/// (`--approve-plan-id`, FR-2545) so both sides hash the same bytes (FR-3813).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingPlanFile {
    pub prompt: String,
    pub tasks: Vec<PlanTask>,
    pub steps: Vec<PlanStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic_cap: Option<usize>,
}

impl PendingPlanFile {
    /// Canonical JSON and its plan hash (first 16 hex chars of its sha256).
    pub fn json_and_hash(&self) -> Result<(String, String), serde_json::Error> {
        let json = serde_json::to_string(self)?;
        let hash = sha256_hex(json.as_bytes()).chars().take(16).collect();
        Ok((json, hash))
    }
}

pub fn pending_plan_path(state: &Path, hash: &str) -> PathBuf {
    state.join("graph-pending").join(format!("{hash}.json"))
}

pub fn board_plan_copy_path(board_base: &Path, hash: &str) -> PathBuf {
    board_base.join("plans").join(format!("{hash}.json"))
}

fn read_checked_bytes(path: &Path, not_found_msg: String) -> Result<Vec<u8>, BoardError> {
    crate::run_journal::verify_no_symlinks(path).map_err(|e| BoardError::Usage(e.to_string()))?;
    if crate::run_journal::is_symlink(path).unwrap_or(false) {
        return Err(BoardError::Usage(format!(
            "refusing symlinked plan file at {}",
            path.display()
        )));
    }
    std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            BoardError::NotFound(not_found_msg)
        } else {
            BoardError::Internal(e.to_string())
        }
    })
}

/// Write `<state base>/board/plans/<hash>.json` (0600, canonical hash-verified
/// bytes) so failed runs can retry with the same hash (FR-3130).
pub fn write_board_plan_copy(
    state_root: &Path,
    board_base: &Path,
    hash: &str,
) -> Result<(), BoardError> {
    if !valid_pending_plan_hash(hash) {
        return Err(BoardError::Usage(
            "plan hash must be 16 lowercase hex characters".to_owned(),
        ));
    }
    let pending_path = pending_plan_path(state_root, hash);
    let bytes = read_checked_bytes(&pending_path, format!("No pending plan found for '{hash}'"))?;
    let pending: PendingPlanFile = serde_json::from_slice(&bytes)
        .map_err(|e| BoardError::Failed(format!("Pending plan '{hash}' is invalid: {e}")))?;
    if pending_hash_for_plan(&pending)? != hash {
        return Err(BoardError::Failed("pending plan hash mismatch".to_owned()));
    }
    let raw_prefix: String = sha256_hex(&bytes).chars().take(16).collect();
    let canonical_bytes;
    let out_bytes = if raw_prefix == hash {
        bytes.as_slice()
    } else {
        canonical_bytes =
            serde_json::to_vec(&pending).map_err(|e| BoardError::Internal(e.to_string()))?;
        canonical_bytes.as_slice()
    };
    let plans_dir = board_base.join("plans");
    crate::board::store::ensure_dir(&plans_dir)?;
    let copy_path = board_plan_copy_path(board_base, hash);
    crate::board::store::atomic_write(&copy_path, out_bytes)
}

fn read_board_plan_copy_bytes(
    board_base: &Path,
    hash: &str,
) -> Result<(PendingPlanFile, Vec<u8>), BoardError> {
    let copy_path = board_plan_copy_path(board_base, hash);
    let bytes = read_checked_bytes(
        &copy_path,
        format!("No pending plan or board copy found for '{hash}'"),
    )?;
    let actual_prefix: String = sha256_hex(&bytes).chars().take(16).collect();
    if actual_prefix != hash {
        return Err(BoardError::Failed(format!(
            "Board plan copy '{hash}' failed sha256 verification"
        )));
    }
    let plan: PendingPlanFile = serde_json::from_slice(&bytes)
        .map_err(|e| BoardError::Failed(format!("Board plan copy '{hash}' is invalid: {e}")))?;
    Ok((plan, bytes))
}

/// Ensure `graph-pending/<hash>.json` exists; if missing, restore it from
/// `<board_base>/plans/<hash>.json` after verifying its sha256 prefix (FR-3130).
pub fn ensure_pending_plan_from_copy(
    state_root: &Path,
    board_base: &Path,
    hash: &str,
) -> Result<PendingPlanFile, BoardError> {
    match read_pending_plan(state_root, hash) {
        Ok(plan) => return Ok(plan),
        Err(BoardError::NotFound(_)) => {}
        Err(other) => return Err(other),
    }
    let (plan, bytes) = read_board_plan_copy_bytes(board_base, hash)?;
    let dir = state_root.join("graph-pending");
    crate::board::store::ensure_dir(&dir)?;
    crate::board::store::atomic_write(&pending_plan_path(state_root, hash), &bytes)?;
    Ok(plan)
}

/// Read `graph-pending/<hash>.json` or fall back to `<board_base>/plans/<hash>.json`
/// without mutating `graph-pending` (FR-3130).
pub fn read_pending_or_board_copy(
    state_root: &Path,
    board_base: &Path,
    hash: &str,
) -> Result<PendingPlanFile, BoardError> {
    match read_pending_plan(state_root, hash) {
        Ok(plan) => Ok(plan),
        Err(BoardError::NotFound(_)) => {
            read_board_plan_copy_bytes(board_base, hash).map(|(p, _)| p)
        }
        Err(other) => Err(other),
    }
}

/// Prune `<board_base>/plans/<hash>.json` copies that belong only to
/// `done`/`rejected` items older than 30 days (FR-3130).
pub fn prune_old_plan_copies(board_base: &Path, items: &[super::BoardItem], now_ms: u64) {
    let cutoff_ms = now_ms.saturating_sub(super::CLOSED_HIDE_DAYS * 86_400 * 1000);
    let mut keep_hashes = std::collections::HashSet::new();
    for item in items {
        let is_old_closed = matches!(item.lane, super::Lane::Done | super::Lane::Rejected)
            && item.updated_ms < cutoff_ms;
        if !is_old_closed && let Some(plan) = &item.plan {
            keep_hashes.insert(plan.hash.as_str());
            for h in &plan.history {
                keep_hashes.insert(h.as_str());
            }
        }
    }
    let plans_dir = board_base.join("plans");
    let Ok(entries) = std::fs::read_dir(&plans_dir) else {
        return;
    };
    let max_age = std::time::Duration::from_secs(super::CLOSED_HIDE_DAYS * 86_400);
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if keep_hashes.contains(stem) {
            continue;
        }
        let is_old_file = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if is_old_file {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn read_pending_plan(state: &Path, hash: &str) -> Result<PendingPlanFile, BoardError> {
    if !valid_pending_plan_hash(hash) {
        return Err(BoardError::Usage(
            "plan hash must be 16 lowercase hex characters".to_owned(),
        ));
    }
    let path = pending_plan_path(state, hash);
    let bytes = read_checked_bytes(&path, format!("No pending plan found for '{hash}'"))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| BoardError::Failed(format!("Pending plan '{hash}' is invalid: {e}")))
}

pub fn pending_hash_for_plan(plan: &PendingPlanFile) -> Result<String, BoardError> {
    plan.json_and_hash()
        .map(|(_, hash)| hash)
        .map_err(|e| BoardError::Internal(e.to_string()))
}

pub fn write_pending_plan(state: &Path, plan: &PendingPlanFile) -> Result<String, BoardError> {
    let (json, hash) = plan
        .json_and_hash()
        .map_err(|e| BoardError::Internal(e.to_string()))?;
    let dir = state.join("graph-pending");
    crate::board::store::ensure_dir(&dir)?;
    let path = dir.join(format!("{hash}.json"));
    crate::board::store::atomic_write(&path, json.as_bytes())?;
    Ok(hash)
}

/// The unmasked pending plan behind an item's current plan, if it has one.
pub fn unmasked_pending_from_item_plan(
    state_root: &Path,
    board_base: &Path,
    plan: Option<&super::BoardPlan>,
) -> Result<Option<PendingPlanFile>, BoardError> {
    let Some(plan) = plan else {
        return Ok(None);
    };
    read_pending_or_board_copy(state_root, board_base, &plan.hash).map(Some)
}

pub fn unmasked_prompt_from_item_plan(
    state_root: &Path,
    board_base: &Path,
    plan: Option<&super::BoardPlan>,
) -> Result<Option<String>, BoardError> {
    Ok(unmasked_pending_from_item_plan(state_root, board_base, plan)?.map(|p| p.prompt))
}

/// Runner list and model hints for board plan edits. This is a pre-check only;
/// `run --graph --approve-plan-id` re-validates authoritatively at execution.
pub struct PlanValidationCtx {
    runners: Vec<String>,
    settings: DmuxSettings,
}

impl PlanValidationCtx {
    pub fn from_settings(settings: &DmuxSettings) -> Self {
        let mut runners = Vec::new();
        for name in ["cursor", "agy", "codex"] {
            if settings.graph_runner_allowed(name) {
                runners.push(name.to_owned());
            }
        }
        for spec in &settings.custom_agents {
            if settings.graph_runner_allowed(&spec.name) {
                runners.push(spec.name.clone());
            }
        }
        Self {
            runners,
            settings: settings.clone(),
        }
    }

    fn effective_model(&self, runner: &str) -> Option<String> {
        let key = runner.trim().to_ascii_lowercase();
        if let Some(model) = self.settings.runner_models.get(&key) {
            return Some(model.clone());
        }
        if let Some(spec) = self
            .settings
            .custom_agents
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(&key))
        {
            return spec.default_model.clone().or_else(|| {
                crate::model_policy::default_model_for_runner(&key).map(str::to_owned)
            });
        }
        crate::model_policy::default_model_for_runner(&key).map(str::to_owned)
    }
}

pub fn edit_plan_from_json(
    workspace_root: &Path,
    state_root: &Path,
    stored_prompt: &str,
    json_text: &str,
    inherit_tasks: Option<Vec<PlanTask>>,
) -> Result<(PendingPlanFile, String), BoardError> {
    edit_plan_from_json_keep_cap(
        workspace_root,
        state_root,
        stored_prompt,
        json_text,
        inherit_tasks,
        None,
    )
}

/// Like [`edit_plan_from_json`], carrying the approved `dynamic_cap` of the
/// plan being edited into the new pending plan (FR-3813).
pub fn edit_plan_from_json_keep_cap(
    workspace_root: &Path,
    state_root: &Path,
    stored_prompt: &str,
    json_text: &str,
    inherit_tasks: Option<Vec<PlanTask>>,
    dynamic_cap: Option<usize>,
) -> Result<(PendingPlanFile, String), BoardError> {
    let mut doc = parse_plan(json_text).map_err(|errors| {
        BoardError::Failed(
            errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    })?;
    if doc.tasks.is_empty()
        && let Some(tasks) = inherit_tasks
    {
        doc.tasks = tasks;
        doc.task_errors.clear();
        doc.require_tasks = !doc.tasks.is_empty();
    }
    let settings = DmuxSettings::load(workspace_root);
    let normalized_prompt = super::normalize_plan_prompt(stored_prompt);
    let graph =
        validate_doc_for(&settings, &normalized_prompt, &doc).map_err(BoardError::Failed)?;
    if let Some(expanded) = expand_doc(&settings, &normalized_prompt, &doc, &graph) {
        doc = expanded;
    }
    let mut plan = pending_from_doc(&normalized_prompt, &doc);
    plan.dynamic_cap = dynamic_cap;
    let hash = write_pending_plan(state_root, &plan)?;
    Ok((plan, hash))
}

fn validate_doc_for(
    settings: &DmuxSettings,
    prompt: &str,
    doc: &PlanDoc,
) -> Result<GraphPlan, String> {
    let holder = PlanValidationCtx::from_settings(settings);
    let cap = settings.graph_max_nodes.clamp(1, 8) as usize;
    let ctx = PlanContext {
        user_prompt: prompt,
        available_runners: &holder.runners,
        runner_allowed: &|runner: &str| settings.graph_runner_allowed(runner),
        effective_model: &|runner: &str| holder.effective_model(runner),
        validate_model: &crate::model_policy::validate_model_for_runner,
    };
    validate_plan_with_limit(doc, &ctx, cap).map_err(|errors| {
        errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// The spec-first expansion headless runs apply (FR-3503), so the stored
/// plan is the plan `--approve-plan-id` executes (FR-3715). `None` when
/// nothing is inserted or the expanded plan would not validate (for example
/// over the step cap); the run then expands the stored plan as before.
fn expand_doc(
    settings: &DmuxSettings,
    prompt: &str,
    doc: &PlanDoc,
    graph: &GraphPlan,
) -> Option<PlanDoc> {
    let (steps, inserted) = expand_plan_steps(&doc.steps, graph, &settings.workflow);
    if inserted.is_empty() {
        return None;
    }
    let expanded = PlanDoc {
        steps,
        ..doc.clone()
    };
    validate_doc_for(settings, prompt, &expanded)
        .ok()
        .map(|_| expanded)
}

/// Spec-first expansion of an attached pending plan for `workspace_root`
/// (FR-3715). Writes and returns the expanded plan and its hash, or `None`
/// when the plan is stored as given.
pub fn expand_pending_plan(
    workspace_root: &Path,
    state_root: &Path,
    pending: &PendingPlanFile,
) -> Result<Option<(PendingPlanFile, String)>, BoardError> {
    if !workspace_root.is_dir() {
        return Ok(None);
    }
    let settings = DmuxSettings::load(workspace_root);
    let doc = PlanDoc {
        steps: pending.steps.clone(),
        tasks: pending.tasks.clone(),
        require_tasks: !pending.tasks.is_empty(),
        task_errors: Vec::new(),
    };
    let Ok(graph) = validate_doc_for(&settings, &pending.prompt, &doc) else {
        return Ok(None);
    };
    let Some(expanded) = expand_doc(&settings, &pending.prompt, &doc, &graph) else {
        return Ok(None);
    };
    let plan = PendingPlanFile {
        prompt: pending.prompt.clone(),
        tasks: expanded.tasks,
        steps: expanded.steps,
        dynamic_cap: pending.dynamic_cap,
    };
    let hash = write_pending_plan(state_root, &plan)?;
    Ok(Some((plan, hash)))
}

pub fn pending_from_doc(prompt: &str, doc: &PlanDoc) -> PendingPlanFile {
    PendingPlanFile {
        prompt: super::normalize_plan_prompt(prompt),
        tasks: doc.tasks.clone(),
        steps: doc.steps.clone(),
        dynamic_cap: None,
    }
}

pub fn board_plan_from_pending(
    pending: PendingPlanFile,
    hash: String,
    previous: Option<&BoardPlan>,
) -> BoardPlan {
    let revision = previous.map(|p| p.revision + 1).unwrap_or(1);
    let mut history: Vec<String> = previous
        .map(|p| {
            let mut h = p.history.clone();
            h.push(p.hash.clone());
            h
        })
        .unwrap_or_default();
    if history.len() > PLAN_HISTORY_MAX {
        let drop = history.len() - PLAN_HISTORY_MAX;
        history.drain(..drop);
    }
    let previous_steps = previous.map(|p| p.steps.clone());
    let spec_first_added = pending
        .steps
        .iter()
        .filter(|step| is_spec_first_step(&step.role, &step.job, &pending.prompt))
        .map(|step| step.id.clone())
        .collect();
    let mut plan = BoardPlan {
        hash,
        prompt: pending.prompt,
        tasks: pending.tasks,
        steps: pending.steps,
        revision,
        history,
        previous_steps,
        spec_first_added,
    };
    mask_board_plan(&mut plan);
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Kind;
    use crate::board::store::{AddItemRequest, BoardStore, ensure_dir};
    use tempfile::tempdir;

    fn sample_plan_json() -> String {
        r#"{"tasks":[{"id":"task-1","title":"do it","done_check":"done","size":"small","depends_on":[]}],"steps":[{"id":"step-1","role":"implement","job":"edit","runner":"cursor","depends_on":[],"task":"task-1"}]}"#.to_owned()
    }

    #[test]
    fn tc_3106_attach_pending_plan_by_hash_missing_is_not_found() {
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        ensure_dir(&state).unwrap();
        let err = read_pending_plan(&state, "deadbeefdeadbeef").unwrap_err();
        assert!(matches!(err, BoardError::NotFound(_)));
        assert!(read_pending_plan(&state, "../bad").is_err());
    }

    #[test]
    fn tc_3107_plan_edit_validates_writes_new_pending_and_bumps_revision() {
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        ensure_dir(&state.join("board")).unwrap();
        let store = BoardStore::open(state.join("board")).unwrap();
        let item = store
            .add(AddItemRequest {
                title: "feat".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: None,
                source_bot: Some("bot".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(workspace.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        // Edit from a file path, as `board plan --edit <file>` does.
        let plan_path = dir.path().join("plan.json");
        std::fs::write(&plan_path, sample_plan_json()).unwrap();
        let from_file = crate::board::read_path_or_stdin(plan_path.to_str().unwrap()).unwrap();
        let (_, hash) =
            edit_plan_from_json(&workspace, &state, "ship it", &from_file, None).unwrap();
        let updated = store
            .attach_plan(&item.id, &hash, &state, "bot:test")
            .unwrap();
        assert_eq!(updated.plan.as_ref().unwrap().revision, 1);
        assert_eq!(updated.plan.as_ref().unwrap().tasks.len(), 1);

        // Omitted tasks are inherited from the current plan.
        let steps_only = r#"{"steps":[{"id":"step-1","role":"implement","job":"edit-more","runner":"cursor","depends_on":[],"task":"task-1"}]}"#;
        let (_, hash2) = edit_plan_from_json(
            &workspace,
            &state,
            "ship it",
            steps_only,
            updated.plan.as_ref().map(|p| p.tasks.clone()),
        )
        .unwrap();
        let edited = store
            .edit_plan(&item.id, &hash2, &workspace, &state, "bot:test")
            .unwrap();
        assert_eq!(edited.plan.as_ref().unwrap().revision, 2);
        assert_eq!(edited.plan.as_ref().unwrap().tasks.len(), 1);
        assert_eq!(edited.plan.as_ref().unwrap().tasks[0].id, "task-1");
        let edited_step = edited
            .plan
            .as_ref()
            .unwrap()
            .steps
            .iter()
            .find(|s| s.id == "step-1");
        assert_eq!(edited_step.unwrap().job, "edit-more");
        assert_ne!(hash, hash2);

        // An approved item goes back to needs_decision and loses its decision.
        let approved = store
            .decide(
                &item.id,
                crate::board::DecideRequest {
                    verdict: crate::board::Verdict::Approve,
                    text: None,
                    option: None,
                    plan_hash: Some(hash2.clone()),
                    by: "owner".to_owned(),
                },
            )
            .unwrap();
        assert_eq!(approved.lane, crate::board::Lane::Approved);
        assert!(approved.decision.is_some());

        // A validation error leaves the stored item unchanged.
        let invalid = sample_plan_json().replace("\"cursor\"", "\"not-a-runner\"");
        let error = edit_plan_from_json(&workspace, &state, "ship it", &invalid, None).unwrap_err();
        assert!(matches!(error, BoardError::Failed(_)));
        let unchanged = store.get(&item.id).unwrap();
        assert_eq!(unchanged.version, approved.version);
        assert_eq!(unchanged.plan.as_ref().unwrap().hash, hash2);
        assert_eq!(unchanged.lane, crate::board::Lane::Approved);
        assert!(unchanged.decision.is_some());

        let final_json = sample_plan_json().replace("\"edit\"", "\"edit-final\"");
        let (_, hash3) = edit_plan_from_json(
            &workspace,
            &state,
            "ship it",
            &final_json,
            unchanged.plan.as_ref().map(|p| p.tasks.clone()),
        )
        .unwrap();
        let revised = store
            .edit_plan(&item.id, &hash3, &workspace, &state, "bot:test")
            .unwrap();
        assert_eq!(revised.plan.as_ref().unwrap().revision, 3);
        assert_eq!(revised.lane, crate::board::Lane::NeedsDecision);
        assert!(revised.decision.is_none());
    }

    /// A user config with `[workflow] spec_first = "<mode>"`; hold the guard
    /// while plans are validated.
    fn spec_first_user_config(
        dir: &Path,
        mode: &str,
    ) -> (
        std::sync::MutexGuard<'static, ()>,
        Option<std::ffi::OsString>,
    ) {
        let guard = crate::state_path::test_env_lock();
        let xdg = dir.join("xdg");
        std::fs::create_dir_all(xdg.join("dmuxai")).unwrap();
        std::fs::write(
            xdg.join("dmuxai").join("dmuxai.toml"),
            format!("[workflow]\nspec_first = \"{mode}\"\n"),
        )
        .unwrap();
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &xdg) };
        (guard, prev)
    }

    fn restore_xdg(prev: Option<std::ffi::OsString>) {
        match prev {
            Some(value) => unsafe { std::env::set_var("XDG_CONFIG_HOME", value) },
            None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
        }
    }

    // TC-3715 / FR-3715, FR-3503 (#374): `board plan --edit`, Save plan and
    // `board add --plan-hash` store the spec-first expanded plan, so the
    // stored hash and steps are what `--approve-plan-id` executes; inserted
    // steps are marked.
    #[test]
    fn tc_3715_plan_edit_and_attach_store_spec_first_expanded_plan() {
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        ensure_dir(&state.join("board")).unwrap();
        let (guard, prev) = spec_first_user_config(dir.path(), "full");
        let one_step = sample_plan_json();
        let one_step = one_step.as_str();

        let (pending, hash) =
            edit_plan_from_json(&workspace, &state, "ship it", one_step, None).unwrap();
        let ids: Vec<&str> = pending.steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["spec-1", "step-1", "test-1"]);
        assert_eq!(pending.steps[1].depends_on, ["spec-1"]);
        assert_eq!(pending.steps[2].depends_on, ["step-1"]);
        assert_eq!(pending.steps[0].role, "spec");
        assert_eq!(pending.steps[2].role, "test");
        assert_eq!(pending_hash_for_plan(&pending).unwrap(), hash);
        assert_eq!(read_pending_plan(&state, &hash).unwrap(), pending);

        // Re-saving the expanded plan (dashboard Save plan) is a no-op.
        let resaved =
            serde_json::json!({ "tasks": pending.tasks, "steps": pending.steps }).to_string();
        let (_, hash_again) =
            edit_plan_from_json(&workspace, &state, "ship it", &resaved, None).unwrap();
        assert_eq!(hash_again, hash);

        // `board add --plan-hash` with an unexpanded pending plan stores the
        // expanded plan under its own hash.
        let raw_doc = parse_plan(one_step).unwrap();
        let raw = PendingPlanFile {
            prompt: "ship it".to_owned(),
            tasks: raw_doc.tasks,
            steps: raw_doc.steps,
            dynamic_cap: None,
        };
        let raw_hash = write_pending_plan(&state, &raw).unwrap();
        let store = BoardStore::open(state.join("board")).unwrap();
        let item = store
            .add(AddItemRequest {
                title: "expanded".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: None,
                source_bot: Some("bot".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(workspace.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: Some(raw_hash.clone()),
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let plan = item.plan.as_ref().unwrap();
        assert_eq!(plan.hash, hash);
        assert_ne!(plan.hash, raw_hash);
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.spec_first_added, ["spec-1", "test-1"]);
        assert!(board_plan_copy_path(&state.join("board"), &hash).exists());

        // With spec-first off nothing is inserted or marked.
        restore_xdg(prev);
        drop(guard);
        let (guard, prev) = spec_first_user_config(dir.path(), "off");
        let (off_pending, _) =
            edit_plan_from_json(&workspace, &state, "ship it", one_step, None).unwrap();
        assert_eq!(off_pending.steps.len(), 1);
        restore_xdg(prev);
        drop(guard);
    }

    // TC-3813 / FR-3813: a pending plan with `dynamic_cap` written by the headless
    // planner attaches by hash, board edits keep the cap, and plans without a
    // cap hash exactly as before.
    #[test]
    fn tc_3813_dynamic_cap_hash_roundtrip() {
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board_base = state.join("board");
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        ensure_dir(&state.join("graph-pending")).unwrap();
        let store = BoardStore::open(board_base.clone()).unwrap();
        let (guard, prev) = spec_first_user_config(dir.path(), "off");

        // Bytes exactly as `headless_write_pending_plan` serializes them.
        let step = r#"{"id":"step-1","role":"implement","job":"edit","runner":"cursor","depends_on":[],"task":"task-1"}"#;
        let task = r#"{"id":"task-1","title":"do it","done_check":"done","areas":[],"size":"small","depends_on":[]}"#;
        let headless_json =
            format!(r#"{{"prompt":"ship it","tasks":[{task}],"steps":[{step}],"dynamic_cap":3}}"#);
        let headless_hash: String = sha256_hex(headless_json.as_bytes())
            .chars()
            .take(16)
            .collect();
        std::fs::write(pending_plan_path(&state, &headless_hash), &headless_json).unwrap();

        let pending = read_pending_plan(&state, &headless_hash).unwrap();
        assert_eq!(pending.dynamic_cap, Some(3));
        assert_eq!(pending_hash_for_plan(&pending).unwrap(), headless_hash);
        assert_eq!(pending.json_and_hash().unwrap().0, headless_json);

        let item = store
            .add(AddItemRequest {
                title: "dynamic".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: None,
                source_bot: Some("bot".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(workspace.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: Some(headless_hash.clone()),
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .expect("board add --plan-hash accepts a dynamic plan");
        let attached = store
            .attach_plan(&item.id, &headless_hash, &state, "bot:test")
            .expect("board plan --hash accepts a dynamic plan");
        assert_eq!(attached.plan.as_ref().unwrap().hash, headless_hash);

        // Edit through the same path the dashboard, CLI and MCP use.
        let stored = unmasked_pending_from_item_plan(&state, &board_base, attached.plan.as_ref())
            .unwrap()
            .unwrap();
        let edited_json = sample_plan_json().replace("\"edit\"", "\"edit-more\"");
        let (edited, edited_hash) = edit_plan_from_json_keep_cap(
            &workspace,
            &state,
            &stored.prompt,
            &edited_json,
            attached.plan.as_ref().map(|p| p.tasks.clone()),
            stored.dynamic_cap,
        )
        .unwrap();
        assert_eq!(edited.dynamic_cap, Some(3));
        let edited_bytes = std::fs::read(pending_plan_path(&state, &edited_hash)).unwrap();
        let edited_prefix: String = sha256_hex(&edited_bytes).chars().take(16).collect();
        assert_eq!(edited_prefix, edited_hash);
        assert!(String::from_utf8_lossy(&edited_bytes).contains("\"dynamic_cap\":3"));
        let revised = store
            .edit_plan(&item.id, &edited_hash, &workspace, &state, "bot:test")
            .unwrap();
        assert_eq!(revised.plan.as_ref().unwrap().hash, edited_hash);
        assert_eq!(
            read_pending_plan(&state, &edited_hash).unwrap().dynamic_cap,
            Some(3)
        );

        // FR-3715 with FR-3813: spec-first on, `board add --plan-hash` verifies
        // the planner's hash, then stores the expanded plan under the hash of
        // its own bytes, which keep `dynamic_cap`; re-saving it is a no-op.
        restore_xdg(prev);
        drop(guard);
        let (guard, prev) = spec_first_user_config(dir.path(), "full");
        let expanded_item = store
            .add(AddItemRequest {
                title: "dynamic expanded".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: None,
                source_bot: Some("bot".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(workspace.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: Some(headless_hash.clone()),
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let expanded_plan = expanded_item.plan.as_ref().unwrap();
        assert_ne!(expanded_plan.hash, headless_hash);
        assert_eq!(expanded_plan.spec_first_added, ["spec-1", "test-1"]);
        let expanded_bytes = std::fs::read(pending_plan_path(&state, &expanded_plan.hash)).unwrap();
        let expanded_prefix: String = sha256_hex(&expanded_bytes).chars().take(16).collect();
        assert_eq!(expanded_prefix, expanded_plan.hash);
        let expanded = read_pending_plan(&state, &expanded_plan.hash).unwrap();
        assert_eq!(expanded.dynamic_cap, Some(3));
        assert_eq!(expanded.steps.len(), 3);
        let resaved =
            serde_json::json!({ "tasks": expanded.tasks, "steps": expanded.steps }).to_string();
        let (resaved_plan, resaved_hash) = edit_plan_from_json_keep_cap(
            &workspace,
            &state,
            &expanded.prompt,
            &resaved,
            None,
            expanded.dynamic_cap,
        )
        .unwrap();
        assert_eq!(resaved_plan.dynamic_cap, Some(3));
        assert_eq!(resaved_hash, expanded_plan.hash);
        restore_xdg(prev);
        drop(guard);

        // Boundary: no cap -> no `dynamic_cap` key, hash of the legacy bytes.
        let legacy_json = format!(r#"{{"prompt":"ship it","tasks":[{task}],"steps":[{step}]}}"#);
        let legacy: PendingPlanFile = serde_json::from_str(&legacy_json).unwrap();
        assert_eq!(legacy.dynamic_cap, None);
        let (legacy_out, legacy_hash) = legacy.json_and_hash().unwrap();
        assert_eq!(legacy_out, legacy_json);
        assert_eq!(
            legacy_hash,
            sha256_hex(legacy_json.as_bytes())
                .chars()
                .take(16)
                .collect::<String>()
        );
        assert_ne!(legacy_hash, headless_hash);

        // Failure: a tampered cap no longer matches the planner's hash.
        let tampered = headless_json.replace("\"dynamic_cap\":3", "\"dynamic_cap\":8");
        let tampered_hash: String = sha256_hex(tampered.as_bytes()).chars().take(16).collect();
        std::fs::write(pending_plan_path(&state, &headless_hash), &tampered).unwrap();
        let err = store
            .attach_plan(&item.id, &headless_hash, &state, "bot:test")
            .unwrap_err();
        assert!(matches!(err, BoardError::Failed(_)), "{err}");
        assert_ne!(tampered_hash, headless_hash);
    }

    #[test]
    fn custom_runner_default_model_satisfies_plan_validation() {
        use crate::custom_agent::CustomAgentSpec;
        let mut settings = DmuxSettings::default();
        settings.custom_agents.push(CustomAgentSpec {
            name: "mybot".to_owned(),
            default_model: Some("composer-2.5".to_owned()),
            ..Default::default()
        });
        let holder = PlanValidationCtx::from_settings(&settings);
        assert!(holder.runners.iter().any(|r| r == "mybot"));
        let plan_json = r#"{"tasks":[{"id":"task-1","title":"do it","done_check":"done","size":"small","depends_on":[]}],"steps":[{"id":"step-1","role":"implement","job":"edit","runner":"mybot","depends_on":[],"task":"task-1"}]}"#;
        let doc = parse_plan(plan_json).unwrap();
        let ctx = PlanContext {
            user_prompt: "prompt",
            available_runners: &holder.runners,
            runner_allowed: &|runner: &str| settings.graph_runner_allowed(runner),
            effective_model: &|runner: &str| holder.effective_model(runner),
            validate_model: &crate::model_policy::validate_model_for_runner,
        };
        validate_plan_with_limit(&doc, &ctx, 8).expect("custom default_model should validate");
    }

    #[test]
    fn tc_3130_plan_copy_written_and_restores_missing_pending() {
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let board_base = state.join("board");
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        ensure_dir(&board_base).unwrap();
        let store = BoardStore::open(board_base.clone()).unwrap();
        let item = store
            .add(AddItemRequest {
                title: "plan copy".to_owned(),
                summary: String::new(),
                kind: Kind::Feature,
                grade: None,
                lane: None,
                source_bot: Some("bot".to_owned()),
                source_channel: None,
                source_ref: None,
                workspace: Some(workspace.clone()),
                recommendation: None,
                options: vec![],
                plan_hash: None,
                key: None,
                by: "bot:test".to_owned(),
                state_root: state.clone(),
            })
            .unwrap();
        let (_, hash) =
            edit_plan_from_json(&workspace, &state, "keep me", &sample_plan_json(), None).unwrap();
        store
            .attach_plan(&item.id, &hash, &state, "bot:test")
            .unwrap();

        let copy_path = board_plan_copy_path(&board_base, &hash);
        assert!(copy_path.exists(), "board plan copy should be written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&copy_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // Remove graph-pending/<hash>.json and verify fallback + restore.
        let pending_path = pending_plan_path(&state, &hash);
        std::fs::remove_file(&pending_path).unwrap();
        assert!(!pending_path.exists());

        let item_with_plan = store.get(&item.id).unwrap();
        assert_eq!(
            unmasked_prompt_from_item_plan(&state, &board_base, item_with_plan.plan.as_ref())
                .unwrap()
                .as_deref(),
            Some("keep me")
        );

        let restored = ensure_pending_plan_from_copy(&state, &board_base, &hash).unwrap();
        assert_eq!(restored.prompt, "keep me");
        assert!(pending_path.exists());

        // Corrupt copy fails sha256 verification.
        std::fs::remove_file(&pending_path).unwrap();
        std::fs::write(&copy_path, b"{\"prompt\":\"tampered\"}").unwrap();
        let err = ensure_pending_plan_from_copy(&state, &board_base, &hash).unwrap_err();
        assert!(matches!(err, BoardError::Failed(_)));

        // Prune removes copies whose done/rejected items and files are > 30 days old,
        // while keeping active/recent copies.
        let old_copy = board_plan_copy_path(&board_base, "0123456789abcdef");
        std::fs::write(&old_copy, b"{}").unwrap();
        let old_time = filetime::FileTime::from_unix_time(1, 0);
        filetime::set_file_mtime(&old_copy, old_time).unwrap();
        let now_ms = 60 * 86_400 * 1000u64;
        prune_old_plan_copies(&board_base, &[item_with_plan], now_ms);
        assert!(
            !old_copy.exists(),
            "old orphan/closed copy should be pruned"
        );
        assert!(copy_path.exists(), "active item plan copy should be kept");
    }
}
