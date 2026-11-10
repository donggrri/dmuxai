//! Dangerous-command guard for terminal session input (SPEC-terminal-sessions
//! FR-2630). Blocked `send`/`exec` text requires a one-shot approval created by
//! the guard and released via `term approve`. This is a seatbelt, not a
//! sandbox: only CLI/agent input text is inspected, never content already
//! running inside a session.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use regex::Regex;

use super::TermError;
use crate::agent_api::exit;

/// `[terminal.policy] approve_timeout` default: 10 minutes.
pub const DEFAULT_APPROVE_TIMEOUT_MS: u64 = 600_000;

/// Upper bound for `approve_timeout`; anything larger falls back to the default.
pub const MAX_APPROVE_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;

/// Smallest accepted `approve_timeout`; 0 (and below) falls back to the default.
pub const MIN_APPROVE_TIMEOUT_MS: u64 = 1_000;

/// Approvals inside a run (DMUXAI_RUN_ID set) exit as forbidden (`term approve`
/// is reserved for humans).
pub const RUN_ID_ENV: &str = "DMUXAI_RUN_ID";

/// Default deny patterns (FR-2630), always active regardless of trust.
pub const DEFAULT_DENY_PATTERNS: &[&str] = &[
    r"\brm\s+-[a-zA-Z]*[fr][a-zA-Z]*\s+(/|~|\*|\.\.)",
    r"\bdd\b[^;\n]*\bof=",
    r"\b(?:mkfs|wipefs|fdisk|parted|sgdisk)\b",
    r"/dev/(?:sd|nvme|mmcblk|mtd)",
    r"\b(?:flash_erase|flashcp|nandwrite)\b",
    r"\bmtd\s+erase\b",
    r"\b(?:nand|sf|mmc)\s+erase\b",
    r"\berase\s",
    r"\benv default -a\b",
    r"\bsaveenv\b",
    r"\bfastboot\s+(?:flash|erase|format)\b",
    r"\b(?:reboot|poweroff|shutdown|halt)\b",
    r"\binit\s+[06]\b",
    r"\bsystemctl\s+(?:reboot|poweroff|shutdown)\b",
    r"\bchmod\s+-R\s+777\s+/",
    r":\s*\(\s*\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;\s*:",
];

/// Outcome of evaluating one input against the policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The text may be written.
    Allowed,
    /// The write is refused; the pattern that matched is reported so the
    /// caller can create an approval with `{approval_id, matched, session}`.
    ApprovalRequired { matched: String },
}

/// Raw trusted `[terminal]` knobs resolved by `settings.load` (FR-2630,
/// FR-2614). Defaults keep the spec behavior: default deny only, 10-minute
/// approvals, no usbipd busid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSettings {
    /// Trusted `[terminal.policy] allow` regexes; a match always wins.
    pub allow: Vec<String>,
    /// Trusted `[terminal.policy] deny` regexes, added to the default deny list.
    pub deny: Vec<String>,
    /// `approve_timeout` in milliseconds after which a created-but-unapproved
    /// or approved-but-unconsumed id is invalid.
    pub approve_timeout_ms: u64,
    /// `[terminal.serial] usbipd_busid` (FR-2614), trusted workspace only.
    pub serial_usbipd_busid: Option<String>,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            allow: Vec::new(),
            deny: Vec::new(),
            approve_timeout_ms: DEFAULT_APPROVE_TIMEOUT_MS,
            serial_usbipd_busid: None,
        }
    }
}

/// Parses a `approve_timeout` config value. An integer is milliseconds; a
/// string like `"10m"`, `"45s"` or `"1h30m"` is parsed with those units.
/// Returns `None` (caller warns and keeps the default) for 0, out-of-range or
/// unparseable values.
pub fn parse_approve_timeout_ms(value: &toml::Value) -> Option<u64> {
    let ms = match value {
        toml::Value::Integer(ms) => u64::try_from(*ms).ok()?,
        toml::Value::String(text) => parse_humantime_ms(text)?,
        _ => return None,
    };
    (MIN_APPROVE_TIMEOUT_MS..=MAX_APPROVE_TIMEOUT_MS)
        .contains(&ms)
        .then_some(ms)
}

