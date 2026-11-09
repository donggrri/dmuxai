use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::term::{SessionKind, TermError};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RequestOp {
    Open {
        name: String,
        kind: SessionKind,
        cwd: Option<String>,
        #[serde(default)]
        env: Vec<(String, String)>,
        cols: u16,
        rows: u16,
        #[serde(default)]
        argv: Vec<String>,
    },
    Send {
        name: String,
        text: String,
        #[serde(default)]
        newline: bool,
        #[serde(default)]
        secret: bool,
    },
    SendKeys {
        name: String,
        keys: String,
    },
    Read {
        name: String,
        #[serde(default)]
        since: u64,
        tail: Option<usize>,
        #[serde(default)]
        raw: bool,
    },
    Wait {
        name: String,
        regex: Option<String>,
        idle_ms: Option<u64>,
        timeout_ms: Option<u64>,
        #[serde(default)]
        since: u64,
    },
    Exec {
        name: String,
        command: String,
        timeout_ms: Option<u64>,
    },
    List,
    Show {
        name: String,
    },
    Resize {
        name: String,
        cols: u16,
        rows: u16,
    },
    Close {
        name: Option<String>,
        #[serde(default)]
        force: bool,
        #[serde(default)]
        all: bool,
    },
    Reopen {
        name: String,
    },
    Takeover {
        name: String,
    },
    Release {
        name: String,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    #[serde(flatten)]
    pub op: RequestOp,
}

impl Request {
    pub fn new(op: RequestOp) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            op,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl Response {
    pub fn ok(data: Value) -> Self {
        Self {
            ok: true,
            error_kind: None,
            error: None,
            exit_code: None,
            data: Some(data),
        }
    }

    pub fn with_exit_code(data: Value, exit_code: i32) -> Self {
        Self {
            ok: true,
            error_kind: None,
            error: None,
            exit_code: Some(exit_code),
            data: Some(data),
        }
    }

    pub fn error(error: &TermError) -> Self {
        Self {
            ok: false,
            error_kind: error.error_kind.map(str::to_owned),
            error: Some(error.message.clone()),
            exit_code: Some(error.code),
            data: None,
        }
    }
}

pub fn write_request<W: Write>(writer: &mut W, request: &Request) -> io::Result<()> {
    write_json(writer, request)
}

pub fn read_request<R: Read>(reader: &mut R) -> io::Result<Request> {
    read_json(reader)
}

pub fn write_response<W: Write>(writer: &mut W, response: &Response) -> io::Result<()> {
    write_json(writer, response)
}

pub fn read_response<R: Read>(reader: &mut R) -> io::Result<Response> {
    read_json(reader)
}

fn write_json<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "term protocol message is too large",
        ));
    }
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "term message is too large"))?;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

fn read_json<R: Read, T: for<'de> Deserialize<'de>>(reader: &mut R) -> io::Result<T> {
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes)?;
    let len = u32::from_be_bytes(len_bytes) as usize;
    if len > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "term protocol message is too large",
        ));
    }
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tc_2601_protocol_round_trip_is_length_prefixed_json_v1() {
        let request = Request::new(RequestOp::Send {
            name: "a".to_owned(),
            text: "hi".to_owned(),
            newline: true,
            secret: false,
        });
        let mut bytes = Vec::new();
        write_request(&mut bytes, &request).unwrap();
        assert_eq!(
            u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize,
            bytes.len() - 4
        );
        let decoded = read_request(&mut bytes.as_slice()).unwrap();
        assert_eq!(decoded, request);

        let response = Response::ok(serde_json::json!({"seq": 3}));
        let mut bytes = Vec::new();
        write_response(&mut bytes, &response).unwrap();
        assert_eq!(read_response(&mut bytes.as_slice()).unwrap(), response);
    }
}
