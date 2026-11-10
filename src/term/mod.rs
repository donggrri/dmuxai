use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod backend;
pub mod buffer;
pub mod client;
pub mod host;
pub mod journal;
pub mod policy;
pub mod proto;
#[cfg(unix)]
pub mod serial;
pub mod wait;

pub use crate::agent_api::exit;

pub const MAX_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_NAME_LEN: usize = 32;
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;
pub const MIN_DIMENSION: u16 = 1;
pub const MAX_DIMENSION: u16 = 1000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    #[default]
    Shell,
    Cmd,
    Serial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Starting,
    Open,
    Exited,
    Closed,
    Lost,
}

impl SessionState {
    pub fn is_terminal(self) -> bool {
        matches!(self, SessionState::Exited | SessionState::Closed)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitStatus {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

impl ExitStatus {
    pub fn from_code(code: i32) -> Self {
        Self {
            code: Some(code),
            signal: None,
        }
    }

    pub fn from_signal(signal: i32) -> Self {
        Self {
            code: None,
            signal: Some(signal),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SerialParity {
    #[default]
    None,
    Even,
    Odd,
}

impl SerialParity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Even => "even",
            Self::Odd => "odd",
        }
    }
}

impl std::str::FromStr for SerialParity {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "even" => Ok(Self::Even),
            "odd" => Ok(Self::Odd),
            other => Err(format!(
                "invalid parity '{other}' (expected none, even or odd)"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SerialFlow {
    #[default]
    None,
    RtsCts,
}

impl SerialFlow {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::RtsCts => "rtscts",
        }
    }
}

impl std::str::FromStr for SerialFlow {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "rtscts" | "rts/cts" | "rts-cts" => Ok(Self::RtsCts),
            other => Err(format!(
                "invalid flow control '{other}' (expected none or rtscts)"
            )),
        }
    }
}

fn default_data_bits() -> u8 {
    8
}

fn default_stop_bits() -> u8 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerialConfig {
    pub device: PathBuf,
    pub baud: u32,
    #[serde(default)]
    pub parity: SerialParity,
    #[serde(default = "default_data_bits")]
    pub data: u8,
    #[serde(default = "default_stop_bits")]
    pub stop: u8,
    #[serde(default)]
    pub flow: SerialFlow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usbipd_busid: Option<String>,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            device: PathBuf::new(),
            baud: 115_200,
            parity: SerialParity::None,
            data: default_data_bits(),
            stop: default_stop_bits(),
            flow: SerialFlow::None,
            usbipd_busid: None,
        }
    }
}

