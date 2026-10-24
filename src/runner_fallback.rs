//! Runner fallback (SPEC-runner-fallback): when a runner's request fails for
//! provider reasons or stalls, the turn is handed to the next runner/model in
//! an ordered chain.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackTarget {
    pub runner: String,
    pub model: Option<String>,
}

impl FallbackTarget {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("empty target".to_owned());
        }
        let (runner_part, model_part) = match raw.find(':') {
            Some(i) => (&raw[..i], Some(&raw[i + 1..])),
            None => (raw, None),
        };
        let runner = runner_part.trim().to_ascii_lowercase();
        if runner.is_empty() {
            return Err("empty runner".to_owned());
        }
        let model = model_part
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty());
        Ok(Self { runner, model })
    }
}

impl fmt::Display for FallbackTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.model {
            Some(m) => write!(f, "{}:{}", self.runner, m),
            None => write!(f, "{}", self.runner),
        }
    }
}

pub const DEFAULT_STALL_TIMEOUT_SECS: u64 = 240;
pub const DEFAULT_STALL_API_RETRIES: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerFallbackConfig {
    pub chain: Vec<FallbackTarget>,
    pub stall_timeout_secs: u64,
    pub stall_api_retries: u32,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
pub struct RunnerFallbackToml {
    pub fallback: Option<Vec<String>>,
    pub stall_timeout_secs: Option<u64>,
    pub stall_api_retries: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FallbackSettings {
    pub default: Option<RunnerFallbackConfig>,
    pub runners: BTreeMap<String, RunnerFallbackConfig>,
}

impl FallbackSettings {
    pub fn from_tables(
        tables: &BTreeMap<String, RunnerFallbackToml>,
        validate: &dyn Fn(&FallbackTarget) -> Result<(), String>,
        warnings: &mut Vec<String>,
    ) -> Self {
        let mut settings = Self::default();
        for (id, toml) in tables {
            let has_stall = toml.stall_timeout_secs.is_some() || toml.stall_api_retries.is_some();
            if toml.fallback.is_none() && !has_stall {
                continue;
            }
            let mut chain: Vec<FallbackTarget> = Vec::new();
            if let Some(list) = &toml.fallback {
                let mut seen: Vec<FallbackTarget> = Vec::new();
                for raw in list {
                    let target = match FallbackTarget::parse(raw) {
                        Ok(t) => t,
                        Err(err) => {
                            warnings
                                .push(format!("[runners.{id}] fallback '{raw}' ignored: {err}"));
                            continue;
                        }
                    };
                    if let Err(err) = validate(&target) {
                        warnings.push(format!("[runners.{id}] fallback '{raw}' ignored: {err}"));
                        continue;
                    }
                    if seen.contains(&target) {
                        warnings.push(format!(
                            "[runners.{id}] fallback '{raw}' ignored: duplicate"
                        ));
                        continue;
                    }
                    seen.push(target.clone());
                    chain.push(target);
                }
            }
            let cfg = RunnerFallbackConfig {
                chain,
                stall_timeout_secs: toml
                    .stall_timeout_secs
                    .unwrap_or(DEFAULT_STALL_TIMEOUT_SECS),
                stall_api_retries: toml.stall_api_retries.unwrap_or(DEFAULT_STALL_API_RETRIES),
            };
            if id == "default" {
                settings.default = Some(cfg);
            } else {
                settings.runners.insert(id.clone(), cfg);
            }
        }
        settings
    }

    pub fn merge(user: Self, workspace: Self) -> Self {
        let mut result = user;
        if workspace.default.is_some() {
            result.default = workspace.default;
        }
        for (k, v) in workspace.runners {
            result.runners.insert(k, v);
        }
        result
    }

    pub fn config_for(&self, runner: &str) -> Option<&RunnerFallbackConfig> {
        let id = runner.trim().to_ascii_lowercase();
        if let Some(c) = self.runners.get(&id) {
            return Some(c);
        }
        self.default.as_ref()
    }

    pub fn chain_for(&self, runner: &str, current_model: Option<&str>) -> Vec<FallbackTarget> {
        let Some(cfg) = self.config_for(runner) else {
            return Vec::new();
        };
        let runner_lc = runner.trim().to_ascii_lowercase();
        let cur = current_model.map(str::to_owned);
        cfg.chain
            .iter()
            .filter(|t| {
                if !t.runner.eq_ignore_ascii_case(&runner_lc) {
                    return true;
                }
                match &t.model {
                    None => false,
                    Some(m) => Some(m.clone()) != cur,
                }
            })
            .cloned()
            .collect()
    }

    pub fn stall_policy_for(&self, runner: &str) -> Option<StallPolicy> {
        if self.chain_for(runner, None).is_empty() {
            return None;
        }
        let cfg = self.config_for(runner)?;
        let timeout = if cfg.stall_timeout_secs == 0 {
            None
        } else {
            Some(Duration::from_secs(cfg.stall_timeout_secs))
        };
        let retries = if cfg.stall_api_retries == 0 {
            None
        } else {
            Some(cfg.stall_api_retries)
        };
        if timeout.is_none() && retries.is_none() {
            return None;
        }
        Some(StallPolicy {
            timeout,
            max_api_retries: retries,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    Provider,
    Quota,
    Auth,
    Unavailable,
    Stall,
}

impl FallbackReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Quota => "quota",
            Self::Auth => "auth",
            Self::Unavailable => "unavailable",
            Self::Stall => "stall",
        }
    }
}

static QUOTA_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)quota|rate.?limit|\b429\b|resource.?exhausted|too many requests|usage limit|(out of|insufficient|no remaining) credits?").unwrap()
});
static AUTH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)unauthori[sz]ed|\b401\b|\b403\b|not (logged|signed) in|log ?in (again|required)|auth(entication)? (expired|required|failed)|(token|session|credential)s? (has )?expired|invalid api key").unwrap()
});
static UNAVAIL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)could not start|no such file or directory|not found in path|command not found")
        .unwrap()
});

