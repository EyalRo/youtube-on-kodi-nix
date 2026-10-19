//! Minimal HTTP/1.0 client for mediad on localhost (no TLS, no chunking, JSON bodies).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("cannot reach mediad at {addr}: {source}")]
    Connect {
        addr: String,
        source: std::io::Error,
    },
    #[error("mediad connection failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("mediad answered {status}: {body}")]
    Status { status: u16, body: String },
    #[error("mediad sent an unreadable reply")]
    BadReply,
}

fn request(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, String), HttpError> {
    let mut s = TcpStream::connect(addr).map_err(|source| HttpError::Connect {
        addr: addr.into(),
        source,
    })?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    s.set_write_timeout(Some(Duration::from_secs(10)))?;
    let payload = body.map(Value::to_string).unwrap_or_default();
    let ctype = if body.is_some() {
        "Content-Type: application/json\r\n"
    } else {
        ""
    };
    write!(
        s,
        "{method} {path} HTTP/1.0\r\nHost: {addr}\r\n{ctype}Content-Length: {}\r\n\r\n{payload}",
        payload.len()
    )?;
    let mut reply = Vec::new();
    s.read_to_end(&mut reply)?;
    let text = String::from_utf8_lossy(&reply);
    let (head, body) = text.split_once("\r\n\r\n").ok_or(HttpError::BadReply)?;
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .ok_or(HttpError::BadReply)?;
    Ok((status, body.to_string()))
}

pub fn get_json(addr: &str, path: &str) -> Result<Value, HttpError> {
    let (status, body) = request(addr, "GET", path, None)?;
    if status != 200 {
        return Err(HttpError::Status { status, body });
    }
    serde_json::from_str(&body).map_err(|_| HttpError::BadReply)
}

pub fn post_json(addr: &str, path: &str, body: &Value) -> Result<(), HttpError> {
    let (status, text) = request(addr, "POST", path, Some(body))?;
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(HttpError::Status { status, body: text })
    }
}
