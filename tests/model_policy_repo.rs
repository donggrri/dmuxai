use std::fs;
use std::path::Path;

use dmuxai::model_policy::{ALLOWED_MODEL, validate_model_id};

#[test]
fn tc_310_graph_agent_definitions_use_composer_only() {
    let agents_dir = Path::new(".cursor/agents");
    for entry in fs::read_dir(agents_dir).expect("graph agents directory") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let contents = fs::read_to_string(&path).expect("agent file readable");
        for line in contents.lines() {
            if let Some(model) = line.strip_prefix("model:") {
                validate_model_id(model.trim()).expect("graph agent model must be composer-2.5");
            }
        }
    }
}

#[test]
fn dmuxai_graph_yaml_declares_composer_only_policy() {
    let yaml = fs::read_to_string("docs/dmuxai-graph.yaml").expect("graph yaml");
    assert!(
        yaml.contains("allowed_only: composer-2.5"),
        "graph yaml must pin allowed_only model"
    );
    assert!(
        !yaml.contains("model: grok"),
        "graph node models must not reference grok"
    );
    assert!(
        !yaml.contains("model: claude-haiku"),
        "graph node models must not reference haiku"
    );
}

// TC-912 / FR-912
#[test]
fn tc_912_dmuxai_graph_yaml_keeps_grok_disallow_and_explains_why() {
    let yaml = fs::read_to_string("docs/dmuxai-graph.yaml").expect("graph yaml");
    assert!(yaml.contains("allowed_only: composer-2.5"));
    assert!(yaml.contains("*-fast"));
    assert!(yaml.contains("*grok*"));
    assert!(!yaml.contains("model: grok"));
    let disallow_at = yaml.find("disallow_patterns:").expect("disallow_patterns");
    let before = yaml[..disallow_at].trim_end();
    assert!(
        before.ends_with("*grok* stays here so this file does not assign a grok model."),
        "comment must sit immediately above disallow_patterns:\n{before}"
    );
    assert!(before.contains("composer-2.5"));
    assert!(before.contains("grok runner"));
}

#[test]
fn mission_doc_states_composer_policy() {
    let mission = fs::read_to_string("MISSION.md").expect("MISSION.md");
    assert!(mission.contains(ALLOWED_MODEL));
    assert!(mission.contains("cursor-agent"));
}

// TC-3208 / UX-3200
#[test]
fn tc_3208_docs_name_cursor_agent_keyword() {
    let readme = fs::read_to_string("README.md").expect("README.md");
    assert!(readme.contains("cursor-agent"));
    assert!(
        readme.contains("still accepted") || readme.contains("alias"),
        "README should note that cursor is still accepted"
    );
    let api = fs::read_to_string("docs/API.md").expect("docs/API.md");
    assert!(api.contains("cursor-agent"));
    assert!(api.contains("alias"));
    let changelog = fs::read_to_string("CHANGELOG.md").expect("CHANGELOG.md");
    assert!(changelog.contains("cursor-agent"));
    let cli_rs = fs::read_to_string("src/cli.rs").expect("src/cli.rs");
    for usage in ["RUN_USAGE", "MODELS_USAGE", "RUNNERS_USAGE"] {
        let marker = format!("pub const {usage}");
        let block = cli_rs
            .split(&marker)
            .nth(1)
            .and_then(|rest| rest.split("\");").next())
            .unwrap_or_else(|| panic!("{usage} not found"));
        assert!(
            block.contains("cursor-agent"),
            "{usage} must name cursor-agent"
        );
    }
}
