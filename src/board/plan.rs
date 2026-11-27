//! Pending graph plan attach/edit (FR-3106, FR-3107).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{BoardError, BoardPlan, PLAN_HISTORY_MAX, mask_board_plan};
use crate::docs::sha256_hex;
use crate::graph::{
    PlanContext, PlanDoc, PlanStep, PlanTask, parse_plan, validate_plan_with_limit,
};
use crate::settings::DmuxSettings;
use crate::state_path::valid_pending_plan_hash;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingPlanFile {
    pub prompt: String,
    pub tasks: Vec<PlanTask>,
    pub steps: Vec<PlanStep>,
}

pub fn pending_plan_path(state: &Path, hash: &str) -> PathBuf {
    state.join("graph-pending").join(format!("{hash}.json"))
}

pub fn read_pending_plan(state: &Path, hash: &str) -> Result<PendingPlanFile, BoardError> {
    if !valid_pending_plan_hash(hash) {
        return Err(BoardError::Usage(
            "plan hash must be 16 lowercase hex characters".to_owned(),
        ));
    }
    let path = pending_plan_path(state, hash);
    let bytes = std::fs::read(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            BoardError::NotFound(format!("No pending plan found for '{hash}'"))
        } else {
            BoardError::Internal(e.to_string())
        }
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|e| BoardError::Failed(format!("Pending plan '{hash}' is invalid: {e}")))
}

pub fn pending_hash_for_plan(plan: &PendingPlanFile) -> Result<String, BoardError> {
    let json = serde_json::to_string(plan).map_err(|e| BoardError::Internal(e.to_string()))?;
    Ok(sha256_hex(json.as_bytes())
        .chars()
        .take(16)
        .collect::<String>())
}

pub fn write_pending_plan(state: &Path, plan: &PendingPlanFile) -> Result<String, BoardError> {
    let hash = pending_hash_for_plan(plan)?;
    let dir = state.join("graph-pending");
    crate::board::store::ensure_dir(&dir)?;
    let path = dir.join(format!("{hash}.json"));
    let json = serde_json::to_string(plan).map_err(|e| BoardError::Internal(e.to_string()))?;
    crate::board::store::atomic_write(&path, json.as_bytes())?;
    Ok(hash)
}

pub fn unmasked_prompt_from_item_plan(
    state_root: &Path,
    plan: Option<&super::BoardPlan>,
) -> Result<Option<String>, BoardError> {
    let Some(plan) = plan else {
        return Ok(None);
    };
    let pending = read_pending_plan(state_root, &plan.hash)?;
    Ok(Some(pending.prompt))
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
    let holder = PlanValidationCtx::from_settings(&settings);
    let cap = settings.graph_max_nodes.clamp(1, 8) as usize;
    let ctx = PlanContext {
        user_prompt: stored_prompt,
        available_runners: &holder.runners,
        runner_allowed: &|runner: &str| settings.graph_runner_allowed(runner),
        effective_model: &|runner: &str| holder.effective_model(runner),
        validate_model: &crate::model_policy::validate_model_for_runner,
    };
    let _graph = validate_plan_with_limit(&doc, &ctx, cap).map_err(|errors| {
        BoardError::Failed(
            errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    })?;
    let plan = pending_from_doc(stored_prompt, &doc);
    let hash = write_pending_plan(state_root, &plan)?;
    Ok((plan, hash))
}

pub fn pending_from_doc(prompt: &str, doc: &PlanDoc) -> PendingPlanFile {
    PendingPlanFile {
        prompt: prompt.to_owned(),
        tasks: doc.tasks.clone(),
        steps: doc.steps.clone(),
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
    let mut plan = BoardPlan {
        hash,
        prompt: pending.prompt,
        tasks: pending.tasks,
        steps: pending.steps,
        revision,
        history,
        previous_steps,
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
        assert_eq!(edited.plan.as_ref().unwrap().steps[0].job, "edit-more");
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
}
