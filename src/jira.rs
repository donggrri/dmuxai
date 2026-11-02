//! Read-only Jira Cloud search client (SPEC-jira-todos FR-2402..FR-2405).
//!
//! The client only ever issues GET requests. The API token is read from the
//! environment at fetch time (or injected by tests) and is never stored in a
//! response, error, log, or `Debug` representation.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::settings::JiraSettings;

const SEARCH_PATH: &str = "/rest/api/3/search/jql";
const LEGACY_SEARCH_PATH: &str = "/rest/api/3/search";
const SEARCH_FIELDS: &str = "summary,status,priority,issuetype,updated";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(60);

const MSG_NOT_CONFIGURED: &str = "Jira is not configured";
const MSG_MISSING_TOKEN: &str = "Jira token environment variable is not set";
const MSG_UNAUTHORIZED: &str = "Jira rejected the credentials (401/403)";
const MSG_NETWORK: &str = "Could not reach Jira";
const MSG_BAD_RESPONSE: &str = "Jira returned an unexpected response";

/// Stable error classification for the dashboard (serialized snake_case).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JiraErrorKind {
    NotConfigured,
    MissingToken,
    Unauthorized,
    Network,
    Http,
    BadResponse,
}

/// A fixed, neutral Jira failure. Messages never contain the token, an
/// Authorization header, the request URL with credentials, or the response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JiraError {
    pub kind: JiraErrorKind,
    pub message: String,
}

impl JiraError {
    fn new(kind: JiraErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn not_configured() -> Self {
        Self::new(JiraErrorKind::NotConfigured, MSG_NOT_CONFIGURED)
    }

    fn missing_token() -> Self {
        Self::new(JiraErrorKind::MissingToken, MSG_MISSING_TOKEN)
    }

    fn unauthorized() -> Self {
        Self::new(JiraErrorKind::Unauthorized, MSG_UNAUTHORIZED)
    }

    fn network() -> Self {
        Self::new(JiraErrorKind::Network, MSG_NETWORK)
    }

    fn http(status: reqwest::StatusCode) -> Self {
        Self::new(
            JiraErrorKind::Http,
            format!("Jira returned HTTP {}", status.as_u16()),
        )
    }

    fn bad_response() -> Self {
        Self::new(JiraErrorKind::BadResponse, MSG_BAD_RESPONSE)
    }
}

/// One Jira issue projected for the dashboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JiraIssue {
    pub key: String,
    pub summary: String,
    pub status: String,
    pub status_category: String,
    pub priority: String,
    pub issuetype: String,
    pub updated: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    issues: Vec<RawIssue>,
}

#[derive(Debug, Deserialize)]
struct RawIssue {
    #[serde(default)]
    key: String,
    #[serde(default)]
    fields: RawFields,
}

#[derive(Debug, Default, Deserialize)]
struct RawFields {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    status: Option<RawNamed>,
    #[serde(default)]
    priority: Option<RawNamed>,
    #[serde(default)]
    issuetype: Option<RawNamed>,
    #[serde(default)]
    updated: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawNamed {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "statusCategory")]
    status_category: Option<RawStatusCategory>,
}

#[derive(Debug, Deserialize)]
struct RawStatusCategory {
    #[serde(default)]
    key: Option<String>,
}

type TokenLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Jira search client. `Debug` is manual so an injected token can never leak.
pub struct JiraClient {
    base_url: String,
    email: String,
    jql: String,
    max_results: u32,
    token_env: String,
    token_lookup: TokenLookup,
    http: Option<reqwest::Client>,
}

