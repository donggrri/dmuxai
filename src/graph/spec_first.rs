//! Spec-first plan insertion, gates, and prompts (SPEC-spec-first-workflow FR-3503..FR-3512).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::run_journal::{mask_sensitive_tokens, truncate_tail_utf8};
use crate::settings::{SpecFirstMode, WorkflowSettings};

use super::plan_doc::{PLAN_PROMPT, PlanStep};
use super::replan::REPLAN_PROMPT;
use super::types::{GraphEdge, GraphNode, GraphNodeStatus, GraphPlan, NodeRole};

const SPEC_RULE: &str =
    "A plan that changes code starts with a spec step that writes or updates the spec under ";
const OLD_ALLOWED_ROLES: &str =
    "Allowed roles: research, explore, test, implement, draft, plan, route, review.";
const NEW_ALLOWED_ROLES: &str =
    "Allowed roles: spec, research, explore, test, implement, draft, plan, route, review.";

/// Recorded test gate outcome (FR-3509/FR-3510).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestGate {
    pub source: String,
    pub command: String,
    pub exit: i32,
    pub duration_ms: u64,
    pub output_tail: String,
}

/// Result of the spec diff gate (FR-3508).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecGateResult {
    Ok,
    Skipped(&'static str),
    Failed(String),
}

fn is_write_role(role: NodeRole) -> bool {
    matches!(role, NodeRole::Implement | NodeRole::Draft)
}

fn node_index(plan: &GraphPlan) -> HashMap<&str, &GraphNode> {
    plan.nodes.iter().map(|n| (n.id.as_str(), n)).collect()
}

fn build_incoming(plan: &GraphPlan) -> HashMap<String, Vec<String>> {
    let mut incoming: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &plan.edges {
        incoming
            .entry(edge.to.clone())
            .or_default()
            .push(edge.from.clone());
    }
    incoming
}

fn build_outgoing(plan: &GraphPlan) -> HashMap<String, Vec<String>> {
    let mut outgoing: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &plan.edges {
        outgoing
            .entry(edge.from.clone())
            .or_default()
            .push(edge.to.clone());
    }
    outgoing
}

fn ancestors(plan: &GraphPlan, node_id: &str) -> HashSet<String> {
    let incoming = build_incoming(plan);
    let mut seen = HashSet::new();
    let mut stack = vec![node_id.to_owned()];
    while let Some(cur) = stack.pop() {
        if let Some(parents) = incoming.get(&cur) {
            for p in parents {
                if seen.insert(p.clone()) {
                    stack.push(p.clone());
                }
            }
        }
    }
    seen
}

fn is_ancestor(plan: &GraphPlan, from: &str, to: &str) -> bool {
    if from == to {
        return true;
    }
    let outgoing = build_outgoing(plan);
    let mut seen = HashSet::new();
    let mut stack = vec![from.to_owned()];
    while let Some(cur) = stack.pop() {
        if cur == to {
            return true;
        }
        if let Some(nexts) = outgoing.get(&cur) {
            for n in nexts {
                if seen.insert(n.clone()) {
                    stack.push(n.clone());
                }
            }
        }
    }
    false
}

pub fn node_has_spec_ancestor(plan: &GraphPlan, write_id: &str) -> bool {
    let index = node_index(plan);
    ancestors(plan, write_id).into_iter().any(|id| {
        index
            .get(id.as_str())
            .is_some_and(|n| n.role == NodeRole::Spec)
    })
}

fn write_step_ids(plan: &GraphPlan) -> Vec<String> {
    plan.nodes
        .iter()
        .filter(|n| is_write_role(n.role))
        .map(|n| n.id.clone())
        .collect()
}

fn plan_is_tested(plan: &GraphPlan, write_ids: &[String]) -> bool {
    if write_ids.is_empty() {
        return true;
    }
    plan.nodes
        .iter()
        .any(|n| n.role == NodeRole::Test && write_ids.iter().all(|w| is_ancestor(plan, w, &n.id)))
}

fn write_descendants(plan: &GraphPlan, write_id: &str) -> HashSet<String> {
    let index = node_index(plan);
    let outgoing = build_outgoing(plan);
    let mut seen = HashSet::new();
    let mut stack = vec![write_id.to_owned()];
    while let Some(cur) = stack.pop() {
        if let Some(nexts) = outgoing.get(&cur) {
            for n in nexts {
                if seen.insert(n.clone())
                    && index
                        .get(n.as_str())
                        .is_some_and(|node| is_write_role(node.role))
                {
                    stack.push(n.clone());
                }
            }
        }
    }
    seen
}