/// Parses `"45s"`, `"10m"`, `"1h"` or concatenated forms like `"1h30m"`.
fn parse_humantime_ms(text: &str) -> Option<u64> {
    let mut total_ms = 0u64;
    let mut rest = text.trim();
    let mut matched_any = false;
    while !rest.is_empty() {
        let digits_end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let (digits, after) = rest.split_at(digits_end);
        let number: u64 = if digits.is_empty() {
            0
        } else {
            digits.parse().ok()?
        };
        let unit_end = after
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(after.len());
        let unit = &after[..unit_end];
        let multiplier = match unit {
            "s" => 1_000u64,
            "m" => 60_000,
            "h" => 3_600_000,
            "ms" => 1,
            _ => return None,
        };
        total_ms = total_ms.checked_add(number.checked_mul(multiplier)?)?;
        rest = &after[unit_end..];
        matched_any = true;
    }
    matched_any.then_some(total_ms)
}

/// An evaluated policy: trusted allow regexes plus (default + trusted extra)
/// deny regexes, and the approval timeout. A policy that fails to compile a
/// configured regex keeps the other patterns and reports them in warnings.
#[derive(Debug, Clone)]
pub struct Policy {
    allow: Vec<Regex>,
    deny: Vec<Regex>,
    approve_timeout: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        let deny = DEFAULT_DENY_PATTERNS
            .iter()
            .filter_map(|pattern| Regex::new(pattern).ok())
            .collect();
        Self {
            allow: Vec::new(),
            deny,
            approve_timeout: Duration::from_millis(DEFAULT_APPROVE_TIMEOUT_MS),
        }
    }
}

impl Policy {
    /// Compiles the trusted `[terminal.policy]` overrides on top of the
    /// default deny list. Invalid regexes are skipped and named in the
    /// returned warnings.
    pub fn compile(settings: &TerminalSettings) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();

        let mut allow = Vec::new();
        for pattern in &settings.allow {
            match Regex::new(pattern) {
                Ok(regex) => allow.push(regex),
                Err(error) => warnings.push(format!(
                    "[terminal.policy] allow '{pattern}' ignored: invalid regex ({error})"
                )),
            }
        }

        let mut deny: Vec<Regex> = DEFAULT_DENY_PATTERNS
            .iter()
            .filter_map(|pattern| Regex::new(pattern).ok())
            .collect();
        for pattern in &settings.deny {
            match Regex::new(pattern) {
                Ok(regex) => deny.push(regex),
                Err(error) => warnings.push(format!(
                    "[terminal.policy] deny '{pattern}' ignored: invalid regex ({error})"
                )),
            }
        }

        let approve_timeout = Duration::from_millis(settings.approve_timeout_ms);
        (
            Self {
                allow,
                deny,
                approve_timeout,
            },
            warnings,
        )
    }

    pub fn approve_timeout(&self) -> Duration {
        self.approve_timeout
    }

    /// FR-2630: a trusted allow regex wins, then default and trusted extra
    /// deny patterns block the write with the matched pattern.
    pub fn evaluate(&self, text: &str) -> Decision {
        if self.allow.iter().any(|pattern| pattern.is_match(text)) {
            return Decision::Allowed;
        }
        if let Some(pattern) = self.deny.iter().find(|pattern| pattern.is_match(text)) {
            return Decision::ApprovalRequired {
                matched: pattern.as_str().to_owned(),
            };
        }
        Decision::Allowed
    }
}

/// Outcome of an approval lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApproveOutcome {
    /// The approval exists, has not expired, and is now armed for release.
    Approved,
    Unknown,
    Expired,
}

#[derive(Debug)]
struct PendingApproval {
    session: String,
    text: String,
    matched: String,
    created_at: Instant,
    approved: bool,
}

/// Store of approval ids for blocked inputs (FR-2630).
///
/// Flow: a blocked input creates an entry via [`issue`]; `term approve <id>`
/// arms it with [`approve`]; the next identical send on the same session
/// consumed it one-shot via [`release`]. Entries expire after the configured
/// timeout. All clocks come from the caller so tests are deterministic.
#[derive(Debug, Default)]
pub struct ApprovalGate {
    entries: Mutex<HashMap<String, PendingApproval>>,
    timeout: Duration,
}

