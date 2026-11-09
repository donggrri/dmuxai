use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::term::buffer::{BufferRead, RingBuffer};
use crate::term::{ExitStatus, TermError};

pub const MARKER_PREFIX: &str = "__DMUXAI_";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaitReason {
    Match,
    Idle,
    Timeout,
    Exited,
    Marker,
}

#[derive(Debug, Clone)]
pub struct WaitOptions {
    pub regex: Option<String>,
    pub idle: Option<Duration>,
    pub timeout: Duration,
    pub since: u64,
}

impl WaitOptions {
    pub fn new(timeout: Duration) -> Self {
        Self {
            regex: None,
            idle: None,
            timeout,
            since: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitResult {
    pub reason: WaitReason,
    pub text: String,
    pub seq_from: u64,
    pub seq_to: u64,
    pub dropped_bytes: u64,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

pub fn wait(buffer: &RingBuffer, options: &WaitOptions) -> Result<WaitResult, TermError> {
    if options.regex.is_none() && options.idle.is_none() {
        return Err(TermError::usage("wait requires --match or --idle"));
    }
    let regex = match &options.regex {
        Some(pattern) => Some(
            Regex::new(pattern)
                .map_err(|error| TermError::usage(format!("invalid regex '{pattern}': {error}")))?,
        ),
        None => None,
    };

    let deadline = Instant::now() + options.timeout;
    let mut inner = buffer.lock();
    loop {
        let output = inner.read_since(options.since, false);
        if let Some(regex) = &regex
            && regex.is_match(&output.text)
        {
            return Ok(result(WaitReason::Match, output, inner.exit));
        }
        if inner.exit.is_some() {
            return Ok(result(WaitReason::Exited, output, inner.exit));
        }

        let now = Instant::now();
        let idle_deadline = options.idle.and_then(|idle| {
            (output.seq_to > options.since)
                .then(|| inner.last_output.map(|last| last + idle))
                .flatten()
        });
        if let Some(idle_deadline) = idle_deadline
            && now >= idle_deadline
        {
            return Ok(result(WaitReason::Idle, output, inner.exit));
        }
        if now >= deadline {
            return Ok(result(WaitReason::Timeout, output, inner.exit));
        }

        let mut wait_for = deadline.saturating_duration_since(now);
        if let Some(idle_deadline) = idle_deadline {
            wait_for = wait_for.min(idle_deadline.saturating_duration_since(now));
        }
        let (next, _) = buffer
            .condvar()
            .wait_timeout(inner, wait_for)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner = next;
    }
}

fn result(reason: WaitReason, output: BufferRead, exit: Option<ExitStatus>) -> WaitResult {
    WaitResult {
        reason,
        text: output.text,
        seq_from: output.seq_from,
        seq_to: output.seq_to,
        dropped_bytes: output.dropped_bytes,
        exit_code: exit.and_then(|status| status.code),
        signal: exit.and_then(|status| status.signal),
    }
}

pub fn exit_marker(nonce: &str, exit_code: i32) -> String {
    format!("{MARKER_PREFIX}{nonce}_{exit_code}__")
}

pub fn marker_command(nonce: &str) -> String {
    format!("printf '\\n{MARKER_PREFIX}{nonce}_%s__\\n' \"$?\"")
}

pub fn parse_exit_marker(text: &str, nonce: &str) -> Option<i32> {
    let token = format!("{MARKER_PREFIX}{nonce}_");
    for line in text.lines() {
        let Some(position) = line.find(&token) else {
            continue;
        };
        let rest = &line[position + token.len()..];
        let digits: String = rest
            .chars()
            .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
            .collect();
        if !digits.is_empty() && rest[digits.len()..].starts_with("__") {
            return digits.parse().ok();
        }
    }
    None
}

pub fn has_exit_marker(text: &str, nonce: &str) -> bool {
    text.contains(&format!("{MARKER_PREFIX}{nonce}_"))
}

pub fn strip_exit_marker(text: &str, nonce: &str) -> String {
    let token = format!("{MARKER_PREFIX}{nonce}_");
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if !line.contains(&token) {
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread;

    fn options_match(pattern: &str, timeout: Duration) -> WaitOptions {
        WaitOptions {
            regex: Some(pattern.to_owned()),
            idle: None,
            timeout,
            since: 0,
        }
    }

    #[test]
    fn tc_2604_match_returns_when_output_arrives() {
        let buffer = Arc::new(RingBuffer::new(1024));
        let waiter = {
            let buffer = Arc::clone(&buffer);
            thread::spawn(move || {
                wait(&buffer, &options_match("done", Duration::from_secs(3))).unwrap()
            })
        };
        thread::sleep(Duration::from_millis(20));
        buffer.push_str("partial\n");
        thread::sleep(Duration::from_millis(20));
        buffer.push_str("done\n");

        let result = waiter.join().unwrap();
        assert_eq!(result.reason, WaitReason::Match);
        assert!(result.text.contains("done"));
        assert_eq!(result.seq_to, buffer.seq());
    }

    #[test]
    fn tc_2604_timeout_returns_tail_without_matching() {
        let buffer = RingBuffer::new(1024);
        buffer.push_str("never\n");
        let result = wait(&buffer, &options_match("done", Duration::from_millis(30))).unwrap();
        assert_eq!(result.reason, WaitReason::Timeout);
        assert!(result.text.contains("never"));
    }

    #[test]
    fn tc_2604_invalid_regex_is_a_usage_error() {
        let buffer = RingBuffer::new(1024);
        let error = wait(&buffer, &options_match("(", Duration::from_millis(30))).unwrap_err();
        assert_eq!(error.code, crate::term::exit::USAGE);
    }

    #[test]
    fn tc_2604_exit_is_reported_before_idle() {
        let buffer = RingBuffer::new(1024);
        buffer.push_str("bye\n");
        buffer.set_exited(ExitStatus::from_code(3));
        let options = WaitOptions {
            regex: None,
            idle: Some(Duration::from_secs(1)),
            timeout: Duration::from_secs(2),
            since: 0,
        };
        let result = wait(&buffer, &options).unwrap();
        assert_eq!(result.reason, WaitReason::Exited);
        assert_eq!(result.exit_code, Some(3));
    }

    #[test]
    fn tc_2605_exit_marker_round_trips_and_is_stripped_with_its_echo() {
        for code in [0, 1, 0, 127] {
            let nonce = "deadbeef";
            let marker = exit_marker(nonce, code);
            assert_eq!(parse_exit_marker(&marker, nonce), Some(code));

            let text = format!(
                "command output {code}\n{}\n{marker}\n",
                marker_command(nonce)
            );
            assert_eq!(
                strip_exit_marker(&text, nonce),
                format!("command output {code}\n")
            );
            assert!(has_exit_marker(&text, nonce));
        }
        assert_eq!(parse_exit_marker("no marker here\n", "deadbeef"), None);
    }

    #[test]
    fn tc_2605_marker_can_complete_a_wait() {
        let buffer = RingBuffer::new(1024);
        let nonce = "cafebabe";
        buffer.push_str(&format!("x\n{}\n", exit_marker(nonce, 0)));
        let result = wait(
            &buffer,
            &options_match(&exit_marker(nonce, 0), Duration::from_millis(30)),
        )
        .unwrap();
        assert_eq!(result.reason, WaitReason::Match);
        assert_eq!(parse_exit_marker(&result.text, nonce), Some(0));
        assert_eq!(strip_exit_marker(&result.text, nonce), "x\n");
    }

    #[test]
    fn tc_2693_idle_wait_uses_notification_without_polling() {
        let buffer = Arc::new(RingBuffer::new(1024));
        let (sender, receiver) = mpsc::channel();
        let waiter = {
            let buffer = Arc::clone(&buffer);
            thread::spawn(move || {
                let result = wait(
                    &buffer,
                    &WaitOptions {
                        regex: None,
                        idle: Some(Duration::from_millis(100)),
                        timeout: Duration::from_secs(3),
                        since: 0,
                    },
                )
                .unwrap();
                sender.send((result.reason, Instant::now())).unwrap();
            })
        };

        thread::sleep(Duration::from_millis(150));
        assert!(
            receiver.try_recv().is_err(),
            "idle wait must not complete before any output arrives"
        );

        let pushed = Instant::now();
        buffer.push_str("hello\n");
        let (reason, finished) = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(reason, WaitReason::Idle);
        assert!(finished.duration_since(pushed) < Duration::from_secs(2));
        waiter.join().unwrap();
    }
}