fn next_free_id(plan: &GraphPlan, prefix: &str) -> String {
    let mut n = 1u32;
    loop {
        let id = format!("{prefix}-{n}");
        if !plan.nodes.iter().any(|node| node.id == id) {
            return id;
        }
        n += 1;
    }
}

fn first_write_step(plan: &GraphPlan) -> Option<&GraphNode> {
    plan.nodes.iter().find(|n| is_write_role(n.role))
}

fn make_spec_node(id: &str, template: &GraphNode, user_prompt: &str, spec_dir: &str) -> GraphNode {
    GraphNode {
        id: id.to_owned(),
        role: NodeRole::Spec,
        job: format!("Write or update the spec under {spec_dir} for: {user_prompt}"),
        runner: template.runner.clone(),
        model_id: template.model_id.clone(),
        skill: template.skill.clone(),
        requires_gate: false,
        status: GraphNodeStatus::Pending,
        attempt: 0,
        task: template.task.clone(),
        started_at_ms: None,
        finished_at_ms: None,
        test_gate: None,
    }
}

fn make_test_node(id: &str, template: &GraphNode, user_prompt: &str) -> GraphNode {
    GraphNode {
        id: id.to_owned(),
        role: NodeRole::Test,
        job: format!("Run the tests for: {user_prompt}"),
        runner: template.runner.clone(),
        model_id: template.model_id.clone(),
        skill: template.skill.clone(),
        requires_gate: false,
        status: GraphNodeStatus::Pending,
        attempt: 0,
        task: template.task.clone(),
        started_at_ms: None,
        finished_at_ms: None,
        test_gate: None,
    }
}

/// Makes a plan covered and tested per FR-3503/FR-3504; idempotent.
pub fn apply_spec_first(plan: &mut GraphPlan, wf: &WorkflowSettings) -> Vec<String> {
    if wf.spec_first == SpecFirstMode::Off {
        return Vec::new();
    }
    let write_ids = write_step_ids(plan);
    if write_ids.is_empty() {
        return Vec::new();
    }

    let mut inserted = Vec::new();
    let spec_dir = wf.spec_dir.as_str();

    let uncovered: Vec<String> = write_ids
        .iter()
        .filter(|id| !node_has_spec_ancestor(plan, id))
        .cloned()
        .collect();

    let need_spec = !uncovered.is_empty();
    let need_test = wf.require_tests && !plan_is_tested(plan, &write_ids);

    if !need_spec && !need_test {
        return Vec::new();
    }

    let template = first_write_step(plan).expect("write step exists").clone();

    if need_spec {
        let spec_id = next_free_id(plan, "spec");
        let spec_node = make_spec_node(&spec_id, &template, &plan.user_prompt, spec_dir);
        let uncovered_roots: Vec<String> = uncovered
            .iter()
            .filter(|id| {
                !uncovered
                    .iter()
                    .any(|other| other.as_str() != id.as_str() && is_ancestor(plan, other, id))
            })
            .cloned()
            .collect();
        for to in uncovered_roots {
            plan.edges.push(GraphEdge {
                from: spec_id.clone(),
                to,
            });
        }
        plan.nodes.insert(0, spec_node);
        inserted.push(spec_id);
    }

    if need_test {
        let write_ids = write_step_ids(plan);
        let test_id = next_free_id(plan, "test");
        let test_node = make_test_node(&test_id, &template, &plan.user_prompt);
        for write_id in &write_ids {
            if write_descendants(plan, write_id).is_empty() {
                plan.edges.push(GraphEdge {
                    from: write_id.clone(),
                    to: test_id.clone(),
                });
            }
        }
        plan.nodes.push(test_node);
        inserted.push(test_id);
    }

    inserted
}

