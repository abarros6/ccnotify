//! Minimal HTTP/1.1 client over TcpStream for talking to the wrapper's
//! loopback server. Deliberately tiny: we control both ends, always use
//! Connection: close, and never need TLS, redirects, or chunked requests.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub fn request(
    port: u16,
    token: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<String, String> {
    let mut stream =
        TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("connect: {e}"))?;
    // Long-poll holds up to 25s server-side; leave headroom.
    stream
        .set_read_timeout(Some(Duration::from_secs(40)))
        .map_err(|e| e.to_string())?;

    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         X-CCNotify-Token: {token}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write: {e}"))?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("read: {e}"))?;
    let raw = String::from_utf8_lossy(&raw);

    let (head, response_body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed http response".to_string())?;
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "malformed status line".to_string())?;
    if !(200..300).contains(&status) {
        return Err(format!("http {status}: {response_body}"));
    }
    Ok(response_body.to_string())
}
