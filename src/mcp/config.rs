//! Shared MCP server configuration: the parsed `[[mcp.servers]]` specs, the
//! name-keyed merge used by [`crate::settings::DmuxSettings`], and the resolved
//! runtime form with env references expanded.
//!
//! Secrets never appear in [`std::fmt::Debug`] output or in resolve warnings.
//! See docs/specs/SPEC-web-dashboard.md (INT-560, TC-560) and the PLAN "Slice C
//! contract".

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

/// What an MCP server is used for. `generic` is the default and is neither
/// listed nor importable by `POST /api/import`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ServerKind {
    #[default]
    Generic,
    Jira,
    Confluence,
}

impl ServerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ServerKind::Generic => "generic",
            ServerKind::Jira => "jira",
            ServerKind::Confluence => "confluence",
        }
    }
}

/// A secret value: either a literal string or a `{ env = "VAR" }` reference
/// resolved at runtime. Never logged or serialized.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum SecretValue {
    Literal(String),
    Env { env: String },
}

impl SecretValue {
    /// The referenced environment variable name, if this is an env reference.
    pub fn env_var(&self) -> Option<&str> {
        match self {
            SecretValue::Literal(_) => None,
            SecretValue::Env { env } => Some(env),
        }
    }

    fn resolve(&self, env_lookup: &impl Fn(&str) -> Option<String>) -> Result<String, String> {
        match self {
            SecretValue::Literal(value) => Ok(value.clone()),
            SecretValue::Env { env } => env_lookup(env).ok_or_else(|| env.clone()),
        }
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretValue::Literal(_) => f.write_str("Literal(***)"),
            SecretValue::Env { env } => f.debug_tuple("Env").field(env).finish(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_search_tool() -> String {
    "search".to_owned()
}

fn default_get_tool() -> String {
    "get".to_owned()
}

/// Tool names used by `POST /api/import` for Jira/Confluence servers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ToolNames {
    #[serde(default = "default_search_tool")]
    pub search: String,
    #[serde(default = "default_get_tool")]
    pub get: String,
}

impl Default for ToolNames {
    fn default() -> Self {
        Self {
            search: default_search_tool(),
            get: default_get_tool(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct LegacyRunnersPresent;

impl<'de> Deserialize<'de> for LegacyRunnersPresent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        serde::de::IgnoredAny::deserialize(deserializer)?;
        Ok(LegacyRunnersPresent)
    }
}

/// One `[[mcp.servers]]` entry as written in user or repo TOML before merging.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct McpServerSpec {
    pub name: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, SecretValue>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, SecretValue>,
    #[serde(default)]
    pub kind: ServerKind,
    #[serde(default, rename = "runners")]
    pub(crate) legacy_runners: Option<LegacyRunnersPresent>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub tools: ToolNames,
}

fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl McpServerSpec {
    /// Rejects entries that cannot be resolved (bad name, both or neither of
    /// `command`/`url`). The returned message never contains a secret value.
    pub fn validate(&self) -> Result<(), String> {
        if !valid_server_name(&self.name) {
            return Err(format!(
                "mcp server name '{}' is invalid (expected [A-Za-z0-9_-]{{1,64}})",
                self.name
            ));
        }
        if !self.enabled {
            return Ok(());
        }
        let has_command = self
            .command
            .as_deref()
            .is_some_and(|command| !command.trim().is_empty());
        let has_url = self
            .url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty());
        match (has_command, has_url) {
            (true, true) => Err(format!(
                "mcp server '{}' sets both command and url; ignored",
                self.name
            )),
            (false, false) => Err(format!(
                "mcp server '{}' needs either command or url; ignored",
                self.name
            )),
            _ => Ok(()),
        }
    }
}

impl fmt::Debug for McpServerSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let env = redacted_secrets(&self.env);
        let headers = redacted_secrets(&self.headers);
        f.debug_struct("McpServerSpec")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("args", &self.args)
            .field("env", &env)
            .field("url", &self.url.as_deref().map(redact_url))
            .field("headers", &headers)
            .field("kind", &self.kind)
            .field("enabled", &self.enabled)
            .field("tools", &self.tools)
            .finish()
    }
}