/// Expands plan steps the way [`apply_spec_first`] expands their graph
/// (FR-3503, FR-3715). `graph` must be the validated graph of `steps`.
/// Returns the steps a run executes and the ids of the inserted steps;
/// existing steps keep their fields and only gain the new dependencies.
pub fn expand_plan_steps(
    steps: &[PlanStep],
    graph: &GraphPlan,
    wf: &WorkflowSettings,
) -> (Vec<PlanStep>, Vec<String>) {
    let mut expanded = graph.clone();
    let inserted = apply_spec_first(&mut expanded, wf);
    if inserted.is_empty() {
        return (steps.to_vec(), inserted);
    }
    let template_step = first_write_step(graph)
        .and_then(|node| steps.iter().find(|step| step.id == node.id))
        .cloned()
        .unwrap_or_default();
    let incoming = build_incoming(&expanded);
    let out = expanded
        .nodes
        .iter()
        .filter_map(|node| {
            let deps = incoming.get(&node.id).cloned().unwrap_or_default();
            if let Some(step) = steps.iter().find(|step| step.id == node.id) {
                let mut step = step.clone();
                for dep in deps {
                    if !step.depends_on.contains(&dep) {
                        step.depends_on.push(dep);
                    }
                }
                Some(step)
            } else if inserted.contains(&node.id) {
                Some(PlanStep {
                    id: node.id.clone(),
                    role: node.role.to_string(),
                    job: node.job.clone(),
                    runner: template_step.runner.clone(),
                    model: template_step.model.clone(),
                    depends_on: deps,
                    sensitive: None,
                    task: template_step.task.clone(),
                    ..Default::default()
                })
            } else {
                None
            }
        })
        .collect();
    (out, inserted)
}

/// Whether a step carries the fixed job [`apply_spec_first`] gives the spec
/// and test steps it inserts for `user_prompt` (FR-3715).
pub fn is_spec_first_step(role: &str, job: &str, user_prompt: &str) -> bool {
    match role {
        "spec" => {
            job.starts_with("Write or update the spec under ")
                && job.ends_with(&format!(" for: {user_prompt}"))
        }
        "test" => job == format!("Run the tests for: {user_prompt}"),
        _ => false,
    }
}

/// Builds the spec → implement → test graph for simple runs (FR-3505).
pub fn three_step_plan(base: GraphPlan) -> GraphPlan {
    let wf = WorkflowSettings {
        require_tests: true,
        ..WorkflowSettings::default()
    };
    let mut plan = base;
    apply_spec_first(&mut plan, &wf);
    plan
}

pub fn plan_prompt_for(mode: SpecFirstMode, spec_dir: &str) -> String {
    if mode == SpecFirstMode::Off {
        return PLAN_PROMPT.to_owned();
    }
    let rule = format!(
        "{SPEC_RULE}{spec_dir}; every implement or draft step depends on it, and a test step depends on every implement or draft step. Other roles have no fixed order."
    );
    let mut out = PLAN_PROMPT.replace(OLD_ALLOWED_ROLES, NEW_ALLOWED_ROLES);
    out.push('\n');
    out.push_str(&rule);
    out
}

pub fn replan_prompt_for(mode: SpecFirstMode, spec_dir: &str) -> String {
    if mode == SpecFirstMode::Off {
        return REPLAN_PROMPT.to_owned();
    }
    let rule = format!(
        "{SPEC_RULE}{spec_dir}; every implement or draft step depends on it, and a test step depends on every implement or draft step. Other roles have no fixed order."
    );
    let mut out = REPLAN_PROMPT.replace(OLD_ALLOWED_ROLES, NEW_ALLOWED_ROLES);
    out.push('\n');
    out.push_str(&rule);
    out
}

pub fn spec_node_directive(spec_dir: &str) -> String {
    format!(
        "Write or update a spec under {spec_dir} with FR and TC rows for this request. Change no file outside {spec_dir}."
    )
}

pub fn implement_after_spec_directive() -> String {
    "Follow the spec written by the earlier spec step.".to_owned()
}

pub fn test_node_directive(wf: &WorkflowSettings) -> String {
    if !wf.test_command.trim().is_empty() {
        format!(
            "After this step dmuxai runs `{}`; make sure it passes.",
            wf.test_command.trim()
        )
    } else {
        "Run the project's tests. End your reply with one line: DMUXAI_TESTS: {\"command\": \"<what you ran>\", \"exit\": <exit code>}".to_owned()
    }
}

