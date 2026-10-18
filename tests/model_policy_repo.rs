use std::fs;
use std::path::Path;

use dmuxai::model_policy::{ALLOWED_MODEL, validate_model_id};

#[test]
fn graph_agent_definitions_use_composer_only() {
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
fn dmuxai_graph_yaml_keeps_grok_disallow_and_explains_why() {
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
