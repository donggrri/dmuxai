use std::fs;
use std::path::Path;

use dmuxai::model_policy::{
    IMPLEMENT_MODEL, ORCHESTRATOR_MODEL, RESEARCH_MODEL, validate_model_id,
};

#[test]
fn graph_agent_definitions_declare_allowed_models() {
    let agents_dir = Path::new(".cursor/agents");
    for entry in fs::read_dir(agents_dir).expect("graph agents directory") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let contents = fs::read_to_string(&path).expect("agent file readable");
        for line in contents.lines() {
            if let Some(model) = line.strip_prefix("model:") {
                validate_model_id(model.trim()).expect("graph agent model must be policy-allowed");
            }
        }
    }
}

#[test]
fn dmuxai_graph_yaml_declares_multi_model_policy() {
    let yaml = fs::read_to_string("docs/dmuxai-graph.yaml").expect("graph yaml");
    assert!(yaml.contains(ORCHESTRATOR_MODEL));
    assert!(yaml.contains(RESEARCH_MODEL));
    assert!(yaml.contains(IMPLEMENT_MODEL));
}

#[test]
fn mission_doc_states_graph_first_direction() {
    let mission = fs::read_to_string("MISSION.md").expect("MISSION.md");
    assert!(mission.contains("Graph-first"));
    assert!(mission.contains("cursor-agent"));
    assert!(mission.contains("grok-4.7-medium"));
}