pub fn lite_directive(spec_dir: &str, wf: &WorkflowSettings) -> String {
    let mut parts = vec![spec_node_directive(spec_dir)];
    parts.push(implement_after_spec_directive());
    parts.push(test_node_directive(wf));
    format!(
        "Spec first in one turn: {}\nThen implement.\n{}",
        parts[0], parts[2]
    )
}

fn path_under_spec_dir(path: &str, spec_dir: &str) -> bool {
    let norm = path.replace('\\', "/");
    let dir = spec_dir.trim().trim_matches('/').replace('\\', "/");
    norm == dir || norm.starts_with(&format!("{dir}/"))
}

fn ignore_dmuxai_path(path: &str) -> bool {
    let norm = path.replace('\\', "/");
    norm == ".dmuxai" || norm.starts_with(".dmuxai/")
}

/// Workspace git porcelain snapshot (FR-3508).
pub fn spec_snapshot(workspace: &Path) -> Option<BTreeMap<String, String>> {
    snapshot_impl(workspace, None)
}

/// Like [`spec_snapshot`], but entries under `spec_dir` also carry a size and
/// modification-time fingerprint, so a spec file that stays untracked or
/// modified between two snapshots still counts as changed when a step rewrites
/// it (a spec re-run after Request changes, SPEC-spec-review-gate FR-3705).
pub fn spec_snapshot_in(workspace: &Path, spec_dir: &str) -> Option<BTreeMap<String, String>> {
    snapshot_impl(workspace, Some(spec_dir))
}

fn file_fingerprint(workspace: &Path, path: &str) -> String {
    let Ok(meta) = std::fs::symlink_metadata(workspace.join(path)) else {
        return "|gone".to_owned();
    };
    let nanos = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("|{}:{nanos}", meta.len())
}

fn snapshot_impl(workspace: &Path, spec_dir: Option<&str>) -> Option<BTreeMap<String, String>> {
    if !workspace.join(".git").exists() {
        let output = Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(workspace)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
    }
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .current_dir(workspace)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut map = BTreeMap::new();
    let bytes = output.stdout;
    let mut i = 0usize;
    while i < bytes.len() {
        if i + 3 > bytes.len() {
            break;
        }
        let status = String::from_utf8_lossy(&bytes[i..i + 2]).to_string();
        i += 3;
        let start = i;
        while i < bytes.len() && bytes[i] != 0 {
            i += 1;
        }
        let path = String::from_utf8_lossy(&bytes[start..i]).to_string();
        i += 1;
        if status.starts_with('R') || status.starts_with('C') {
            let start2 = i;
            while i < bytes.len() && bytes[i] != 0 {
                i += 1;
            }
            let new_path = String::from_utf8_lossy(&bytes[start2..i]).to_string();
            i += 1;
            map.insert(new_path, status.clone());
        } else {
            map.insert(path, status);
        }
    }
    if let Some(dir) = spec_dir {
        for (path, status) in map.iter_mut() {
            if path_under_spec_dir(path, dir) {
                status.push_str(&file_fingerprint(workspace, path));
            }
        }
    }
    Some(map)
}

pub fn ensure_spec_dir(workspace: &Path, spec_dir: &str) -> io::Result<()> {
    std::fs::create_dir_all(workspace.join(spec_dir))
}

fn changed_since(
    before: Option<&BTreeMap<String, String>>,
    after: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut changed = Vec::new();
    for (path, status) in after {
        if ignore_dmuxai_path(path) {
            continue;
        }
        let is_changed = match before {
            None => true,
            Some(prev) => prev.get(path) != Some(status),
        };
        if is_changed {
            changed.push(path.clone());
        }
    }
    changed
}

/// Workspace-relative paths under `spec_dir` that changed since `before`
/// (the paths a spec review shows, SPEC-spec-review-gate FR-3703). Sorted; never
/// contains a path outside `spec_dir` or one that escapes the workspace.
pub fn changed_spec_paths(
    before: Option<&BTreeMap<String, String>>,
    workspace: &Path,
    spec_dir: &str,
) -> Vec<String> {
    let Some(after) = snapshot_impl(workspace, Some(spec_dir)) else {
        return Vec::new();
    };
    changed_since(before, &after)
        .into_iter()
        .filter(|path| path_under_spec_dir(path, spec_dir) && !path.split('/').any(|c| c == ".."))
        .collect()
}