impl ApprovalGate {
    pub fn new(timeout: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            timeout,
        }
    }

    /// Creates a one-shot approval for a blocked input and returns its id.
    pub fn issue(&self, session: &str, text: &str, matched: &str, now: Instant) -> String {
        let id = generate_approval_id();
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                id.clone(),
                PendingApproval {
                    session: session.to_owned(),
                    text: text.to_owned(),
                    matched: matched.to_owned(),
                    created_at: now,
                    approved: false,
                },
            );
        id
    }

    /// `term approve <id>`: arms the one-shot release for the input.
    pub fn approve(&self, id: &str, now: Instant) -> ApproveOutcome {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = entries.get_mut(id) else {
            return ApproveOutcome::Unknown;
        };
        if now.duration_since(entry.created_at) >= self.timeout {
            entries.remove(id);
            return ApproveOutcome::Expired;
        }
        entry.approved = true;
        ApproveOutcome::Approved
    }

    /// Consumes an armed approval for exactly this session and text. Returns
    /// the matched pattern when the input may now go through.
    pub fn release(&self, session: &str, text: &str, now: Instant) -> Option<String> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let id = entries.iter().find_map(|(id, entry)| {
            let eligible = entry.approved
                && entry.session == session
                && entry.text == text
                && now.duration_since(entry.created_at) < self.timeout;
            eligible.then(|| id.clone())
        })?;
        let matched = entries
            .get(&id)
            .map(|entry| entry.matched.clone())
            .unwrap_or_default();
        entries.remove(&id);
        Some(matched)
    }
}

/// A 16-hex approval id, like graph plan approval ids.
pub fn generate_approval_id() -> String {
    let uuid = uuid::Uuid::new_v4();
    let bytes = uuid.as_bytes();
    format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]
    )
}

