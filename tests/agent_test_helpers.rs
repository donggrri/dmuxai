// Requirement/TC traceability: docs/specs/SPEC-agent-test-helpers.md
// FR-510..FR-514 / TC-510: AGENTS.md documents submit_prompt and test helpers.
// FR-515 / TC-512: app.rs test module doc lists the same helpers.

use std::fs;

#[test]
fn agents_md_documents_agent_test_helpers() {
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
fn app_tests_module_doc_lists_helpers() {
    let app_rs = fs::read_to_string("src/app.rs").expect("src/app.rs readable");
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