pub fn check_spec_gate(
    before: Option<&BTreeMap<String, String>>,
    workspace: &Path,
    spec_dir: &str,
    lite: bool,
) -> SpecGateResult {
    let after = match snapshot_impl(workspace, Some(spec_dir)) {
        Some(map) => map,
        None => return SpecGateResult::Skipped("spec gate skipped (not a git workspace)"),
    };
    let changed = changed_since(before, &after);

    if changed.is_empty() {
        return SpecGateResult::Failed(format!("spec step did not write a spec under {spec_dir}"));
    }

    let under_spec: Vec<String> = changed
        .iter()
        .filter(|p| path_under_spec_dir(p, spec_dir))
        .cloned()
        .collect();

    if !lite {
        let outside: Vec<String> = changed
            .iter()
            .filter(|p| !path_under_spec_dir(p, spec_dir))
            .cloned()
            .collect();
        if !outside.is_empty() {
            return SpecGateResult::Failed(format!(
                "spec step changed files outside {spec_dir}: {}",
                outside.join(", ")
            ));
        }
    }

    if under_spec.is_empty() {
        return SpecGateResult::Failed(format!("spec step did not write a spec under {spec_dir}"));
    }

    SpecGateResult::Ok
}

#[cfg(unix)]
fn kill_child_process_group(child: &mut std::process::Child) {
    let pgid = child.id() as i32;
    if pgid > 0 {
        // SAFETY: child spawned with process_group(0).
        unsafe {
            libc::killpg(pgid, libc::SIGTERM);
        }
        let grace = Instant::now() + Duration::from_millis(200);
        while Instant::now() < grace {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Runs `sh -c cmd` and records output (FR-3509).
pub fn run_test_command(
    workspace: &Path,
    cmd: &str,
    timeout: Duration,
) -> (TestGate, Result<(), String>) {
    let start = Instant::now();
    let mut command = Command::new("sh");
    command.arg("-c").arg(cmd).current_dir(workspace);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            let gate = TestGate {
                source: "command".to_owned(),
                command: cmd.to_owned(),
                exit: 127,
                duration_ms: 0,
                output_tail: mask_sensitive_tokens(&e.to_string()),
            };
            return (gate, Err("test_command failed (exit 127)".to_owned()));
        }
    };

    let deadline = start + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if Instant::now() >= deadline {
                    #[cfg(unix)]
                    kill_child_process_group(&mut child);
                    #[cfg(not(unix))]
                    {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    let gate = TestGate {
                        source: "command".to_owned(),
                        command: cmd.to_owned(),
                        exit: 124,
                        duration_ms: start.elapsed().as_millis() as u64,
                        output_tail: String::new(),
                    };
                    let secs = timeout.as_secs();
                    return (gate, Err(format!("test_command timed out after {secs}s")));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                #[cfg(unix)]
                kill_child_process_group(&mut child);
                let gate = TestGate {
                    source: "command".to_owned(),
                    command: cmd.to_owned(),
                    exit: 1,
                    duration_ms: start.elapsed().as_millis() as u64,
                    output_tail: mask_sensitive_tokens(&e.to_string()),
                };
                return (gate, Err("test_command failed (exit 1)".to_owned()));
            }
        }
    };

    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut out, &mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut err, &mut stderr);
    }

    let exit = status.map(|s| s.code().unwrap_or(1)).unwrap_or(124);
    let combined = format!("{stdout}{stderr}");
    let masked = mask_sensitive_tokens(&combined);
    const TAIL: usize = 8 * 1024;
    let tail = truncate_tail_utf8(&masked, TAIL).to_owned();
    let gate = TestGate {
        source: "command".to_owned(),
        command: cmd.to_owned(),
        exit,
        duration_ms: start.elapsed().as_millis() as u64,
        output_tail: tail,
    };
    if exit == 0 {
        (gate, Ok(()))
    } else {
        (gate, Err(format!("test_command failed (exit {exit})")))
    }
}

pub fn parse_tests_marker(reply: &str) -> Option<TestGate> {
    let line = reply
        .lines()
        .find(|l| l.trim_start().starts_with("DMUXAI_TESTS:"))?;
    let json_part = line.split_once(':').map(|(_, j)| j.trim())?;
    let value: serde_json::Value = serde_json::from_str(json_part).ok()?;
    let command = value.get("command")?.as_str()?.to_owned();
    let exit = value.get("exit")?.as_i64()? as i32;
    Some(TestGate {
        source: "marker".to_owned(),
        command,
        exit,
        duration_ms: 0,
        output_tail: String::new(),
    })
}

