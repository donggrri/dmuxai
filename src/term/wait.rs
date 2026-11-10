use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::run_journal::strip_ansi_and_control;
use crate::term::buffer::{BufferRead, RingBuffer};
use crate::term::{ExitStatus, TermError};

pub const MARKER_PREFIX: &str = "__DMUXAI_";

static CSI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[\x30-\x3f]*[\x20-\x2f]*[\x40-\x7e]").unwrap());

fn strip_for_match(input: &str) -> String {
    strip_ansi_and_control(&CSI_RE.replace_all(input, ""))
}

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

    let start_seq = buffer.seq();
    let started = Instant::now();
    let deadline = started + options.timeout;
    let mut inner = buffer.lock();
    loop {
        let mut output = inner.read_since(options.since, true);
        output.text = strip_for_match(&output.text);
        if let Some(regex) = &regex
            && regex.is_match(&output.text)
        {
            return Ok(result(WaitReason::Match, output, inner.exit));
        }
        if inner.exit.is_some() {
            return Ok(result(WaitReason::Exited, output, inner.exit));
        }

        let now = Instant::now();
        let idle_deadline = options.idle.map(|idle| {
            let idle_start = if output.seq_to > start_seq {
                inner.last_output.unwrap_or(started)
            } else {
                started
            };
            idle_start + idle
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

/// FR-2616: the `exec` reply text is exactly the command output. The marker
/// command shares the sent line, so the buffer is `echoed-line`, then the raw
/// output, then the numbered marker; text is sliced between them and the
/// trailing newline of the marker print is trimmed away.
pub fn exec_output_text(full: &str, command: &str, nonce: &str) -> String {
    let token = format!("{MARKER_PREFIX}{nonce}_%s__");
    let echo_line = format!("{command}; printf '\\n{token}\\n' \"$?\"");
    let body_start = full
        .find(&echo_line)
        .and_then(|pos| line_end(full, pos + echo_line.len()));
    let marker = marker_start(full, nonce);

    match (body_start, marker) {
        (Some(start), Some(end)) if end >= start => {
            strip_leading_prompt(full[start..end].trim_end()).to_owned()
        }
        (Some(start), _) => {
            let text = strip_exit_marker(&full[start..], nonce);
            strip_leading_prompt(&text).to_owned()
        }
        (None, Some(end)) => {
            let text = strip_exit_marker(&full[..end], nonce);
            strip_leading_prompt(text.trim_end()).to_owned()
        }
        (None, None) => {
            let text = strip_exit_marker(full, nonce);
            strip_leading_prompt(&text).to_owned()
        }
    }
}

fn marker_start(full: &str, nonce: &str) -> Option<usize> {
    let prefix = format!("{MARKER_PREFIX}{nonce}_");
    full.match_indices(&prefix)
        .find(|(pos, _)| {
            let rest = &full[pos + prefix.len()..];
            let digits = rest
                .chars()
                .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
                .count();
            digits > 0 && rest[digits..].starts_with("__")
        })
        .map(|(pos, _)| pos)
}

fn line_end(full: &str, from: usize) -> Option<usize> {
    let rest = &full[from..];
    let offset = rest.find(['\r', '\n'])?;
    let mut end = from + offset;
    if full[end..].starts_with("\r\n") {
        end += 2;
    } else {
        end += 1;
    }
    Some(end)
}

fn strip_leading_prompt(text: &str) -> &str {
    for prompt in ["$ ", "# ", "> "] {
        if let Some(rest) = text.strip_prefix(prompt) {
            return rest;
        }
    }
    text
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
    fn tc_2622_match_accumulates_prompt_across_reset_and_space() {
        let buffer = RingBuffer::new(1024);
        buffer.push_str("#");
        buffer.push_str("\x1b[0m ");
        let result = wait(
            &buffer,
            &options_match(r"[#\$] ", Duration::from_millis(30)),
        )
        .unwrap();
        assert_eq!(result.reason, WaitReason::Match);
        assert!(result.text.contains("# "), "text: {:?}", result.text);
    }

    #[test]
    fn tc_2622_match_strips_private_mode_csi_between_hash_and_space() {
        let buffer = RingBuffer::new(1024);
        buffer.push_str("#\x1b[?2004h ");
        let result = wait(
            &buffer,
            &options_match(r"[#\$] ", Duration::from_millis(30)),
        )
        .unwrap();
        assert_eq!(result.reason, WaitReason::Match);
        assert!(result.text.contains("# "), "text: {:?}", result.text);
        let raw = buffer.read_since(0, true);
        assert!(raw.text.contains('\x1b'), "raw text: {:?}", raw.text);
        assert!(raw.text.contains("?2004h"), "raw text: {:?}", raw.text);
    }

    #[test]
    fn tc_2622_match_handles_cr_wrapped_prompt() {
        let buffer = RingBuffer::new(1024);
        buffer.push_str("\r# \r\n");
        let result = wait(
            &buffer,
            &options_match(r"[#\$] ", Duration::from_millis(30)),
        )
        .unwrap();
        assert_eq!(result.reason, WaitReason::Match);
    }

    #[test]
    fn tc_2622_prompt_before_since_still_times_out() {
        let buffer = RingBuffer::new(1024);
        buffer.push_str("# ");
        let since = buffer.seq();
        let result = wait(
            &buffer,
            &WaitOptions {
                regex: Some(r"[#\$] ".to_owned()),
                idle: None,
                timeout: Duration::from_millis(30),
                since,
            },
        )
        .unwrap();
        assert_eq!(result.reason, WaitReason::Timeout);
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
    fn tc_2616_exec_output_text_is_exactly_the_command_output() {
        let nonce = "01ab40d3";
        let token = format!("{MARKER_PREFIX}{nonce}_%s__");
        let marker_args = format!("printf '\\n{token}\\n' \"$?\"");

        // Plain output plus shell prompt before and after: text is exactly `x`.
        let full = format!(
            "$ printf x; {marker_args}\r\nx\r\n{}\r\n$ ",
            exit_marker(nonce, 0)
        );
        let text = exec_output_text(&full, "printf x", nonce);
        assert_eq!(text, "x", "plain output: {text:?}");
        assert!(!text.contains("__DMUXAI_"), "marker leaked: {text:?}");
        assert!(!text.contains("printf x"), "command echo leaked: {text:?}");
        assert!(!text.contains('$'), "prompt leaked: {text:?}");

        // Multi-line output keeps its interior newline.
        let full = format!(
            "$ echo hi; {marker_args}\r\nhi\r\nbye\r\n\r\n{}\r\n$ ",
            exit_marker(nonce, 1)
        );
        assert_eq!(exec_output_text(&full, "echo hi", nonce), "hi\r\nbye");

        // Empty output stays empty.
        let full = format!(
            "$ true; {marker_args}\r\n\r\n{}\r\n$ ",
            exit_marker(nonce, 0)
        );
        assert_eq!(exec_output_text(&full, "true", nonce), "");

        // Older output in the window does not fool the echo slice: the slice
        // anchors on the echoed command line, not the first text occurrence.
        let full = format!(
            "$ echo hi\r\nhi\r\n$ echo hi; {marker_args}\r\nhi\r\n\r\n{}\r\n$ ",
            exit_marker(nonce, 0)
        );
        assert_eq!(exec_output_text(&full, "echo hi", nonce), "hi");
    }

    #[test]
    fn tc_2616_exec_output_text_falls_back_on_timeout_and_missing_echo() {
        let nonce = "0badc0de";
        let token = format!("{MARKER_PREFIX}{nonce}_%s__");
        let marker_args = format!("printf '\\n{token}\\n' \"$?\"");

        // Without the numbered marker (timeout) the echo line is stripped but
        // the rest of the window is kept.
        let full = format!("$ true; {marker_args}\r\noutput line\r\n");
        assert_eq!(exec_output_text(&full, "true", nonce), "output line\r\n");

        // Echo disabled: same fallback.
        let no_echo = "some output\nmore\n".to_owned();
        assert_eq!(exec_output_text(&no_echo, "true", nonce), no_echo);
    }

    #[test]
    fn tc_2616_exec_output_text_drops_leading_prompt_when_echo_is_missing() {
        let nonce = "feedbeef";
        let marker = exit_marker(nonce, 0);

        // The echo line is missing: the prompt before the output must not leak.
        let full = format!("$ x\r\n{marker}\r\n");
        assert_eq!(exec_output_text(&full, "printf x", nonce), "x");

        // A CR-wrapped echo line still slices to the command output.
        let token = format!("{MARKER_PREFIX}{nonce}_%s__");
        let marker_args = format!("printf '\\n{token}\\n' \"$?\"");
        let full = format!("$ printf x; {marker_args}\rx\r\n{marker}\r\n");
        assert_eq!(exec_output_text(&full, "printf x", nonce), "x");
    }

    #[test]
    fn tc_2693_idle_wait_uses_notification_without_polling() {
        let buffer = Arc::new(RingBuffer::new(1024));
        let idle = Duration::from_millis(100);
        let (sender, receiver) = mpsc::channel();
        let started = Instant::now();
        let waiter = {
            let buffer = Arc::clone(&buffer);
            thread::spawn(move || {
                let result = wait(
                    &buffer,
                    &WaitOptions {
                        regex: None,
                        idle: Some(idle),
                        timeout: Duration::from_secs(3),
                        since: 0,
                    },
                )
                .unwrap();
                sender.send((result.reason, Instant::now())).unwrap();
            })
        };

        thread::sleep(Duration::from_millis(30));
        assert!(
            receiver.try_recv().is_err(),
            "idle wait must block until the idle duration elapses"
        );

        let (reason, finished) = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(reason, WaitReason::Idle);
        let elapsed = finished.duration_since(started);
        assert!(
            elapsed >= idle,
            "idle fired before the idle duration: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "idle must use a timed notification rather than polling: {elapsed:?}"
        );
        assert_eq!(buffer.seq(), 0, "an idle wait must not need a push");
        waiter.join().unwrap();
    }

    #[test]
    fn tc_2619_idle_succeeds_when_since_is_current_seq() {
        let buffer = RingBuffer::new(1024);
        buffer.push_str("old output\n");
        let since = buffer.seq();
        let started = Instant::now();
        let result = wait(
            &buffer,
            &WaitOptions {
                regex: None,
                idle: Some(Duration::from_millis(100)),
                timeout: Duration::from_secs(3),
                since,
            },
        )
        .unwrap();
        let elapsed = started.elapsed();
        assert_eq!(result.reason, WaitReason::Idle);
        assert_eq!(result.seq_from, since);
        assert_eq!(result.seq_to, since);
        assert!(result.text.is_empty());
        assert!(
            elapsed >= Duration::from_millis(100),
            "idle returned too early: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "idle did not use the wait-start clock: {elapsed:?}"
        );
    }

    #[test]
    fn tc_2619_idle_timer_resets_after_later_output() {
        let buffer = Arc::new(RingBuffer::new(1024));
        let idle = Duration::from_millis(200);
        let (sender, receiver) = mpsc::channel();
        let waiter = {
            let buffer = Arc::clone(&buffer);
            thread::spawn(move || {
                let result = wait(
                    &buffer,
                    &WaitOptions {
                        regex: None,
                        idle: Some(idle),
                        timeout: Duration::from_secs(3),
                        since: 0,
                    },
                )
                .unwrap();
                sender.send((result.reason, Instant::now())).unwrap();
            })
        };

        thread::sleep(Duration::from_millis(50));
        let pushed = Instant::now();
        buffer.push_str("late\n");
        let (reason, finished) = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(reason, WaitReason::Idle);
        let after_push = finished.duration_since(pushed);
        assert!(
            after_push >= Duration::from_millis(170),
            "output must reset the idle clock instead of using the wait start: {after_push:?}"
        );
        waiter.join().unwrap();
    }
}