fn redacted_secrets(map: &BTreeMap<String, SecretValue>) -> BTreeMap<String, String> {
    map.iter()
        .map(|(key, value)| {
            let shown = match value {
                SecretValue::Literal(_) => "***".to_owned(),
                SecretValue::Env { env } => format!("${env}"),
            };
            (key.clone(), shown)
        })
        .collect()
}

/// Strips any query string or fragment from a URL so Debug output and notices
/// can never leak a query-string credential (INT-560, NFR-560).
pub(crate) fn redact_url(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// The runtime form of a server with env/header references already expanded.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedMcpServer {
    pub name: String,
    pub kind: ServerKind,
    pub transport: Transport,
    pub tools: ToolNames,
}

impl fmt::Debug for ResolvedMcpServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedMcpServer")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("transport", &self.transport)
            .field("tools", &self.tools)
            .finish()
    }
}

/// The resolved transport. Values are redacted in `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub enum Transport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

impl fmt::Debug for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Transport::Stdio { command, args, env } => f
                .debug_struct("Stdio")
                .field("command", command)
                .field("args", args)
                .field("env", &RedactedValues(env))
                .finish(),
            Transport::Http { url, headers } => f
                .debug_struct("Http")
                .field("url", &redact_url(url))
                .field("headers", &RedactedValues(headers))
                .finish(),
        }
    }
}

struct RedactedValues<'a>(&'a BTreeMap<String, String>);

impl fmt::Debug for RedactedValues<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.0.keys().map(|key| (key, "***")))
            .finish()
    }
}

