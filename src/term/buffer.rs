use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Instant;

use crate::run_journal::strip_ansi_and_control;
use crate::term::ExitStatus;

pub const DEFAULT_CAPACITY: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferRead {
    pub text: String,
    pub seq_from: u64,
    pub seq_to: u64,
    pub dropped_bytes: u64,
}

#[derive(Debug, Clone)]
struct Chunk {
    start: u64,
    data: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct Inner {
    chunks: VecDeque<Chunk>,
    next_seq: u64,
    oldest_seq: u64,
    retained: usize,
    pub(crate) last_output: Option<Instant>,
    pub(crate) exit: Option<ExitStatus>,
}

impl Inner {
    fn new() -> Self {
        Self {
            chunks: VecDeque::new(),
            next_seq: 0,
            oldest_seq: 0,
            retained: 0,
            last_output: None,
            exit: None,
        }
    }

    pub(crate) fn read_since(&self, since: u64, raw: bool) -> BufferRead {
        let request = since.min(self.next_seq);
        let dropped_bytes = self.oldest_seq.saturating_sub(request);
        let seq_from = self.oldest_seq.max(request);
        let mut bytes = Vec::new();
        for chunk in &self.chunks {
            let end = chunk.start + chunk.data.len() as u64;
            if end <= seq_from {
                continue;
            }
            let offset = seq_from.saturating_sub(chunk.start) as usize;
            bytes.extend_from_slice(&chunk.data[offset..]);
        }
        BufferRead {
            text: render(&bytes, raw),
            seq_from,
            seq_to: self.next_seq,
            dropped_bytes,
        }
    }

    pub(crate) fn read_tail(&self, lines: usize, raw: bool) -> BufferRead {
        let mut bytes = Vec::new();
        for chunk in &self.chunks {
            bytes.extend_from_slice(&chunk.data);
        }
        let text = render(&bytes, raw);
        BufferRead {
            text: take_last_lines(&text, lines),
            seq_from: self.oldest_seq,
            seq_to: self.next_seq,
            dropped_bytes: self.oldest_seq,
        }
    }

    fn trim(&mut self, capacity: usize) {
        while self.retained > capacity {
            let excess = self.retained - capacity;
            let (removed, pop) = {
                let front = self.chunks.front_mut().expect("non-empty ring");
                if front.data.len() <= excess {
                    (front.data.len(), true)
                } else {
                    front.data.drain(..excess);
                    front.start += excess as u64;
                    (excess, false)
                }
            };
            if pop {
                let start = self.chunks.pop_front().expect("non-empty ring").start;
                self.oldest_seq = start + removed as u64;
            } else {
                self.oldest_seq = self.chunks.front().expect("non-empty ring").start;
            }
            self.retained -= removed;
        }
        if self.chunks.is_empty() {
            self.oldest_seq = self.next_seq;
        }
    }
}

pub struct RingBuffer {
    inner: Mutex<Inner>,
    condvar: Condvar,
    capacity: usize,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner::new()),
            condvar: Condvar::new(),
            capacity: capacity.max(1),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn push(&self, data: &[u8]) -> u64 {
        let mut inner = self.lock();
        if !data.is_empty() {
            let start = inner.next_seq;
            inner.chunks.push_back(Chunk {
                start,
                data: data.to_vec(),
            });
            inner.next_seq += data.len() as u64;
            inner.retained += data.len();
            inner.last_output = Some(Instant::now());
            inner.trim(self.capacity);
            self.condvar.notify_all();
        }
        inner.next_seq
    }

    pub fn push_str(&self, data: &str) -> u64 {
        self.push(data.as_bytes())
    }

    pub fn seq(&self) -> u64 {
        self.lock().next_seq
    }

    pub fn oldest_seq(&self) -> u64 {
        self.lock().oldest_seq
    }

    pub fn retained_bytes(&self) -> usize {
        self.lock().retained
    }

    pub fn read_since(&self, since: u64, raw: bool) -> BufferRead {
        self.lock().read_since(since, raw)
    }

    pub fn read_tail(&self, lines: usize, raw: bool) -> BufferRead {
        self.lock().read_tail(lines, raw)
    }

    pub fn set_exited(&self, exit: ExitStatus) {
        let mut inner = self.lock();
        if inner.exit.is_none() {
            inner.exit = Some(exit);
            self.condvar.notify_all();
        }
    }

    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.lock().exit
    }

    pub fn last_output_at(&self) -> Option<Instant> {
        self.lock().last_output
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn condvar(&self) -> &Condvar {
        &self.condvar
    }
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

fn render(bytes: &[u8], raw: bool) -> String {
    let text = String::from_utf8_lossy(bytes).into_owned();
    if raw {
        text
    } else {
        strip_ansi_and_control(&text)
    }
}

fn take_last_lines(text: &str, lines: usize) -> String {
    if lines == 0 {
        return String::new();
    }
    let parts: Vec<&str> = text.split_inclusive('\n').collect();
    let start = parts.len().saturating_sub(lines);
    parts[start..].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tc_2603_ring_evicts_old_data_and_reports_dropped_bytes() {
        let buffer = RingBuffer::new(16);
        assert_eq!(buffer.push(b"0123456789"), 10);
        assert_eq!(buffer.seq(), 10);
        assert_eq!(buffer.push(b"abcdefghij"), 20);
        assert_eq!(buffer.seq(), 20);
        assert_eq!(buffer.retained_bytes(), 16);

        let read = buffer.read_since(0, true);
        assert_eq!(read.seq_from, 4);
        assert_eq!(read.seq_to, 20);
        assert_eq!(read.dropped_bytes, 4);
        assert_eq!(read.text, "456789abcdefghij");

        let next = buffer.read_since(read.seq_to, true);
        assert_eq!(next.seq_from, 20);
        assert_eq!(next.seq_to, 20);
        assert_eq!(next.dropped_bytes, 0);
        assert!(next.text.is_empty());
    }

    #[test]
    fn tc_2603_read_since_does_not_duplicate_or_skip_bytes() {
        let buffer = RingBuffer::new(1024);
        buffer.push_str("hello");
        let read = buffer.read_since(2, true);
        assert_eq!(read.text, "llo");
        assert_eq!(read.seq_from, 2);
        assert_eq!(read.seq_to, 5);
        assert_eq!(read.dropped_bytes, 0);

        buffer.push_str(" world");
        let rest = buffer.read_since(read.seq_to, true);
        assert_eq!(rest.text, " world");
        assert_eq!(rest.seq_from, 5);
        assert_eq!(rest.seq_to, 11);
    }

    #[test]
    fn tc_2603_ansi_is_stripped_unless_raw() {
        let buffer = RingBuffer::new(1024);
        buffer.push_str("\x1b[31mred\x1b[0m\n");
        let raw = buffer.read_since(0, true);
        assert!(raw.text.contains('\x1b'));
        assert!(raw.text.contains("red"));
        let clean = buffer.read_since(0, false);
        assert_eq!(clean.text, "red\n");
    }

    #[test]
    fn tc_2603_tail_returns_last_lines() {
        let buffer = RingBuffer::new(1024);
        buffer.push_str("one\ntwo\nthree\n");
        assert_eq!(buffer.read_tail(2, false).text, "two\nthree\n");
        assert_eq!(buffer.read_tail(0, false).text, "");
    }
}