impl SerialConfig {
    pub fn new(device: impl Into<PathBuf>, baud: u32) -> Self {
        Self {
            device: device.into(),
            baud,
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(), TermError> {
        if self.device.as_os_str().is_empty() {
            return Err(TermError::usage("--serial needs a device path"));
        }
        if self.baud == 0 {
            return Err(TermError::usage("--baud must be greater than zero"));
        }
        if !matches!(self.data, 5..=8) {
            return Err(TermError::usage("--data must be 5, 6, 7 or 8"));
        }
        if !matches!(self.stop, 1 | 2) {
            return Err(TermError::usage("--stop must be 1 or 2"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSpec {
    pub name: String,
    pub kind: SessionKind,
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<SerialConfig>,
}

impl SessionSpec {
    pub fn shell(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: SessionKind::Shell,
            cwd: None,
            env: Vec::new(),
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            argv: Vec::new(),
            serial: None,
        }
    }

    pub fn serial(name: impl Into<String>, config: SerialConfig) -> Self {
        Self {
            kind: SessionKind::Serial,
            serial: Some(config),
            ..Self::shell(name)
        }
    }

    pub fn cmd(name: impl Into<String>, argv: Vec<String>) -> Self {
        Self {
            kind: SessionKind::Cmd,
            argv,
            ..Self::shell(name)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub name: String,
    pub kind: SessionKind,
    pub state: SessionState,
    pub pid: Option<u32>,
    pub created_ms: u64,
    pub last_output_ms: u64,
    pub cols: u16,
    pub rows: u16,
    pub takeover: bool,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub journal_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermError {
    pub code: i32,
    pub message: String,
    pub error_kind: Option<&'static str>,
    /// Optional structured fields merged into `to_json()` (for example the
    /// approval id and matched pattern of an FR-2630 refusal).
    pub data: Option<Value>,
}

impl TermError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            error_kind: error_kind_for_exit(code),
            data: None,
        }
    }

    /// FR-2630: a dangerous command needs one-shot human approval.
    pub fn approval_required(session: &str, matched: &str, approval_id: &str) -> Self {
        Self {
            code: exit::APPROVAL_REQUIRED,
            message: format!(
                "input blocked by terminal policy (matched '{matched}'); ask a human to run `dmuxai term approve {approval_id}`"
            ),
            error_kind: Some("approval_required"),
            data: Some(json!({
                "approval_id": approval_id,
                "matched": matched,
                "session": session,
            })),
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(exit::USAGE, message)
    }

    pub fn not_found(name: &str) -> Self {
        Self::new(exit::NOT_FOUND, format!("session '{name}' not found"))
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(exit::CONFLICT, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(exit::INTERNAL, message)
    }

    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "ok": false,
            "exit_code": self.code,
            "error": self.message,
            "error_kind": self.error_kind,
        });
        if let (Some(map), Some(Value::Object(extra))) = (value.as_object_mut(), &self.data) {
            for (key, item) in extra {
                map.insert(key.clone(), item.clone());
            }
        }
        value
    }
}

pub fn error_kind_for_exit(code: i32) -> Option<&'static str> {
    match code {
        exit::USAGE => Some("usage"),
        exit::NOT_FOUND => Some("not_found"),
        exit::CONFLICT => Some("conflict"),
        exit::FORBIDDEN => Some("forbidden"),
        exit::INTERNAL => Some("internal"),
        exit::APPROVAL_REQUIRED => Some("approval_required"),
        _ => None,
    }
}

impl std::fmt::Display for TermError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TermError {}

impl From<TermError> for crate::agent_api::ApiError {
    fn from(error: TermError) -> Self {
        Self::new(error.code, error.message)
    }
}

pub fn validate_name(name: &str) -> Result<(), TermError> {
    let bytes = name.as_bytes();
    let valid_len = !bytes.is_empty() && bytes.len() <= MAX_NAME_LEN;
    let valid_first = bytes
        .first()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    let valid_rest = bytes
        .iter()
        .skip(1)
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-');
    if valid_len && valid_first && valid_rest {
        Ok(())
    } else {
        Err(TermError::usage(format!(
            "invalid session name '{name}': expected [a-z0-9][a-z0-9-]{{0,31}}"
        )))
    }
}

pub fn validate_dimensions(cols: u16, rows: u16) -> Result<(), TermError> {
    if (MIN_DIMENSION..=MAX_DIMENSION).contains(&cols)
        && (MIN_DIMENSION..=MAX_DIMENSION).contains(&rows)
    {
        Ok(())
    } else {
        Err(TermError::usage(format!(
            "invalid size {cols}x{rows}: expected cols and rows in {MIN_DIMENSION}..={MAX_DIMENSION}"
        )))
    }
}

pub fn ensure_input_within_limit(len: usize) -> Result<(), TermError> {
    if len <= MAX_INPUT_BYTES {
        Ok(())
    } else {
        Err(TermError::usage(format!(
            "input is {len} bytes; the per-call limit is {MAX_INPUT_BYTES} bytes"
        )))
    }
}

pub fn check_input_size(input: &[u8]) -> Result<(), TermError> {
    ensure_input_within_limit(input.len())
}

pub fn key_bytes(key: &str) -> Result<Vec<u8>, TermError> {
    if let Some(letter) = control_letter(key) {
        return Ok(vec![letter]);
    }

    let bytes: &[u8] = match key.to_ascii_lowercase().as_str() {
        "enter" => b"\r",
        "tab" => b"\t",
        "esc" => b"\x1b",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "backspace" => b"\x7f",
        "space" => b" ",
        "f1" => b"\x1bOP",
        "f2" => b"\x1bOQ",
        "f3" => b"\x1bOR",
        "f4" => b"\x1bOS",
        "f5" => b"\x1b[15~",
        "f6" => b"\x1b[17~",
        "f7" => b"\x1b[18~",
        "f8" => b"\x1b[19~",
        "f9" => b"\x1b[20~",
        "f10" => b"\x1b[21~",
        "f11" => b"\x1b[23~",
        "f12" => b"\x1b[24~",
        _ => return Err(TermError::usage(format!("unknown key '{key}'"))),
    };
    Ok(bytes.to_vec())
}

pub fn parse_key_list(keys: &str) -> Result<Vec<u8>, TermError> {
    let mut out = Vec::new();
    for key in keys.split(',').filter(|key| !key.is_empty()) {
        out.extend(key_bytes(key)?);
    }
    ensure_input_within_limit(out.len())?;
    Ok(out)
}

fn control_letter(key: &str) -> Option<u8> {
    let rest = key.strip_prefix("C-").or_else(|| key.strip_prefix("c-"))?;
    let bytes = rest.as_bytes();
    if bytes.len() != 1 || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    Some(bytes[0].to_ascii_lowercase() - b'a' + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tc_2602_key_table_maps_named_and_control_keys() {
        assert_eq!(key_bytes("C-c").unwrap(), vec![0x03]);
        assert_eq!(key_bytes("C-a").unwrap(), vec![0x01]);
        assert_eq!(key_bytes("C-z").unwrap(), vec![0x1a]);
        assert_eq!(key_bytes("Enter").unwrap(), b"\r");
        assert_eq!(key_bytes("Tab").unwrap(), b"\t");
        assert_eq!(key_bytes("Esc").unwrap(), b"\x1b");
        assert_eq!(key_bytes("Up").unwrap(), b"\x1b[A");
        assert_eq!(key_bytes("Down").unwrap(), b"\x1b[B");
        assert_eq!(key_bytes("Right").unwrap(), b"\x1b[C");
        assert_eq!(key_bytes("Left").unwrap(), b"\x1b[D");
        assert_eq!(key_bytes("Backspace").unwrap(), b"\x7f");
        assert_eq!(key_bytes("Space").unwrap(), b" ");
        assert_eq!(key_bytes("F1").unwrap(), b"\x1bOP");
        assert_eq!(key_bytes("F12").unwrap(), b"\x1b[24~");
        assert_eq!(parse_key_list("C-c,Enter").unwrap(), b"\x03\r");
    }

    #[test]
    fn tc_2602_unknown_key_is_a_usage_error_naming_it() {
        let error = key_bytes("Nope").unwrap_err();
        assert_eq!(error.code, exit::USAGE);
        assert!(error.message.contains("Nope"));
        assert!(parse_key_list("Enter,Nope").is_err());
    }

    #[test]
    fn tc_2602_input_limit_is_64_kib_per_call() {
        assert!(ensure_input_within_limit(MAX_INPUT_BYTES).is_ok());
        assert!(check_input_size(&vec![b'x'; MAX_INPUT_BYTES]).is_ok());
        let error = ensure_input_within_limit(MAX_INPUT_BYTES + 1).unwrap_err();
        assert_eq!(error.code, exit::USAGE);
        assert!(check_input_size(&vec![b'x'; MAX_INPUT_BYTES + 1]).is_err());
    }

    #[test]
    fn tc_2602_name_check_accepts_only_lowercase_names() {
        for name in [
            "a",
            "a0",
            "test-node-1",
            "0abc",
            "abcdefghijklmnopqrstuvwxyz012345",
        ] {
            assert!(validate_name(name).is_ok(), "{name}");
        }
        for name in [
            "",
            "A",
            "Bad_Name",
            "-a",
            "a b",
            "abcdefghijklmnopqrstuvwxyz0123456",
        ] {
            let error = validate_name(name).unwrap_err();
            assert_eq!(error.code, exit::USAGE, "{name}");
        }
    }

    #[test]
    fn tc_2602_term_errors_carry_exit_codes_and_kinds() {
        assert_eq!(TermError::usage("x").code, exit::USAGE);
        let missing = TermError::not_found("nope");
        assert_eq!(missing.code, exit::NOT_FOUND);
        assert_eq!(missing.error_kind, Some("not_found"));
        assert_eq!(missing.to_json()["error_kind"], "not_found");
        assert_eq!(TermError::conflict("busy").code, exit::CONFLICT);
        assert_eq!(TermError::internal("bad").code, exit::INTERNAL);
    }
}
