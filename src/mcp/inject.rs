//! MCP injection seam with per-runner adapters.
//!
//! Provides [`RunnerMcp`], the single interface delivering resolved MCP servers
//! to each runner adapter for a turn. Runner applicability filtering is performed
//! exactly once via [`RunnerMcp::for_runner`].
//!
//! See docs/specs/SPEC-mcp-injection.md (FR-1980..FR-1984, TC-1980..TC-1984).

use std::collections::BTreeMap;

use super::config::ResolvedMcpServer;
use super::propagate::{
    acp_mcp_servers, agy_mcp_notice, codex_config_overrides_and_env, custom_mcp_json,
    merge_opencode_mcp,
};

/// Notice emitted once per adapter session when MCP servers apply to Cursor in legacy mode.
pub const CURSOR_LEGACY_MCP_NOTICE: &str = "MCP servers are not propagated to Cursor in legacy mode (DmuxAI does not write .cursor/mcp.json into your workspace). Set `[cursor] protocol = \"acp\"` to pass them per session, or add them to ~/.cursor/mcp.json yourself.";

/// MCP servers that apply to one runner, filtered exactly once.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunnerMcp {
    runner: String,
    servers: Vec<ResolvedMcpServer>,
}

impl RunnerMcp {
    /// Constructs a `RunnerMcp` by filtering all resolved servers for `runner`.
    /// This is the designated `applies_to` evaluation seam (FR-1980).
    pub fn for_runner(all: &[ResolvedMcpServer], runner: &str) -> Self {
        let servers = all
            .iter()
            .filter(|s| s.applies_to(runner))
            .cloned()
            .collect();
        Self {
            runner: runner.to_owned(),
            servers,
        }
    }

    /// The runner identifier this instance was filtered for.
    pub fn runner(&self) -> &str {
        &self.runner
    }

    /// The filtered resolved MCP servers applicable to this runner.
    pub fn servers(&self) -> &[ResolvedMcpServer] {
        &self.servers
    }

    /// Returns `true` if no MCP servers apply to this runner.
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// Adapts the filtered servers into OpenCode configuration JSON.
    pub fn opencode_config(&self, base_json: &str) -> (String, bool) {
        merge_opencode_mcp(base_json, &self.servers)
    }

    /// Adapts the filtered servers into Codex config overrides, child environment, and notices.
    pub fn codex_overrides(&self) -> (Vec<String>, BTreeMap<String, String>, Vec<String>) {
        codex_config_overrides_and_env(&self.servers)
    }

    /// Adapts the filtered servers into Claude/Cursor-style `{"mcpServers":{...}}` JSON.
    pub fn custom_json(&self) -> String {
        custom_mcp_json(&self.servers)
    }

    /// Adapts the filtered servers into the ACP `mcpServers` JSON array.
    pub fn acp_servers(&self) -> serde_json::Value {
        acp_mcp_servers(&self.servers)
    }

    /// Returns the AGY CLI instructions notice, or `None` if no servers apply.
    pub fn agy_notice(&self) -> Option<String> {
        agy_mcp_notice(&self.servers)
    }

