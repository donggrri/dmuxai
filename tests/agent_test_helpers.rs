// Requirement/TC traceability: docs/specs/SPEC-agent-test-helpers.md
// FR-510..FR-514 / TC-510: AGENTS.md documents submit_prompt and test helpers.
// FR-515 / TC-512: app.rs test module doc lists the same helpers.

use std::fs;

#[test]
fn tc_510_agents_md_documents_agent_test_helpers() {
    let agents = fs::read_to_string("AGENTS.md").expect("AGENTS.md readable");
    assert!(
        agents.contains("## Agent test helpers"),
        "AGENTS.md must have an Agent test helpers section (FR-510)"
    );
    assert!(
        agents.contains("submit_prompt"),
        "AGENTS.md must document submit_prompt (FR-511)"
    );
    assert!(
        agents.contains("test_app()"),
        "AGENTS.md must document test_app() (FR-512)"
    );
    assert!(
        agents.contains("test_app_with_engine"),
        "AGENTS.md must document test_app_with_engine (FR-513)"
    );
    assert!(
        agents.contains("test_app_runtime"),
        "AGENTS.md must document test_app_runtime (FR-514)"
    );
    assert!(
        agents.contains("no `App::handle_enter` in production"),
        "AGENTS.md must warn that handle_enter is not production API"
    );
}

#[test]
fn tc_512_app_tests_module_doc_lists_helpers() {
    let app_rs = fs::read_to_string("src/app.rs").expect("src/app.rs readable");
    let app_rs = app_rs.replace("\r\n", "\n");
    let marker = "#[cfg(test)]\nmod tests {";
    let start = app_rs.find(marker).expect("app tests module present") + marker.len();
    let rest = &app_rs[start..];
    let doc_end = rest
        .find("\n    use std::sync")
        .expect("tests module imports after doc comment");
    let doc = &rest[..doc_end];
    assert!(
        doc.contains("submit_prompt"),
        "tests module doc must mention submit_prompt (FR-515)"
    );
    assert!(
        doc.contains("test_app"),
        "tests module doc must mention test_app (FR-515)"
    );
    assert!(
        doc.contains("AGENTS.md"),
        "tests module doc must point to AGENTS.md (FR-515)"
    );
}

// TC-513 / FR-519: SPEC-agent-test-helpers has Status line in first 10 lines
#[test]
fn tc_513_spec_agent_test_helpers_has_status_line() {
    let spec = fs::read_to_string("docs/specs/SPEC-agent-test-helpers.md")
        .expect("SPEC-agent-test-helpers.md readable");
    let status_found = spec.lines().take(10).any(|line| {
        line.trim_start()
            .to_ascii_lowercase()
            .starts_with("status:")
    });
    assert!(
        status_found,
        "SPEC-agent-test-helpers.md must have a Status line in the first 10 lines"
    );
}

// TC-514 / FR-519: IDs 510–519 appear only in SPEC-agent-test-helpers
#[test]
fn tc_514_spec_ids_510_to_519_are_unique_to_agent_test_helpers() {
    let row_id = regex::Regex::new(r"^\|[[:space:]]*((FR|NFR|UX|INT|TC)-([0-9]+))[[:space:]]*\|")
        .expect("valid ID regex");
    for entry in fs::read_dir("docs/specs").expect("docs/specs readable") {
        let path = entry.expect("valid entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let file_name = path.file_name().unwrap().to_string_lossy();
        let contents = fs::read_to_string(&path).expect("spec readable");
        for line in contents.lines() {
            if let Some(caps) = row_id.captures(line) {
                let id_num: u32 = caps[3].parse().unwrap();
                if (510..=519).contains(&id_num) {
                    assert_eq!(
                        file_name, "SPEC-agent-test-helpers.md",
                        "ID {} found in {} but must only appear in SPEC-agent-test-helpers.md",
                        &caps[1], file_name
                    );
                }
            }
        }
    }
}

// TC-515 / FR-519: spec-index lists SPEC-agent-test-helpers
#[test]
fn tc_515_spec_index_lists_agent_test_helpers() {
    let output = std::process::Command::new("bash")
        .arg("scripts/spec-index.sh")
        .output()
        .expect("run scripts/spec-index.sh");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[SPEC-agent-test-helpers.md](docs/specs/SPEC-agent-test-helpers.md)"),
        "spec-index.sh must list SPEC-agent-test-helpers.md"
    );
}

// TC-517 / FR-517: dogfood #23 marked fixed
#[test]
fn tc_517_dogfood_23_marked_fixed() {
    let dogfood =
        fs::read_to_string("docs/dogfood/README.md").expect("docs/dogfood/README.md readable");
    assert!(
        dogfood.contains("## #23 AGY repeatedly invents `App::handle_enter` in tests — fixed"),
        "dogfood #23 must be marked fixed"
    );
}

// TC-518 / FR-518: TEST-PLAN has rows for SPEC-agent-test-helpers
#[test]
fn tc_518_test_plan_has_agent_test_helpers_rows() {
    let test_plan = fs::read_to_string("docs/TEST-PLAN.md").expect("docs/TEST-PLAN.md readable");
    let rows: Vec<_> = test_plan
        .lines()
        .filter(|line| line.contains("SPEC-agent-test-helpers"))
        .collect();
    assert!(
        rows.len() >= 5,
        "TEST-PLAN must contain rows for SPEC-agent-test-helpers, found {}",
        rows.len()
    );
}

// TC-519 / FR-510: spec-index includes FR-510..519 range
#[test]
fn tc_519_spec_index_includes_helper_id_ranges() {
    let output = std::process::Command::new("bash")
        .arg("scripts/spec-index.sh")
        .output()
        .expect("run scripts/spec-index.sh");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let row = stdout
        .lines()
        .find(|line| line.contains("SPEC-agent-test-helpers.md"))
        .expect("SPEC-agent-test-helpers row in spec-index");
    assert!(
        row.contains("FR-510..519") && row.contains("TC-510..519"),
        "spec-index row must list FR-510..519 and TC-510..519 ranges: {row}"
    );
}