impl fmt::Debug for JiraClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JiraClient")
            .field("base_url", &self.base_url)
            .field("email", &self.email)
            .field("jql", &self.jql)
            .field("max_results", &self.max_results)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl JiraClient {
    /// Build a client whose token is read from the environment variable named
    /// by `settings.token_env` at each fetch.
    pub fn new(settings: JiraSettings) -> Self {
        Self::with_token_lookup(settings, |name| std::env::var(name).ok())
    }

    /// Build a client with a fixed token (tests, or an already-resolved token).
    pub fn with_token(settings: JiraSettings, token: impl Into<String>) -> Self {
        let token = token.into();
        Self::with_token_lookup(settings, move |_| Some(token.clone()))
    }

    /// Build a client from an arbitrary token lookup closure.
    pub fn with_token_lookup<F>(settings: JiraSettings, lookup: F) -> Self
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        // reqwest is built without a bundled TLS provider; install the ring
        // one. `install_default` fails when a provider is already installed.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .ok();
        Self {
            base_url: settings.base_url.trim_end_matches('/').to_owned(),
            email: settings.email,
            jql: settings.jql,
            max_results: settings.max_results,
            token_env: settings.token_env,
            token_lookup: Arc::new(lookup),
            http,
        }
    }

    fn token(&self) -> Result<String, JiraError> {
        (self.token_lookup)(&self.token_env)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(JiraError::missing_token)
    }

    async fn send(&self, token: &str, path: &str) -> Result<reqwest::Response, JiraError> {
        let Some(http) = self.http.as_ref() else {
            return Err(JiraError::network());
        };
        let params = [
            ("jql", self.jql.clone()),
            ("maxResults", self.max_results.to_string()),
            ("fields", SEARCH_FIELDS.to_owned()),
        ];
        let url = format!("{}{}", self.base_url, path);
        http.get(url)
            .query(&params)
            .basic_auth(&self.email, Some(token))
            .send()
            .await
            .map_err(|_| JiraError::network())
    }

    /// Search Jira, falling back once to the legacy endpoint on 404/410.
    pub async fn search(&self) -> Result<Vec<JiraIssue>, JiraError> {
        if self.base_url.is_empty() || self.email.is_empty() {
            return Err(JiraError::not_configured());
        }
        let token = self.token()?;
        let mut response = self.send(&token, SEARCH_PATH).await?;
        if matches!(
            response.status(),
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE
        ) {
            response = self.send(&token, LEGACY_SEARCH_PATH).await?;
        }
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(JiraError::unauthorized());
        }
        if !status.is_success() {
            return Err(JiraError::http(status));
        }
        let text = response.text().await.map_err(|_| JiraError::network())?;
        let parsed: SearchResponse =
            serde_json::from_str(&text).map_err(|_| JiraError::bad_response())?;
        Ok(parsed
            .issues
            .into_iter()
            .map(|issue| self.map_issue(issue))
            .collect())
    }

    fn map_issue(&self, issue: RawIssue) -> JiraIssue {
        let fields = issue.fields;
        let status = fields.status;
        let status_category = status
            .as_ref()
            .and_then(|status| status.status_category.as_ref())
            .and_then(|category| category.key.clone())
            .unwrap_or_default();
        let status_name = status.and_then(|status| status.name).unwrap_or_default();
        JiraIssue {
            url: format!("{}/browse/{}", self.base_url, issue.key),
            key: issue.key,
            summary: fields.summary.unwrap_or_default(),
            status: status_name,
            status_category,
            priority: fields
                .priority
                .and_then(|priority| priority.name)
                .unwrap_or_default(),
            issuetype: fields
                .issuetype
                .and_then(|issuetype| issuetype.name)
                .unwrap_or_default(),
            updated: fields.updated.unwrap_or_default(),
        }
    }
}

/// Dashboard response for `GET /api/jira/issues`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JiraIssuesResponse {
    pub configured: bool,
    pub issues: Vec<JiraIssue>,
    pub error: Option<JiraError>,
    pub fetched_at: Option<u64>,
}

impl JiraIssuesResponse {
    pub fn not_configured() -> Self {
        Self {
            configured: false,
            issues: Vec::new(),
            error: Some(JiraError::not_configured()),
            fetched_at: None,
        }
    }
}

struct CacheEntry {
    issues: Vec<JiraIssue>,
    fetched_at_secs: u64,
    fetched_at: Instant,
}

/// In-memory Jira search cache used by the dashboard endpoint.
pub struct JiraService {
    client: Option<JiraClient>,
    cache_ttl: Duration,
    cache: tokio::sync::Mutex<Option<CacheEntry>>,
}

impl JiraService {
    pub fn new(client: Option<JiraClient>) -> Self {
        Self::with_ttl(client, DEFAULT_CACHE_TTL)
    }

    pub fn with_ttl(client: Option<JiraClient>, cache_ttl: Duration) -> Self {
        Self {
            client,
            cache_ttl,
            cache: tokio::sync::Mutex::new(None),
        }
    }

    /// Build from resolved settings; `None` means Jira is not configured.
    pub fn from_settings(settings: Option<JiraSettings>) -> Self {
        Self::new(settings.map(JiraClient::new))
    }

    pub fn is_configured(&self) -> bool {
        self.client.is_some()
    }