pub fn is_transient_provider_error(message: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)api error \(attempt \d+\)|request failed|stream idle timeout|stream was interrupted|connection (timed out|reset|refused|closed)|(request|read|connect|connection|operation) timed out|(status|code|http)[ :=(]*5\d\d|\b5\d\d (internal|bad gateway|service unavailable|gateway timeout)|internal \(code|\binternal\b.*\berror\b|service unavailable|\bunavailable\b|overloaded|bad gateway|gateway timeout|deadline exceeded|broken pipe|eof while|network (error|is unreachable)").unwrap()
    });
    RE.is_match(message)
}

pub fn is_api_retry_line(line: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)api error \(attempt \d+\)|stream idle timeout|retrying (request|in)|request failed: .*(timed out|timeout|reset)").unwrap()
    });
    RE.is_match(line)
}

pub fn classify_failure(kind: &str, message: &str) -> Option<FallbackReason> {
    let kind = kind.trim().to_ascii_lowercase();
    if kind == "stalled" {
        return Some(FallbackReason::Stall);
    }
    if matches!(kind.as_str(), "cancelled" | "policy" | "timeout" | "idle") {
        return None;
    }
    if kind == "model_unavailable" {
        if QUOTA_RE.is_match(message) {
            return Some(FallbackReason::Quota);
        }
        return Some(FallbackReason::Unavailable);
    }
    // provider | process_exit | unknown
    if QUOTA_RE.is_match(message) {
        return Some(FallbackReason::Quota);
    }
    if AUTH_RE.is_match(message) {
        return Some(FallbackReason::Auth);
    }
    if is_transient_provider_error(message) {
        return Some(FallbackReason::Provider);
    }
    if UNAVAIL_RE.is_match(message) {
        return Some(FallbackReason::Unavailable);
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StallPolicy {
    pub timeout: Option<Duration>,
    pub max_api_retries: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct StallDetector {
    policy: StallPolicy,
    last_progress: Instant,
    retry_counter: u32,
    running_tools: u32,
}

impl StallDetector {
    pub fn new(policy: StallPolicy, now: Instant) -> Self {
        Self {
            policy,
            last_progress: now,
            retry_counter: 0,
            running_tools: 0,
        }
    }
    pub fn observe_progress(&mut self, now: Instant) {
        self.last_progress = now;
        self.retry_counter = 0;
    }
    pub fn observe_tool_started(&mut self, now: Instant) {
        let _ = now;
        self.running_tools = self.running_tools.saturating_add(1);
    }
    pub fn observe_tool_finished(&mut self, now: Instant) {
        self.running_tools = self.running_tools.saturating_sub(1);
        self.last_progress = now;
        self.retry_counter = 0;
    }
    pub fn observe_keepalive(&mut self) {
        // no-op
    }
    pub fn observe_api_retry(&mut self, now: Instant) {
        let _ = now;
        self.retry_counter = self.retry_counter.saturating_add(1);
    }
    pub fn check(&self, now: Instant) -> Option<String> {
        if let Some(n) = self.policy.max_api_retries
            && n > 0
            && self.retry_counter >= n
        {
            return Some(format!(
                "{} consecutive provider API retries without progress",
                self.retry_counter
            ));
        }
        if let Some(t) = self.policy.timeout
            && self.running_tools == 0
            && now.saturating_duration_since(self.last_progress) >= t
        {
            return Some(format!("no model progress for {}s", t.as_secs()));
        }
        None
    }
}

pub fn handover_preface(from: &str, reason: FallbackReason, detail: &str) -> String {
    let trimmed: String = detail.trim().chars().take(200).collect();
    format!(
        "[DmuxAI runner fallback] The previous agent ({from}) was interrupted ({reason}: {trimmed}) and may have left partial edits in the workspace. Check `git status` and `git diff` first, keep correct partial work, and continue the task instead of restarting it.\n\n",
        reason = reason.as_str(),
    )
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FallbackRecord {
    pub from_runner: String,
    pub from_model: Option<String>,
    pub reason: String,
    pub detail: String,
    pub to_runner: Option<String>,
    pub to_model: Option<String>,
}

impl FallbackRecord {
    pub fn summary(&self) -> String {
        let from_part = match &self.from_model {
            Some(m) => format!("{} ({})", self.from_runner, m),
            None => self.from_runner.clone(),
        };
        let to_part = match &self.to_runner {
            Some(r) => {
                let m = match &self.to_model {
                    Some(m) => format!(" ({})", m),
                    None => String::new(),
                };
                format!("handing the turn to {}{}", r, m)
            }
            None => "fallback chain exhausted".to_owned(),
        };
        format!(
            "runner fallback: {} failed ({}: {}); {}",
            from_part, self.reason, self.detail, to_part
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackChain {
    remaining: VecDeque<FallbackTarget>,
    history: Vec<FallbackRecord>,
}

impl FallbackChain {
    pub fn new(targets: Vec<FallbackTarget>) -> Self {
        Self {
            remaining: targets.into(),
            history: Vec::new(),
        }
    }
    pub fn next_target(
        &mut self,
        is_usable: impl Fn(&FallbackTarget) -> bool,
    ) -> Option<FallbackTarget> {
        while let Some(t) = self.remaining.pop_front() {
            if is_usable(&t) {
                return Some(t);
            }
        }
        None
    }
    pub fn record(&mut self, record: FallbackRecord) {
        self.history.push(record);
    }
    pub fn history(&self) -> &[FallbackRecord] {
        &self.history
    }
    pub fn is_exhausted(&self) -> bool {
        self.remaining.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn tc_2100_target_parse() {
        assert_eq!(
            FallbackTarget::parse("opencode-step").unwrap(),
            FallbackTarget {
                runner: "opencode-step".into(),
                model: None
            }
        );
        assert_eq!(
            FallbackTarget::parse("agy-flash:gemini-3.8-flash-low").unwrap(),
            FallbackTarget {
                runner: "agy-flash".into(),
                model: Some("gemini-3.8-flash-low".into())
            }
        );
        assert_eq!(
            FallbackTarget::parse(" Grok : grok-code ").unwrap(),
            FallbackTarget {
                runner: "grok".into(),
                model: Some("grok-code".into())
            }
        );
        assert_eq!(
            FallbackTarget::parse("opencode-step:opencode/beam-free").unwrap(),
            FallbackTarget {
                runner: "opencode-step".into(),
                model: Some("opencode/beam-free".into())
            }
        );
        assert!(FallbackTarget::parse("").is_err());
        assert!(FallbackTarget::parse(":x").is_err());
        // display
        assert_eq!(
            FallbackTarget::parse("opencode-step").unwrap().to_string(),
            "opencode-step"
        );
        assert_eq!(
            FallbackTarget::parse("agy-flash:gemini-3.8-flash-low")
                .unwrap()
                .to_string(),
            "agy-flash:gemini-3.8-flash-low"
        );
    }

    #[test]
    fn tc_2101_config_for_prefers_runner_entry_over_default() {
        let mut tables = BTreeMap::new();
        tables.insert(
            "default".to_string(),
            RunnerFallbackToml {
                fallback: Some(vec!["opencode".into()]),
                stall_timeout_secs: None,
                stall_api_retries: None,
            },
        );
        tables.insert(
            "agy".to_string(),
            RunnerFallbackToml {
                fallback: Some(vec![]),
                stall_timeout_secs: None,
                stall_api_retries: None,
            },
        );
        let validate = |_: &FallbackTarget| Ok(());
        let mut w = Vec::new();
        let s = FallbackSettings::from_tables(&tables, &validate, &mut w);
        // agy empty chain disables default
        assert!(s.config_for("agy").unwrap().chain.is_empty());
        assert_eq!(s.chain_for("agy", None).len(), 0);
        // default has chain
        assert_eq!(s.config_for("cursor").unwrap().chain.len(), 1);
        // merge
        let mut ut = BTreeMap::new();
        ut.insert(
            "default".to_string(),
            RunnerFallbackToml {
                fallback: Some(vec!["opencode".into()]),
                stall_timeout_secs: None,
                stall_api_retries: None,
            },
        );
        let user = FallbackSettings::from_tables(&ut, &validate, &mut Vec::new());
        let mut wt = BTreeMap::new();
        wt.insert(
            "agy".to_string(),
            RunnerFallbackToml {
                fallback: Some(vec!["opencode".into()]),
                stall_timeout_secs: Some(120),
                stall_api_retries: None,
            },
        );
        let workspace = FallbackSettings::from_tables(&wt, &validate, &mut Vec::new());
        let merged = FallbackSettings::merge(user, workspace);
        assert!(merged.runners.contains_key("agy"));
        assert_eq!(merged.runners.get("agy").unwrap().stall_timeout_secs, 120);
    }

    #[test]
    fn tc_2102_invalid_targets_warn_and_skip() {
        let mut tables = BTreeMap::new();
        tables.insert(
            "agy".to_string(),
            RunnerFallbackToml {
                fallback: Some(vec![
                    "agy:gemini-3-pro".into(),
                    "opencode".into(),
                    "opencode".into(),
                ]),
                stall_timeout_secs: None,
                stall_api_retries: None,
            },
        );
        let validate = |t: &FallbackTarget| {
            if t.runner == "agy" && t.model.as_deref() == Some("gemini-3-pro") {
                Err("not allowed".to_owned())
            } else {
                Ok(())
            }
        };
        let mut w = Vec::new();
        let s = FallbackSettings::from_tables(&tables, &validate, &mut w);
        let chain = &s.runners.get("agy").unwrap().chain;
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].runner, "opencode");
        assert!(w.iter().any(
            |x| x.contains("fallback 'agy:gemini-3-pro' ignored") && x.contains("not allowed")
        ));
        assert!(
            w.iter()
                .any(|x| x.contains("fallback 'opencode' ignored: duplicate"))
        );
    }

    #[test]
    fn tc_2103_chain_excludes_self() {
        let mut tables = BTreeMap::new();
        tables.insert(
            "agy".to_string(),
            RunnerFallbackToml {
                fallback: Some(vec![
                    "agy".into(),
                    "agy:gemini-3.8-flash-low".into(),
                    "opencode".into(),
                ]),
                stall_timeout_secs: None,
                stall_api_retries: None,
            },
        );
        let validate = |_: &FallbackTarget| Ok(());
        let mut w = Vec::new();
        let s = FallbackSettings::from_tables(&tables, &validate, &mut w);
        let chain = s.chain_for("agy", Some("gemini-3.8-flash-high"));
        // agy (no model) excluded; agy:gemini-3.8-flash-low kept (different model); opencode kept
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].runner, "agy");
        assert_eq!(chain[0].model.as_deref(), Some("gemini-3.8-flash-low"));
        // self with same model excluded
        let chain2 = s.chain_for("agy", Some("gemini-3.8-flash-low"));
        assert_eq!(chain2.len(), 1);
        assert_eq!(chain2[0].runner, "opencode");
    }

    #[test]
    fn tc_2104_classify_triggers() {
        assert_eq!(
            classify_failure(
                "provider",
                "API error (attempt 3): request failed: ... read tcp ...: connection timed out"
            ),
            Some(FallbackReason::Provider)
        );
        assert_eq!(
            classify_failure("provider", "429 Too Many Requests"),
            Some(FallbackReason::Quota)
        );
        assert_eq!(
            classify_failure("model_unavailable", "RESOURCE_EXHAUSTED quota"),
            Some(FallbackReason::Quota)
        );
        assert_eq!(
            classify_failure("provider", "authentication expired, please log in again"),
            Some(FallbackReason::Auth)
        );
        assert_eq!(
            classify_failure(
                "process_exit",
                "could not start AGY: No such file or directory"
            ),
            Some(FallbackReason::Unavailable)
        );
        assert_eq!(
            classify_failure("stalled", "x"),
            Some(FallbackReason::Stall)
        );
        assert_eq!(
            classify_failure("provider", "HTTP 503 Service Unavailable"),
            Some(FallbackReason::Provider)
        );
        assert_eq!(
            classify_failure("provider", "status: 500"),
            Some(FallbackReason::Provider)
        );
        assert_eq!(
            classify_failure(
                "provider",
                "agy flash failed: The stream was interrupted. Please continue the task you were working on."
            ),
            Some(FallbackReason::Provider)
        );
    }

    #[test]
    fn tc_2105_no_fallback_on_normal_failure() {
        assert_eq!(classify_failure("cancelled", "x"), None);
        assert_eq!(classify_failure("policy", "permission denied"), None);
        assert_eq!(classify_failure("timeout", "x"), None);
        assert_eq!(classify_failure("idle", "x"), None);
        assert_eq!(
            classify_failure("provider", "tests failed: 3 failures"),
            None
        );
        assert_eq!(
            classify_failure("provider", "tests failed at line 512"),
            None
        );
        assert_eq!(classify_failure("provider", "empty reply text"), None);
        assert_eq!(classify_failure("provider", "AGY turn failed"), None);
        assert_eq!(
            classify_failure("provider", "AGY returned plan-only output"),
            None
        );
    }

    #[test]
    fn tc_2106_stall_detection_with_keepalive() {
        let policy = StallPolicy {
            timeout: Some(Duration::from_secs(240)),
            max_api_retries: None,
        };
        // We can't easily advance Instant; use relative by storing base? Instant supports checked_add? Use Instant::now base and durations.
        // Use a base instant.
        let base = Instant::now();
        let mut d = StallDetector::new(policy, base);
        // keepalive every 10s for 300s
        for i in 1..=30 {
            let now = base + Duration::from_secs(i * 10);
            d.observe_keepalive();
            let _ = now;
        }
        // check at base+240s -> Some (no progress)
        assert!(d.check(base + Duration::from_secs(240)).is_some());
        // progress at 200s resets
        let mut d2 = StallDetector::new(policy, base);
        d2.observe_progress(base + Duration::from_secs(200));
        assert!(d2.check(base + Duration::from_secs(300)).is_none());
        // tool started at 10s still running at 1000s -> None
        let mut d3 = StallDetector::new(policy, base);
        d3.observe_tool_started(base + Duration::from_secs(10));
        assert!(d3.check(base + Duration::from_secs(1000)).is_none());
        // finished at 1000s then check 1239 None, 1240 Some
        d3.observe_tool_finished(base + Duration::from_secs(1000));
        assert!(d3.check(base + Duration::from_secs(1239)).is_none());
        assert!(d3.check(base + Duration::from_secs(1240)).is_some());
        // 3 consecutive api retries -> Some, progress resets
        let mut d4 = StallDetector::new(
            StallPolicy {
                timeout: None,
                max_api_retries: Some(3),
            },
            base,
        );
        d4.observe_api_retry(base);
        d4.observe_api_retry(base);
        d4.observe_api_retry(base);
        assert!(d4.check(base).is_some());
        let mut d5 = StallDetector::new(
            StallPolicy {
                timeout: None,
                max_api_retries: Some(3),
            },
            base,
        );
        d5.observe_api_retry(base);
        d5.observe_progress(base);
        d5.observe_api_retry(base);
        d5.observe_api_retry(base);
        assert!(d5.check(base).is_none());
    }

    #[test]
    fn tc_2107_stall_policy_only_with_chain() {
        let mut tables = BTreeMap::new();
        // no fallback chain
        tables.insert(
            "agy".to_string(),
            RunnerFallbackToml {
                fallback: Some(vec![]),
                stall_timeout_secs: Some(120),
                stall_api_retries: Some(2),
            },
        );
        let validate = |_: &FallbackTarget| Ok(());
        let mut w = Vec::new();
        let s = FallbackSettings::from_tables(&tables, &validate, &mut w);
        assert!(s.stall_policy_for("agy").is_none());
        // with chain
        let mut tables2 = BTreeMap::new();
        tables2.insert(
            "agy".to_string(),
            RunnerFallbackToml {
                fallback: Some(vec!["opencode".into()]),
                stall_timeout_secs: Some(120),
                stall_api_retries: Some(2),
            },
        );
        let s2 = FallbackSettings::from_tables(&tables2, &validate, &mut w);
        assert!(s2.stall_policy_for("agy").is_some());
        // both zero -> None
        let mut tables3 = BTreeMap::new();
        tables3.insert(
            "agy".to_string(),
            RunnerFallbackToml {
                fallback: Some(vec!["opencode".into()]),
                stall_timeout_secs: Some(0),
                stall_api_retries: Some(0),
            },
        );
        let s3 = FallbackSettings::from_tables(&tables3, &validate, &mut w);
        assert!(s3.stall_policy_for("agy").is_none());
    }

    #[test]
    fn tc_2108_chain_order_single_pass() {
        let mut fc = FallbackChain::new(vec![
            FallbackTarget {
                runner: "a".into(),
                model: None,
            },
            FallbackTarget {
                runner: "b".into(),
                model: Some("m".into()),
            },
            FallbackTarget {
                runner: "c".into(),
                model: None,
            },
        ]);
        let first = fc.next_target(|t| t.runner != "b").unwrap();
        assert_eq!(first.runner, "a");
        let second = fc.next_target(|t| t.runner != "b").unwrap();
        assert_eq!(second.runner, "c");
        assert!(fc.next_target(|t| t.runner != "b").is_none());
        assert!(fc.is_exhausted());
    }

    #[test]
    fn tc_2109_preface_text() {
        let p = handover_preface(
            "agy-flash",
            FallbackReason::Stall,
            "no model progress for 240s",
        );
        assert_eq!(
            p,
            "[DmuxAI runner fallback] The previous agent (agy-flash) was interrupted (stall: no model progress for 240s) and may have left partial edits in the workspace. Check `git status` and `git diff` first, keep correct partial work, and continue the task instead of restarting it.\n\n"
        );
    }

    #[test]
    fn tc_2110_record_summary() {
        let r = FallbackRecord {
            from_runner: "agy-flash".into(),
            from_model: Some("gemini-3.8-flash-high".into()),
            reason: "stall".into(),
            detail: "no model progress for 240s".into(),
            to_runner: Some("opencode-step".into()),
            to_model: Some("opencode/beam-free".into()),
        };
        assert_eq!(
            r.summary(),
            "runner fallback: agy-flash (gemini-3.8-flash-high) failed (stall: no model progress for 240s); handing the turn to opencode-step (opencode/beam-free)"
        );
        let r2 = FallbackRecord {
            from_runner: "agy-flash".into(),
            from_model: Some("gemini-3.8-flash-high".into()),
            reason: "stall".into(),
            detail: "no model progress for 240s".into(),
            to_runner: None,
            to_model: None,
        };
        assert_eq!(
            r2.summary(),
            "runner fallback: agy-flash (gemini-3.8-flash-high) failed (stall: no model progress for 240s); fallback chain exhausted"
        );
    }
}