    /// Returns the Cursor legacy notice when servers are configured, or `None` if empty.
    pub fn cursor_legacy_notice(&self) -> Option<String> {
        if self.servers.is_empty() {
            None
        } else {
            Some(CURSOR_LEGACY_MCP_NOTICE.to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::config::{ServerKind, ToolNames, Transport};

    fn make_stdio(name: &str, command: &str, runners: Option<Vec<&str>>) -> ResolvedMcpServer {
        let mut env = BTreeMap::new();
        env.insert("KEY".to_owned(), "value".to_owned());
        ResolvedMcpServer {
            name: name.to_owned(),
            kind: ServerKind::Generic,
            transport: Transport::Stdio {
                command: command.to_owned(),
                args: vec!["--flag".to_owned(), "arg1".to_owned()],
                env,
            },
            runners: runners.map(|r| r.into_iter().map(str::to_owned).collect()),
            tools: ToolNames::default(),
        }
    }

    fn make_http(name: &str, url: &str, runners: Option<Vec<&str>>) -> ResolvedMcpServer {
        let mut headers = BTreeMap::new();
        headers.insert("Authorization".to_owned(), "Bearer test".to_owned());
        ResolvedMcpServer {
            name: name.to_owned(),
            kind: ServerKind::Jira,
            transport: Transport::Http {
                url: url.to_owned(),
                headers,
            },
            runners: runners.map(|r| r.into_iter().map(str::to_owned).collect()),
            tools: ToolNames::default(),
        }
    }

    #[test]
    fn tc_1980_for_runner_filters_once() {
        let s_all = make_stdio("all_runners", "tool-all", None);
        let s_codex = make_stdio("codex_only", "tool-codex", Some(vec!["codex"]));
        let s_cursor = make_stdio("cursor_only", "tool-cursor", Some(vec!["cursor"]));
        let s_multi = make_http(
            "multi",
            "https://api.example.invalid/mcp",
            Some(vec![" CoDeX ", "custom-agent "]),
        );

        let all = vec![
            s_all.clone(),
            s_codex.clone(),
            s_cursor.clone(),
            s_multi.clone(),
        ];

        // 1. Filtering for "codex"
        let codex_mcp = RunnerMcp::for_runner(&all, "codex");
        assert_eq!(codex_mcp.runner(), "codex");
        assert!(!codex_mcp.is_empty());
        let codex_names: Vec<&str> = codex_mcp
            .servers()
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(codex_names, vec!["all_runners", "codex_only", "multi"]);

        // Case and whitespace tolerance for runner query
        let codex_upper = RunnerMcp::for_runner(&all, "  CODEX  ");
        assert_eq!(codex_upper.servers(), codex_mcp.servers());

        // 2. Filtering for "cursor"
        let cursor_mcp = RunnerMcp::for_runner(&all, "cursor");
        assert_eq!(cursor_mcp.runner(), "cursor");
        assert!(!cursor_mcp.is_empty());
        let cursor_names: Vec<&str> = cursor_mcp
            .servers()
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(cursor_names, vec!["all_runners", "cursor_only"]);

        // 3. Filtering for "custom-agent"
        let custom_mcp = RunnerMcp::for_runner(&all, "custom-agent");
        let custom_names: Vec<&str> = custom_mcp
            .servers()
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(custom_names, vec!["all_runners", "multi"]);

        // 4. Filtering for an unmatched runner only receives runner=None servers
        let other_mcp = RunnerMcp::for_runner(&all, "other");
        assert_eq!(other_mcp.servers().len(), 1);
        assert_eq!(other_mcp.servers()[0].name, "all_runners");

        // 5. Completely empty input
        let empty_mcp = RunnerMcp::for_runner(&[], "codex");
        assert_eq!(empty_mcp.runner(), "codex");
        assert!(empty_mcp.is_empty());
        assert_eq!(empty_mcp.servers().len(), 0);

        // 6. Non-empty input with no matching servers
        let cursor_only_list = vec![s_cursor];
        let no_match = RunnerMcp::for_runner(&cursor_only_list, "codex");
        assert!(no_match.is_empty());
    }

    #[test]
    fn tc_1981_adapter_outputs_equal_propagate_builders() {
        let s_stdio = make_stdio("stdio_srv", "run-tool", None);
        let s_http = make_http("http_srv", "https://example.invalid/mcp", None);
        let servers = vec![s_stdio, s_http];

        let mcp = RunnerMcp::for_runner(&servers, "codex");
        let base_json = r#"{"permission":{"edit":"allow"}}"#;

        // 1. opencode_config vs merge_opencode_mcp
        let mcp_opencode = mcp.opencode_config(base_json);
        let direct_opencode = merge_opencode_mcp(base_json, mcp.servers());
        assert_eq!(mcp_opencode, direct_opencode);

        // 2. codex_overrides vs codex_config_overrides_and_env
        let mcp_codex = mcp.codex_overrides();
        let direct_codex = codex_config_overrides_and_env(mcp.servers());
        assert_eq!(mcp_codex, direct_codex);

        // 3. custom_json vs custom_mcp_json
        let mcp_custom = mcp.custom_json();
        let direct_custom = custom_mcp_json(mcp.servers());
        assert_eq!(mcp_custom, direct_custom);

        // 4. acp_servers vs acp_mcp_servers
        let mcp_acp = mcp.acp_servers();
        let direct_acp = acp_mcp_servers(mcp.servers());
        assert_eq!(mcp_acp, direct_acp);

        // 5. agy_notice vs agy_mcp_notice
        let mcp_agy = mcp.agy_notice();
        let direct_agy = agy_mcp_notice(mcp.servers());
        assert_eq!(mcp_agy, direct_agy);
        assert!(mcp_agy.is_some());

        // 6. cursor_legacy_notice
        assert_eq!(
            mcp.cursor_legacy_notice(),
            Some(CURSOR_LEGACY_MCP_NOTICE.to_owned())
        );

        // 7. Verify empty server behavior across all adapters
        let empty_mcp = RunnerMcp::for_runner(&[], "codex");
        assert_eq!(
            empty_mcp.opencode_config(base_json),
            merge_opencode_mcp(base_json, &[])
        );
        assert_eq!(
            empty_mcp.codex_overrides(),
            codex_config_overrides_and_env(&[])
        );
        assert_eq!(empty_mcp.custom_json(), custom_mcp_json(&[]));
        assert_eq!(empty_mcp.acp_servers(), acp_mcp_servers(&[]));
        assert_eq!(empty_mcp.agy_notice(), None);
        assert_eq!(empty_mcp.agy_notice(), agy_mcp_notice(&[]));
        assert_eq!(empty_mcp.cursor_legacy_notice(), None);
    }

    #[test]
    fn tc_1984_no_applies_to_outside_mcp() {
        use std::fs;
        use std::path::Path;

        fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        walk(&path, out);
                    } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                        out.push(path);
                    }
                }
            }
        }

        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let src = manifest.join("src");
        let mcp = src.join("mcp");
        let mut files = Vec::new();
        walk(&src, &mut files);

        let mut offenders = Vec::new();
        for file in files {
            if file.starts_with(&mcp) {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&file)
                && content.contains(".applies_to(")
            {
                offenders.push(
                    file.strip_prefix(manifest)
                        .unwrap_or(&file)
                        .display()
                        .to_string(),
                );
            }
        }
        assert!(
            offenders.is_empty(),
            "applies_to calls found outside src/mcp/: {offenders:?}"
        );
    }
}