    /// Return the dashboard response, using or bypassing the successful-result cache.
    pub async fn issues(&self, refresh: bool) -> JiraIssuesResponse {
        let Some(client) = self.client.as_ref() else {
            return JiraIssuesResponse::not_configured();
        };
        if !refresh {
            let cache = self.cache.lock().await;
            if let Some(entry) = cache.as_ref()
                && entry.fetched_at.elapsed() < self.cache_ttl
            {
                return JiraIssuesResponse {
                    configured: true,
                    issues: entry.issues.clone(),
                    error: None,
                    fetched_at: Some(entry.fetched_at_secs),
                };
            }
        }
        match client.search().await {
            Ok(issues) => {
                let fetched_at_secs = unix_now();
                *self.cache.lock().await = Some(CacheEntry {
                    issues: issues.clone(),
                    fetched_at_secs,
                    fetched_at: Instant::now(),
                });
                JiraIssuesResponse {
                    configured: true,
                    issues,
                    error: None,
                    fetched_at: Some(fetched_at_secs),
                }
            }
            Err(error) => JiraIssuesResponse {
                configured: true,
                issues: Vec::new(),
                error: Some(error),
                fetched_at: None,
            },
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Debug, Clone)]
    struct Captured {
        method: String,
        path: String,
        query: String,
        authorization: Option<String>,
    }

    async fn fake_jira(
        responses: Vec<(u16, String)>,
    ) -> (
        String,
        Arc<Mutex<Vec<Captured>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let handle = tokio::spawn(async move {
            let mut index = 0;
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = vec![0_u8; 16 * 1024];
                let Ok(read) = socket.read(&mut buffer).await else {
                    continue;
                };
                let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let mut lines = request.lines();
                let request_line = lines.next().unwrap_or_default();
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_owned();
                let target = parts.next().unwrap_or_default();
                let (path, query) = match target.split_once('?') {
                    Some((path, query)) => (path.to_owned(), query.to_owned()),
                    None => (target.to_owned(), String::new()),
                };
                let authorization = lines
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                    .map(|(_, value)| value.trim().to_owned());
                recorded.lock().unwrap().push(Captured {
                    method,
                    path,
                    query,
                    authorization,
                });
                let (status, body) = responses
                    .get(index)
                    .cloned()
                    .unwrap_or((500, "{}".to_owned()));
                index += 1;
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        (format!("http://127.0.0.1:{}", addr.port()), calls, handle)
    }

    fn settings(base_url: &str) -> JiraSettings {
        JiraSettings {
            base_url: base_url.to_owned(),
            email: "user@example.com".to_owned(),
            jql: "project = DEMO".to_owned(),
            max_results: 50,
            token_env: "DMUXAI_JIRA_TOKEN".to_owned(),
        }
    }

    fn client(base_url: &str, token: &str) -> JiraClient {
        JiraClient::with_token(settings(base_url), token)
    }

    fn query_value(query: &str, key: &str) -> Option<String> {
        query.split('&').find_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            (percent_decode(name) == key).then(|| percent_decode(value))
        })
    }