/// Expands env references and builds the runtime server list. A missing env var
/// disables that one server with a warning that names the variable but never
/// leaks a value. Disabled specs are skipped silently.
pub fn resolve_servers(
    specs: &[McpServerSpec],
    env_lookup: impl Fn(&str) -> Option<String>,
) -> (Vec<ResolvedMcpServer>, Vec<String>) {
    let mut resolved = Vec::new();
    let mut warnings = Vec::new();
    for spec in specs {
        if !spec.enabled {
            continue;
        }
        if let Err(error) = spec.validate() {
            warnings.push(error);
            continue;
        }

        let mut missing: Option<String> = None;
        let transport = if spec
            .command
            .as_deref()
            .is_some_and(|command| !command.trim().is_empty())
        {
            let mut env = BTreeMap::new();
            for (key, value) in &spec.env {
                match value.resolve(&env_lookup) {
                    Ok(item) => {
                        env.insert(key.clone(), item);
                    }
                    Err(var) => {
                        missing = Some(var);
                        break;
                    }
                }
            }
            missing.is_none().then(|| Transport::Stdio {
                command: spec
                    .command
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .to_owned(),
                args: spec.args.clone(),
                env,
            })
        } else {
            let mut headers = BTreeMap::new();
            for (key, value) in &spec.headers {
                match value.resolve(&env_lookup) {
                    Ok(item) => {
                        headers.insert(key.clone(), item);
                    }
                    Err(var) => {
                        missing = Some(var);
                        break;
                    }
                }
            }
            missing.is_none().then(|| Transport::Http {
                url: spec.url.as_deref().unwrap_or_default().trim().to_owned(),
                headers,
            })
        };

        if let Some(var) = missing {
            warnings.push(format!(
                "mcp server {} disabled: env var {} is not set",
                spec.name, var
            ));
            continue;
        }
        let Some(transport) = transport else {
            continue;
        };
        resolved.push(ResolvedMcpServer {
            name: spec.name.clone(),
            kind: spec.kind,
            transport,
            tools: spec.tools.clone(),
        });
    }
    (resolved, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DmuxSettings;

    fn write(path: &std::path::Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn mcp_config_merges_user_and_repo_by_name() {
        // TC-560, INT-560: repo overrides user by name; enabled=false removes;
        // env refs resolve; a missing env var disables with a value-free
        // warning; Debug output contains no secret.
        let base = std::env::temp_dir().join(format!(
            "dmuxai-mcp-merge-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let user_dir = base.join("user");
        let ws_dir = base.join("workspace");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&ws_dir).unwrap();

        let user_toml = user_dir.join("dmuxai.toml");
        write(
            &user_toml,
            r#"
[[mcp.servers]]
name = "demo"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-everything"]
env = { API_TOKEN = { env = "DEMO_TOKEN" }, MODE = "demo" }
runners = ["codex", "opencode"]

[[mcp.servers]]
name = "shared"
command = "old-command"

[[mcp.servers]]
name = "gone"
command = "goner"

[[mcp.servers]]
name = "needs-env"
command = "npx"
env = { TOKEN = { env = "MISSING_VAR" } }

[[mcp.servers]]
name = "literal"
command = "npx"
env = { TOKEN = "demo-token" }

[[mcp.servers]]
name = "broken"
command = "a"
url = "https://mcp.example.invalid/mcp"
"#,
        );

        let ws_toml = ws_dir.join("dmuxai.toml");
        write(
            &ws_toml,
            r#"
[[mcp.servers]]
name = "shared"
command = "new-command"

[[mcp.servers]]
name = "gone"
enabled = false
"#,
        );

        let settings = DmuxSettings::load_with_user_config(&ws_dir, Some(&user_toml));
        let names: Vec<&str> = settings
            .mcp_servers
            .iter()
            .map(|spec| spec.name.as_str())
            .collect();
        assert!(names.contains(&"demo"));
        assert!(names.contains(&"shared"));
        assert!(names.contains(&"needs-env"));
        assert!(names.contains(&"literal"));
        assert!(!names.contains(&"gone"), "enabled=false must remove");
        assert!(!names.contains(&"broken"), "invalid entry must be skipped");
        let shared = settings
            .mcp_servers
            .iter()
            .find(|spec| spec.name == "shared")
            .unwrap();
        assert_eq!(shared.command.as_deref(), Some("new-command"));
        assert!(
            settings
                .config_warnings
                .iter()
                .any(|warning| warning.contains("broken")),
            "invalid entry must warn: {:?}",
            settings.config_warnings
        );

        let (resolved, warnings) = resolve_servers(&settings.mcp_servers, |key| match key {
            "DEMO_TOKEN" => Some("demo-token".to_owned()),
            _ => None,
        });

        let demo = resolved
            .iter()
            .find(|server| server.name == "demo")
            .unwrap();
        assert_eq!(demo.kind, ServerKind::Generic);
        match &demo.transport {
            Transport::Stdio { command, args, env } => {
                assert_eq!(command, "npx");
                assert_eq!(args, &["-y", "@modelcontextprotocol/server-everything"]);
                assert_eq!(env.get("API_TOKEN").map(String::as_str), Some("demo-token"));
                assert_eq!(env.get("MODE").map(String::as_str), Some("demo"));
            }
            other => panic!("expected stdio transport, got {other:?}"),
        }

        assert!(
            resolved.iter().all(|server| server.name != "needs-env"),
            "missing env var must disable the server"
        );
        let warning = warnings
            .iter()
            .find(|warning| warning.contains("needs-env"))
            .expect("missing env warning");
        assert!(warning.contains("MISSING_VAR"));
        assert!(warning.contains("is not set"));
        assert!(!warning.contains("demo-token"));

        // Debug of resolved servers and of the settings must not leak secrets.
        let resolved_debug = format!("{resolved:?}");
        assert!(!resolved_debug.contains("demo-token"));
        assert!(resolved_debug.contains("***"));
        let settings_debug = format!("{settings:?}");
        assert!(!settings_debug.contains("demo-token"));

        let _ = std::fs::remove_dir_all(&base);
    }

    // Finding 4 + 11: Debug strips a URL query/fragment, and resolve_servers
    // trims surrounding whitespace from command/url.
    #[test]
    fn debug_redacts_url_query_and_resolution_trims() {
        let specs = vec![
            McpServerSpec {
                name: "stdio".to_owned(),
                command: Some("  npx  ".to_owned()),
                args: vec!["-y".to_owned()],
                env: BTreeMap::new(),
                url: None,
                headers: BTreeMap::new(),
                kind: ServerKind::Generic,
                legacy_runners: None,
                enabled: true,
                tools: ToolNames::default(),
            },
            McpServerSpec {
                name: "jira".to_owned(),
                command: None,
                args: Vec::new(),
                env: BTreeMap::new(),
                url: Some(
                    "  https://mcp.example.invalid/mcp?api-key=demo-query-secret#frag  ".to_owned(),
                ),
                headers: BTreeMap::new(),
                kind: ServerKind::Jira,
                legacy_runners: None,
                enabled: true,
                tools: ToolNames::default(),
            },
        ];
        let (resolved, warnings) = resolve_servers(&specs, |_| None);
        assert!(warnings.is_empty());

        match &resolved[0].transport {
            Transport::Stdio { command, .. } => assert_eq!(command, "npx", "command trimmed"),
            other => panic!("expected stdio, got {other:?}"),
        }
        match &resolved[1].transport {
            Transport::Http { url, .. } => assert_eq!(
                url, "https://mcp.example.invalid/mcp?api-key=demo-query-secret#frag",
                "url trimmed, query kept in the runtime value"
            ),
            other => panic!("expected http, got {other:?}"),
        }

        // Debug output never carries the query-string credential.
        let transport_debug = format!("{:?}", resolved[1].transport);
        assert!(transport_debug.contains("https://mcp.example.invalid/mcp"));
        assert!(!transport_debug.contains("demo-query-secret"));
        assert!(!transport_debug.contains("api-key"));
        let server_debug = format!("{:?}", resolved[1]);
        assert!(!server_debug.contains("demo-query-secret"));
        let spec_debug = format!("{:?}", specs[1]);
        assert!(spec_debug.contains("https://mcp.example.invalid/mcp"));
        assert!(!spec_debug.contains("demo-query-secret"));
        assert!(!spec_debug.contains("api-key"));

        assert_eq!(redact_url("https://h/mcp?token=x#y"), "https://h/mcp");
        assert_eq!(redact_url("https://h/mcp"), "https://h/mcp");
    }

    // TC-2085 / FR-2085: source scan of src/**/*.rs asserting no propagation code remains.
    #[test]
    fn tc_2085_no_propagation_code_left() {
        use std::path::{Path, PathBuf};

        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let src_dir = manifest_dir.join("src");
        let patterns = [
            format!("Runner{}", "Mcp"),
            format!("mcp::{}", "propagate"),
            format!("mcp::{}", "inject"),
            format!("applies_{}(", "to"),
            format!(".cursor/{}", "mcp.json"),
        ];

        let mut offenders = Vec::new();
        fn scan_dir(
            dir: &Path,
            patterns: &[String],
            offenders: &mut Vec<(PathBuf, usize, String)>,
        ) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    scan_dir(&path, patterns, offenders);
                } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                    let content = std::fs::read_to_string(&path).unwrap();
                    for (line_no, line) in content.lines().enumerate() {
                        if line.contains("tc_2085_no_propagation_code_left")
                            || line.contains("patterns = [")
                        {
                            continue;
                        }
                        for pattern in patterns {
                            if line.contains(pattern) {
                                offenders.push((path.clone(), line_no + 1, line.trim().to_owned()));
                            }
                        }
                    }
                }
            }
        }

        scan_dir(&src_dir, &patterns, &mut offenders);
        assert!(
            offenders.is_empty(),
            "propagation code found in src/: {offenders:#?}"
        );
    }
}