pub fn check_marker_gate(reply: &str, require_tests: bool) -> Result<Option<TestGate>, String> {
    if !require_tests {
        return Ok(None);
    }
    match parse_tests_marker(reply) {
        Some(gate) if gate.exit == 0 => Ok(Some(gate)),
        Some(_) => Err("test step did not report a passing test run (DMUXAI_TESTS)".to_owned()),
        None => Err("test step did not report a passing test run (DMUXAI_TESTS)".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::WorkflowSettings;

    fn sample_write_plan() -> GraphPlan {
        GraphPlan {
            schema: "cursor-agent-graph/v1".to_owned(),
            id: "p1".to_owned(),
            orchestrator_model: "composer-2.5".to_owned(),
            user_prompt: "fix bug".to_owned(),
            nodes: vec![GraphNode {
                id: "step-1".to_owned(),
                role: NodeRole::Implement,
                job: "fix".to_owned(),
                runner: "cursor".to_owned(),
                model_id: "composer-2.5".to_owned(),
                skill: None,
                requires_gate: false,
                status: GraphNodeStatus::Pending,
                attempt: 0,
                task: Some("task-1".to_owned()),
                started_at_ms: None,
                finished_at_ms: None,
                test_gate: None,
            }],
            edges: vec![],
            limits: Default::default(),
            tasks: vec![],
            dynamic_cap: None,
            workflow: None,
        }
    }

    #[test]
    fn tc_3502_plan_prompt_spec_rule() {
        let off = plan_prompt_for(SpecFirstMode::Off, "docs/specs");
        assert_eq!(off, PLAN_PROMPT);
        let full = plan_prompt_for(SpecFirstMode::Full, "docs/specs");
        assert!(full.contains("spec,"));
        assert!(full.contains("docs/specs"));
        let replan = replan_prompt_for(SpecFirstMode::Full, "docs/specs");
        assert!(replan.contains("spec,"));
    }

    #[test]
    fn tc_3503_auto_insert_cases() {
        let wf = WorkflowSettings::default();
        // research only — unchanged
        let mut research = GraphPlan {
            nodes: vec![GraphNode {
                id: "r1".to_owned(),
                role: NodeRole::Research,
                job: "look".to_owned(),
                runner: "cursor".to_owned(),
                model_id: "composer-2.5".to_owned(),
                skill: None,
                requires_gate: false,
                status: GraphNodeStatus::Pending,
                attempt: 0,
                task: None,
                started_at_ms: None,
                finished_at_ms: None,
                test_gate: None,
            }],
            ..sample_write_plan()
        };
        research.nodes = vec![research.nodes[0].clone()];
        assert!(apply_spec_first(&mut research, &wf).is_empty());

        // implement only
        let mut only_impl = sample_write_plan();
        let ids = apply_spec_first(&mut only_impl, &wf);
        assert_eq!(ids, vec!["spec-1", "test-1"]);
        assert!(
            only_impl
                .edges
                .iter()
                .any(|e| e.from == "spec-1" && e.to == "step-1")
        );
        assert!(
            only_impl
                .edges
                .iter()
                .any(|e| e.from == "step-1" && e.to == "test-1")
        );
        assert_eq!(only_impl.nodes.first().unwrap().role, NodeRole::Spec);
        assert!(apply_spec_first(&mut only_impl, &wf).is_empty());
    }

    #[test]
    fn tc_3504_replan_reinserts_test() {
        let wf = WorkflowSettings::default();
        let mut plan = sample_write_plan();
        plan.nodes.insert(
            0,
            GraphNode {
                id: "spec-1".to_owned(),
                role: NodeRole::Spec,
                job: "spec".to_owned(),
                runner: "cursor".to_owned(),
                model_id: "composer-2.5".to_owned(),
                skill: None,
                requires_gate: false,
                status: GraphNodeStatus::Completed,
                attempt: 1,
                task: None,
                started_at_ms: None,
                finished_at_ms: None,
                test_gate: None,
            },
        );
        plan.edges.push(GraphEdge {
            from: "spec-1".to_owned(),
            to: "step-1".to_owned(),
        });
        let ids = apply_spec_first(&mut plan, &wf);
        assert_eq!(ids, vec!["test-1"]);
    }

    #[test]
    fn tc_3508_spec_gate_cases() {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "t@t.com"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        let before = spec_snapshot(dir.path()).unwrap();
        let spec_path = dir.path().join("docs/specs/x.md");
        std::fs::create_dir_all(spec_path.parent().unwrap()).unwrap();
        std::fs::write(spec_path, "a").unwrap();
        match check_spec_gate(Some(&before), dir.path(), "docs/specs", false) {
            SpecGateResult::Ok => {}
            other => panic!("expected ok, got {other:?}"),
        }
        let before2 = spec_snapshot(dir.path()).unwrap();
        let src_path = dir.path().join("src/a.rs");
        std::fs::create_dir_all(src_path.parent().unwrap()).unwrap();
        std::fs::write(src_path, "b").unwrap();
        match check_spec_gate(Some(&before2), dir.path(), "docs/specs", false) {
            SpecGateResult::Failed(msg) => assert!(msg.contains("src/a.rs")),
            other => panic!("expected fail, got {other:?}"),
        }
        let no_git = std::env::temp_dir().join(format!("dmux-nogit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&no_git).unwrap();
        match check_spec_gate(None, &no_git, "docs/specs", false) {
            SpecGateResult::Skipped(_) => {}
            other => panic!("expected skip, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&no_git);
    }

    #[test]
    fn tc_3509_test_command_gate() {
        let dir = tempfile::tempdir().unwrap();
        let (gate, ok) = run_test_command(dir.path(), "exit 0", Duration::from_secs(5));
        assert!(ok.is_ok());
        assert_eq!(gate.exit, 0);
        let (_, err) = run_test_command(dir.path(), "exit 3", Duration::from_secs(5));
        assert!(err.unwrap_err().contains("exit 3"));
        let (_, tout) = run_test_command(dir.path(), "sleep 5", Duration::from_secs(1));
        assert!(tout.unwrap_err().contains("timed out"));
    }

    #[test]
    fn tc_3510_marker_gate() {
        let ok = "done\nDMUXAI_TESTS: {\"command\": \"cargo test\", \"exit\": 0}\n";
        assert!(check_marker_gate(ok, true).unwrap().is_some());
        let bad = "DMUXAI_TESTS: {\"command\": \"cargo test\", \"exit\": 1}\n";
        assert!(check_marker_gate(bad, true).is_err());
        assert!(check_marker_gate("nope", false).unwrap().is_none());
    }

    #[test]
    fn tc_3512_node_directives() {
        let wf = WorkflowSettings::default();
        assert!(spec_node_directive("docs/specs").contains("docs/specs"));
        assert!(implement_after_spec_directive().contains("earlier spec"));
        assert!(test_node_directive(&wf).contains("DMUXAI_TESTS"));
        let mut wf_cmd = wf.clone();
        wf_cmd.test_command = "cargo test".to_owned();
        assert!(test_node_directive(&wf_cmd).contains("cargo test"));
        assert!(lite_directive("docs/specs", &wf).contains("docs/specs"));
    }

    #[test]
    fn tc_3515_apply_spec_first_perf() {
        let wf = WorkflowSettings::default();
        let mut base = sample_write_plan();
        for i in 2..=8 {
            base.nodes.push(GraphNode {
                id: format!("step-{i}"),
                role: NodeRole::Implement,
                job: format!("job {i}"),
                runner: "cursor".to_owned(),
                model_id: "composer-2.5".to_owned(),
                skill: None,
                requires_gate: false,
                status: GraphNodeStatus::Pending,
                attempt: 0,
                task: None,
                started_at_ms: None,
                finished_at_ms: None,
                test_gate: None,
            });
        }
        let start = Instant::now();
        let n = 10_000usize;
        for _ in 0..n {
            let mut plan = base.clone();
            apply_spec_first(&mut plan, &wf);
        }
        let avg_ms = start.elapsed().as_micros() as f64 / n as f64 / 1000.0;
        eprintln!("tc_3515 apply_spec_first avg_ms={avg_ms}");
        assert!(avg_ms < 1.0, "avg {avg_ms} ms");
    }
}