    fn percent_decode(value: &str) -> String {
        let bytes = value.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => out.push(b' '),
                b'%' if i + 2 < bytes.len() => {
                    let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                    if let Ok(byte) = u8::from_str_radix(hex, 16) {
                        out.push(byte);
                        i += 3;
                        continue;
                    }
                    out.push(bytes[i]);
                }
                byte => out.push(byte),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn base64(input: &str) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bytes = input.as_bytes();
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(TABLE[((n >> 18) & 63) as usize] as char);
            out.push(TABLE[((n >> 12) & 63) as usize] as char);
            out.push(if chunk.len() > 1 {
                TABLE[((n >> 6) & 63) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                TABLE[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        out
    }

    const FULL_BODY: &str = r#"{"issues":[{"key":"DEMO-1","fields":{"summary":"Demo summary","status":{"name":"In Progress","statusCategory":{"key":"indeterminate"}},"priority":{"name":"High"},"issuetype":{"name":"Task"},"updated":"2026-01-01T00:00:00.000+0000"}}]}"#;

    // TC-2402 / FR-2402: request shape, Basic auth, and no request without a token.
    #[tokio::test]
    async fn tc_2402_search_jql_request_shape() {
        let (base, calls, server) = fake_jira(vec![(200, FULL_BODY.to_owned())]).await;
        let issues = client(&base, "demo-token-XYZ").search().await.unwrap();
        assert_eq!(issues.len(), 1);
        {
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            let call = &calls[0];
            assert_eq!(call.method, "GET");
            assert_eq!(call.path, "/rest/api/3/search/jql");
            assert_eq!(
                query_value(&call.query, "jql").as_deref(),
                Some("project = DEMO")
            );
            assert_eq!(
                query_value(&call.query, "maxResults").as_deref(),
                Some("50")
            );
            assert_eq!(
                query_value(&call.query, "fields").as_deref(),
                Some("summary,status,priority,issuetype,updated")
            );
            let expected_auth = format!("Basic {}", base64("user@example.com:demo-token-XYZ"));
            assert_eq!(call.authorization.as_deref(), Some(expected_auth.as_str()));
        }

        let before = calls.lock().unwrap().len();
        let error = client(&base, "").search().await.unwrap_err();
        assert_eq!(error.kind, JiraErrorKind::MissingToken);
        assert_eq!(
            calls.lock().unwrap().len(),
            before,
            "no request without token"
        );
        server.abort();
    }

    // TC-2403 / FR-2403: fallback only on 404/410, never on other statuses.
    #[tokio::test]
    async fn tc_2403_fallback_to_legacy_search() {
        for status in [404_u16, 410] {
            let (base, calls, server) =
                fake_jira(vec![(status, "{}".to_owned()), (200, FULL_BODY.to_owned())]).await;
            let issues = client(&base, "demo-token-XYZ").search().await.unwrap();
            assert_eq!(issues[0].key, "DEMO-1");
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 2, "status {status}");
            assert_eq!(calls[0].path, "/rest/api/3/search/jql");
            assert_eq!(calls[1].path, "/rest/api/3/search");
            assert_eq!(calls[0].query, calls[1].query);
            drop(calls);
            server.abort();
        }

        let (base, calls, server) = fake_jira(vec![(500, "{}".to_owned())]).await;
        let error = client(&base, "demo-token-XYZ").search().await.unwrap_err();
        assert_eq!(error.kind, JiraErrorKind::Http);
        assert_eq!(calls.lock().unwrap().len(), 1, "500 is not retried");
        server.abort();
    }

    // TC-2404 / FR-2404: field mapping, browse URL, sparse and malformed bodies.
    #[tokio::test]
    async fn tc_2404_issue_mapping() {
        let (base, _calls, server) = fake_jira(vec![(200, FULL_BODY.to_owned())]).await;
        let issue = client(&base, "demo-token-XYZ")
            .search()
            .await
            .unwrap()
            .remove(0);
        assert_eq!(issue.key, "DEMO-1");
        assert_eq!(issue.summary, "Demo summary");
        assert_eq!(issue.status, "In Progress");
        assert_eq!(issue.status_category, "indeterminate");
        assert_eq!(issue.priority, "High");
        assert_eq!(issue.issuetype, "Task");
        assert_eq!(issue.updated, "2026-01-01T00:00:00.000+0000");
        assert_eq!(issue.url, format!("{base}/browse/DEMO-1"));
        server.abort();

        let sparse = r#"{"issues":[{"key":"DEMO-2","fields":{}}]}"#;
        let (base, _calls, server) = fake_jira(vec![(200, sparse.to_owned())]).await;
        let issue = client(&base, "demo-token-XYZ")
            .search()
            .await
            .unwrap()
            .remove(0);
        assert_eq!(issue.key, "DEMO-2");
        assert_eq!(issue.summary, "");
        assert_eq!(issue.status, "");
        assert_eq!(issue.status_category, "");
        assert_eq!(issue.priority, "");
        assert_eq!(issue.issuetype, "");
        assert_eq!(issue.updated, "");
        assert_eq!(issue.url, format!("{base}/browse/DEMO-2"));
        server.abort();

        let (base, _calls, server) = fake_jira(vec![(200, r#"{"issues":"#.to_owned())]).await;
        let error = client(&base, "demo-token-XYZ").search().await.unwrap_err();
        assert_eq!(error.kind, JiraErrorKind::BadResponse);
        assert_eq!(error.message, MSG_BAD_RESPONSE);
        server.abort();
    }

    // TC-2405 / FR-2405: neutral error mapping; no token or body in diagnostics.
    #[tokio::test]
    async fn tc_2405_error_mapping_is_neutral() {
        let token = "demo-token-XYZ";
        let secrets = "demo-token-XYZ";

        for (status, kind) in [
            (401_u16, JiraErrorKind::Unauthorized),
            (403, JiraErrorKind::Unauthorized),
        ] {
            let (base, _calls, server) =
                fake_jira(vec![(status, format!(r#"{{"error":"{secrets}"}}"#))]).await;
            let error = client(&base, token).search().await.unwrap_err();
            assert_eq!(error.kind, kind, "status {status}");
            assert!(!error.message.contains(secrets));
            assert!(!format!("{error:?}").contains(secrets));
            server.abort();
        }

        let (base, _calls, server) = fake_jira(vec![(500, secrets.to_owned())]).await;
        let error = client(&base, token).search().await.unwrap_err();
        assert_eq!(error.kind, JiraErrorKind::Http);
        assert!(error.message.contains("500"));
        assert!(!error.message.contains(secrets));
        assert!(!format!("{error:?}").contains(secrets));
        server.abort();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let error = client(&format!("http://127.0.0.1:{port}"), token)
            .search()
            .await
            .unwrap_err();
        assert_eq!(error.kind, JiraErrorKind::Network);
        assert_eq!(error.message, MSG_NETWORK);

        let debug = format!("{:?}", client("http://127.0.0.1:1", token));
        assert!(!debug.contains(token), "Debug leaked token: {debug}");
    }
}