/// FR-2630: `term approve` is refused (exit 5, `forbidden`) whenever
/// `DMUXAI_RUN_ID` is set, so an agent cannot approve its own blocked input.
pub fn approve_with_run_guard(
    gate: &ApprovalGate,
    id: &str,
    run_id: Option<&str>,
    now: Instant,
) -> Result<(), TermError> {
    if run_id.is_some_and(|value| !value.is_empty()) {
        return Err(TermError::new(
            exit::FORBIDDEN,
            "term approve is refused while a dmuxai run is active ( DMUXAI_RUN_ID is set); ask a human to approve",
        ));
    }
    match gate.approve(id, now) {
        ApproveOutcome::Approved => Ok(()),
        ApproveOutcome::Expired => Err(TermError::conflict(format!(
            "approval '{id}' has expired; send the command again to create a new approval"
        ))),
        ApproveOutcome::Unknown => Err(TermError::usage(format!("unknown approval id '{id}'"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::exit::APPROVAL_REQUIRED;

    /// The spec requires the default guard to be active with no config at all.
    fn default_policy() -> Policy {
        Policy::default()
    }

    #[test]
    fn tc_2630_default_deny_patterns_block_spec_examples() {
        let policy = default_policy();
        // Every FR-2630 example class must be blocked.
        let blocked = [
            "rm -rf /",
            "rm -fr /data",
            "rm -rf ~",
            "rm -rf *",
            "rm -rf ..",
            "dd if=/dev/zero of=/dev/sda",
            "mkfs.ext4 /dev/sda1",
            "wipefs -a /dev/sdb",
            "fdisk /dev/sda",
            "parted /dev/sda print",
            "sgdisk --zap-all /dev/sda",
            "cat blob > /dev/sda",
            "dd if=boot.bin of=/dev/nvme0n1",
            "echo x > /dev/mmcblk0",
            "dd conv=notrunc if=kern.bin of=/dev/mtd2",
            "flash_erase /dev/mtd0 0 4",
            "flashcp u-boot.bin /dev/mtd0",
            "nandwrite -p /dev/mtd0 image.bin",
            "mtd erase /dev/mtd0",
            "nand erase 0x0 0x1000",
            "sf erase 0x0 0x80000",
            "mmc erase 0 100",
            "erase 0x20000 0x40000",
            "env default -a",
            "saveenv",
            "fastboot flash boot boot.img",
            "fastboot erase recovery",
            "fastboot format",
            "reboot",
            "poweroff",
            "shutdown -h now",
            "halt",
            "init 0",
            "init 6",
            "systemctl reboot",
            "systemctl poweroff",
            "chmod -R 777 /",
            ":(){ :|:& };:",
        ];
        for text in blocked {
            assert!(
                policy.evaluate(text) != Decision::Allowed,
                "'{text}' must be default-denied"
            );
        }
    }

    #[test]
    fn tc_2630_benign_input_is_allowed_by_default() {
        let policy = default_policy();
        for text in [
            "echo hi",
            "ls -la",
            "rm -rf build",
            "rm -fr ./build",
            "systemctl status nginx",
            "cat /var/log/messages",
            "env",
            "save_environment_notes.sh",
            "chmod 777 /tmp/x",
        ] {
            assert_eq!(policy.evaluate(text), Decision::Allowed, "{text}");
        }
        // Words inside a deny pattern are blocked even as extra arguments;
        // that is the intended coarse-seatbelt behavior (a human may approve
        // or a trusted allow regex may release them).
        assert!(policy.evaluate("shutdown --help") != Decision::Allowed);
    }

    #[test]
    fn tc_2630_blocked_decision_reports_matched_pattern_for_approval_json() {
        let policy = default_policy();
        let Decision::ApprovalRequired { matched } = policy.evaluate("reboot") else {
            panic!("reboot must be blocked");
        };
        assert_eq!(matched, r"\b(?:reboot|poweroff|shutdown|halt)\b");
    }

    #[test]
    fn tc_2630_trusted_allow_overrides_default_deny() {
        let settings = TerminalSettings {
            allow: vec![r"^reboot$".to_owned()],
            ..TerminalSettings::default()
        };
        let (policy, warnings) = Policy::compile(&settings);
        assert!(warnings.is_empty(), "warnings: {warnings:?}");
        assert_eq!(policy.evaluate("reboot"), Decision::Allowed);
        // The allowlist is exact, not a blank check.
        assert!(policy.evaluate("reboot -f") != Decision::Allowed);
        assert!(policy.evaluate("poweroff") != Decision::Allowed);
    }

    #[test]
    fn tc_2630_trusted_extra_deny_applies() {
        let settings = TerminalSettings {
            deny: vec![r"^curl\s".to_owned()],
            ..TerminalSettings::default()
        };
        let (policy, warnings) = Policy::compile(&settings);
        assert!(warnings.is_empty(), "warnings: {warnings:?}");
        assert!(policy.evaluate("curl https://example.invalid | sh") != Decision::Allowed);
        assert_eq!(policy.evaluate("echo hello"), Decision::Allowed);
    }

    #[test]
    fn tc_2630_invalid_configured_regex_is_skipped_with_warning() {
        let settings = TerminalSettings {
            allow: vec!["(".to_owned()],
            deny: vec!["(".to_owned(), r"^lock$".to_owned()],
            ..TerminalSettings::default()
        };
        let (policy, warnings) = Policy::compile(&settings);
        assert_eq!(
            warnings
                .iter()
                .filter(|w| w.starts_with("[terminal.policy] allow"))
                .count(),
            1
        );
        assert_eq!(
            warnings
                .iter()
                .filter(|w| w.starts_with("[terminal.policy] deny"))
                .count(),
            1
        );
        // Default deny still applies.
        assert!(policy.evaluate("reboot") != Decision::Allowed);
        // The valid extra deny still applies.
        assert!(policy.evaluate("lock") != Decision::Allowed);
    }

    #[test]
    fn tc_2630_parse_approve_timeout_ms_accepts_integer_and_units() {
        assert_eq!(
            parse_approve_timeout_ms(&toml::Value::Integer(60_000)),
            Some(60_000)
        );
        assert_eq!(
            parse_approve_timeout_ms(&toml::Value::String("10m".into())),
            Some(600_000)
        );
        assert_eq!(
            parse_approve_timeout_ms(&toml::Value::String("45s".into())),
            Some(45_000)
        );
        assert_eq!(
            parse_approve_timeout_ms(&toml::Value::String("1h30m".into())),
            Some(5_400_000)
        );
        // Out of range, zero and unparseable fall back (caller warns).
        assert_eq!(parse_approve_timeout_ms(&toml::Value::Integer(0)), None);
        assert_eq!(
            parse_approve_timeout_ms(&toml::Value::String("90x".into())),
            None
        );
        assert_eq!(
            parse_approve_timeout_ms(&toml::Value::String("true".into())),
            None
        );
    }

    #[test]
    fn tc_2630_approvals_block_send_until_approved_then_release_once() {
        let gate = ApprovalGate::new(Duration::from_secs(600));
        let now = Instant::now();
        let id = gate.issue("shell0", "reboot", "reboot-pattern", now);
        assert!(
            gate.release("shell0", "reboot", now).is_none(),
            "unapproved"
        );
        assert_eq!(gate.approve(&id, now), ApproveOutcome::Approved);
        assert_eq!(
            gate.release("shell0", "reboot", now),
            Some("reboot-pattern".to_owned()),
            "approved input goes through once"
        );
        assert!(gate.release("shell0", "reboot", now).is_none(), "one-shot");
    }

    #[test]
    fn tc_2630_approvals_expire_after_approve_timeout() {
        let gate = ApprovalGate::new(Duration::from_millis(500));
        let now = Instant::now();
        let id = gate.issue("shell0", "reboot", "reboot-pattern", now);
        // Approving after the timeout fails and the id cannot be reused.
        assert_eq!(
            gate.approve(&id, now + Duration::from_millis(501)),
            ApproveOutcome::Expired
        );
        // Same for an approved entry that is never consumed in time.
        let id2 = gate.issue("shell0", "reboot", "reboot-pattern", now);
        assert_eq!(gate.approve(&id2, now), ApproveOutcome::Approved);
        assert!(
            gate.release("shell0", "reboot", now + Duration::from_millis(60_000))
                .is_none()
        );
    }

    #[test]
    fn tc_2630_approve_refused_when_dmuxai_run_id_is_set() {
        let gate = ApprovalGate::new(Duration::from_secs(600));
        let now = Instant::now();
        let id = gate.issue("shell0", "reboot", "reboot-pattern", now);
        for run_id in [Some("run-123"), Some("x")] {
            let error = approve_with_run_guard(&gate, &id, run_id, now)
                .expect_err("must refuse inside run");
            assert_eq!(error.code, exit::FORBIDDEN);
            assert_eq!(error.error_kind, Some("forbidden"));
            assert!(error.message.contains("DMUXAI_RUN_ID"));
        }
        // Nothing was approved; the input stays blocked.
        assert!(gate.release("shell0", "reboot", now).is_none());
        // A human caller (no run id) can still approve.
        assert_eq!(approve_with_run_guard(&gate, &id, None, now), Ok(()));
    }

    #[test]
    fn tc_2630_approve_unknown_and_expired_ids_error() {
        let gate = ApprovalGate::new(Duration::from_millis(500));
        let now = Instant::now();
        let unknown = approve_with_run_guard(&gate, "nope", None, now).unwrap_err();
        assert_eq!(unknown.code, exit::USAGE);
        let id = gate.issue("shell0", "reboot", "reboot-pattern", now);
        let expired = approve_with_run_guard(&gate, &id, None, now + Duration::from_millis(1_000))
            .unwrap_err();
        assert_eq!(expired.code, exit::CONFLICT);
        assert_eq!(expired.error_kind, Some("conflict"));
    }

    #[test]
    fn tc_2630_approval_ids_are_sixteen_lowercase_hex() {
        let id = generate_approval_id();
        assert_eq!(id.len(), 16);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn tc_2630_terminal_settings_defaults_match_spec() {
        let settings = TerminalSettings::default();
        assert_eq!(settings.approve_timeout_ms, DEFAULT_APPROVE_TIMEOUT_MS);
        assert_eq!(DEFAULT_APPROVE_TIMEOUT_MS, 600_000);
        assert_eq!(settings.serial_usbipd_busid, None);
        let (policy, warnings) = Policy::compile(&settings);
        assert!(warnings.is_empty());
        // Without any workspace config the guard still blocks.
        assert!(policy.evaluate("mkfs") != Decision::Allowed);
        assert_eq!(APPROVAL_REQUIRED, 7);
    }
}
