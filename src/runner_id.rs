//! Canonical runner ids from user-facing keywords (SPEC-cursor-agent-keyword FR-3200).

/// Canonical id for the built-in Cursor runner (FR-3207).
pub const CURSOR_RUNNER_ID: &str = "cursor-agent";

/// Normalizes an agent/runner id at a parse boundary: trim, ASCII-lowercase,
/// and map the legacy alias `cursor` to [`CURSOR_RUNNER_ID`].
pub fn canonical_runner_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == "cursor" || lower == "cursor-agent" {
        CURSOR_RUNNER_ID.to_owned()
    } else {
        lower
    }
}

/// Built-in runner keywords for unknown-agent messages (FR-3201).
pub fn builtin_runner_keywords_display() -> &'static str {
    "cursor-agent, agy, codex"
}

pub fn format_unknown_agent_message(agent: &str) -> String {
    format!(
        "unknown agent '{agent}' (known built-in: {})",
        builtin_runner_keywords_display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // TC-3200 / FR-3200
    #[test]
    fn tc_3200_canonical_runner_id_maps_cursor_agent() {
        assert_eq!(canonical_runner_id("cursor-agent"), CURSOR_RUNNER_ID);
        assert_eq!(canonical_runner_id("CURSOR-AGENT"), CURSOR_RUNNER_ID);
        assert_eq!(canonical_runner_id(" cursor-agent "), CURSOR_RUNNER_ID);
        assert_eq!(canonical_runner_id("cursor"), CURSOR_RUNNER_ID);
        assert_eq!(canonical_runner_id("CURSOR"), CURSOR_RUNNER_ID);
        assert_eq!(canonical_runner_id(" cursor "), CURSOR_RUNNER_ID);
        assert_eq!(canonical_runner_id("agy"), "agy");
        assert_eq!(canonical_runner_id("opencode-glm"), "opencode-glm");
        assert_eq!(canonical_runner_id("cursor-agentx"), "cursor-agentx");
        assert_eq!(canonical_runner_id(""), "");
        assert_eq!(canonical_runner_id("   "), "");
    }
}
