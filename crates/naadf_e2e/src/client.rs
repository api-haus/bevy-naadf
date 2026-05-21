//! `BrpClient` — a blocking BRP-over-HTTP JSON-RPC 2.0 client.
//!
//! ## Why raw `TcpStream` and not an HTTP-client crate
//!
//! The design (§7.1, assumption A6) flagged a blocking HTTP client (`ureq`)
//! as the first choice but documented "raw `TcpStream` + manual HTTP/1.1" as
//! the fallback if the client fights the chunked `text/event-stream` bodies
//! the watching verbs return. We take the fallback deliberately — and it turns
//! out to be the *better* call here:
//!
//! - The BRP server is loopback HTTP on `127.0.0.1` — there is no TLS, no
//!   redirect, no proxy, no content negotiation. A general HTTP client is all
//!   overhead.
//! - **The watching verbs do not actually need SSE on the client side.** BRP's
//!   HTTP transport only switches a response to `text/event-stream` when the
//!   *method name contains `+watch`* (`bevy_remote 0.19.0-rc.1` `http.rs:386`).
//!   The `naadf/*` watching verbs are registered under their bare names
//!   (`naadf/run_until_idle`, `naadf/await_capture`) — so the HTTP layer takes
//!   the `Complete` path: it does a single `result_receiver.recv().await` and
//!   replies with one `application/json` body. The watching *handler* still
//!   re-runs every SUT frame and streams `Ok(None)` until its single final
//!   `Ok(Some(..))`; that final value is exactly what the server's lone
//!   `recv()` delivers. Net: every `naadf/*` verb — instant or watching — is
//!   one blocking request / one JSON response from the client's point of view.
//!   No SSE parser, no chunked-transfer decoding.
//! - This keeps `naadf_e2e`'s dependency tree to `serde`/`serde_json`/`image`/
//!   `base64` — no async runtime, no `hyper` tail.
//!
//! `BrpClient` opens a fresh TCP connection per call (`Connection: close`) —
//! simplest correct behaviour, and the runner makes a handful of calls per
//! scenario, not per frame (design D1).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

/// A blocking BRP JSON-RPC client bound to one SUT's loopback HTTP port.
pub struct BrpClient {
    port: u16,
    /// Per-request socket read timeout. A watching verb (`run_until_idle`,
    /// `await_capture`) blocks the server's `recv()` until the SUT advances
    /// enough frames; this timeout is the client-side fail-fast ceiling.
    timeout: Duration,
    /// Monotonic JSON-RPC request id.
    next_id: u64,
}

/// An error from a BRP call — transport failure or a JSON-RPC error reply.
#[derive(Debug)]
pub enum BrpClientError {
    /// TCP / IO failure talking to the SUT.
    Io(String),
    /// The SUT returned a non-200 HTTP status or an unparseable HTTP response.
    Http(String),
    /// The reply body was not valid JSON / not a JSON-RPC envelope.
    Protocol(String),
    /// The SUT returned a JSON-RPC `error` object.
    Rpc { code: i64, message: String },
}

impl std::fmt::Display for BrpClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrpClientError::Io(m) => write!(f, "BRP transport IO error: {m}"),
            BrpClientError::Http(m) => write!(f, "BRP HTTP error: {m}"),
            BrpClientError::Protocol(m) => write!(f, "BRP protocol error: {m}"),
            BrpClientError::Rpc { code, message } => {
                write!(f, "BRP JSON-RPC error {code}: {message}")
            }
        }
    }
}

impl std::error::Error for BrpClientError {}

/// Convenience result alias.
pub type BrpResult<T> = Result<T, BrpClientError>;

impl BrpClient {
    /// Create a client for the BRP server on `127.0.0.1:port`. The default
    /// per-request read timeout is 120 s — generous enough for the longest
    /// `run_until_idle` budget while still fail-fast on a hung SUT.
    pub fn new(port: u16) -> Self {
        Self {
            port,
            timeout: Duration::from_secs(120),
            next_id: 1,
        }
    }

    /// Override the per-request socket read timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The port this client targets.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Issue a BRP method call and return its `result` value (instant verbs)
    /// or its single final streamed value (watching verbs — see the module
    /// doc; both look identical on the wire). `params` may be `Value::Null`.
    pub fn call(&mut self, method: &str, params: Value) -> BrpResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let body = serde_json::to_vec(&request)
            .map_err(|e| BrpClientError::Protocol(format!("serialise request: {e}")))?;

        let response_body = self.post(&body)?;

        let envelope: Value = serde_json::from_slice(&response_body).map_err(|e| {
            BrpClientError::Protocol(format!(
                "reply is not JSON: {e}; body = {}",
                String::from_utf8_lossy(&response_body)
            ))
        })?;

        if let Some(err) = envelope.get("error") {
            let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("<no message>")
                .to_string();
            return Err(BrpClientError::Rpc { code, message });
        }
        match envelope.get("result") {
            Some(result) => Ok(result.clone()),
            None => Err(BrpClientError::Protocol(format!(
                "reply has neither `result` nor `error`: {envelope}"
            ))),
        }
    }

    /// Like [`BrpClient::call`] but deserialises the `result` into a typed
    /// value (e.g. one of the `bevy_naadf::e2e_brp::schema` structs).
    pub fn call_typed<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: Value,
    ) -> BrpResult<T> {
        let result = self.call(method, params)?;
        serde_json::from_value(result)
            .map_err(|e| BrpClientError::Protocol(format!("deserialise `{method}` result: {e}")))
    }

    /// Probe the BRP server with `rpc.discover` — used by [`crate::Sut`] to
    /// poll readiness after spawning the SUT. Returns `Ok(())` once the server
    /// answers, an error while it is not yet up.
    pub fn ping(&mut self) -> BrpResult<()> {
        self.call("rpc.discover", Value::Null).map(|_| ())
    }

    /// POST `body` to the BRP root URL over a fresh TCP connection, return the
    /// raw HTTP response body bytes.
    fn post(&self, body: &[u8]) -> BrpResult<Vec<u8>> {
        let addr = ("127.0.0.1", self.port)
            .to_socket_addrs()
            .map_err(|e| BrpClientError::Io(e.to_string()))?
            .next()
            .ok_or_else(|| BrpClientError::Io("no socket address".to_string()))?;

        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
            .map_err(|e| BrpClientError::Io(format!("connect: {e}")))?;
        stream
            .set_read_timeout(Some(self.timeout))
            .map_err(|e| BrpClientError::Io(format!("set read timeout: {e}")))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| BrpClientError::Io(format!("set write timeout: {e}")))?;
        stream
            .set_nodelay(true)
            .map_err(|e| BrpClientError::Io(format!("set nodelay: {e}")))?;

        // Minimal HTTP/1.1 request. BRP expects a POST of JSON to the root URL
        // (`bevy_remote 0.19.0-rc.1` `http.rs:8`). `Connection: close` so the
        // server signals end-of-body by closing the socket — we then read to
        // EOF and need no chunked-transfer / Content-Length parsing on the
        // response.
        let header = format!(
            "POST / HTTP/1.1\r\n\
             Host: 127.0.0.1:{}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n",
            self.port,
            body.len(),
        );
        stream
            .write_all(header.as_bytes())
            .and_then(|()| stream.write_all(body))
            .and_then(|()| stream.flush())
            .map_err(|e| BrpClientError::Io(format!("write request: {e}")))?;

        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .map_err(|e| BrpClientError::Io(format!("read response: {e}")))?;

        split_http_body(&raw)
    }
}

/// Split a raw HTTP/1.1 response into (status checked) + body bytes.
///
/// Handles `Connection: close` responses (read-to-EOF, the body is everything
/// after the header terminator) and chunked transfer-encoding (BRP's
/// `Complete` JSON path uses `Content-Length`/EOF, but a watching verb invoked
/// with a `+watch` suffix would be chunked SSE — we decode that defensively
/// even though the `naadf/*` verbs are registered bare; see the module doc).
fn split_http_body(raw: &[u8]) -> BrpResult<Vec<u8>> {
    // Find the header/body separator.
    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| {
            BrpClientError::Http(format!(
                "no header terminator in response ({} bytes)",
                raw.len()
            ))
        })?;
    let header = String::from_utf8_lossy(&raw[..sep]);
    let mut lines = header.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    // "HTTP/1.1 200 OK"
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| BrpClientError::Http(format!("bad status line: {status_line:?}")))?;
    if status_code != 200 {
        return Err(BrpClientError::Http(format!(
            "HTTP {status_code} — {}",
            String::from_utf8_lossy(&raw[sep + 4..])
        )));
    }

    let chunked = header
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked");
    let event_stream = header
        .to_ascii_lowercase()
        .contains("content-type: text/event-stream");

    let body = &raw[sep + 4..];
    let decoded = if chunked {
        dechunk(body)?
    } else {
        body.to_vec()
    };

    if event_stream {
        // SSE: the body is one-or-more `data: <json>\n\n` frames. The
        // `naadf/*` watching verbs emit exactly one final frame — take the
        // last `data:` payload. (Defensive: the bare-named verbs do not hit
        // this path, but if a future verb is registered with `+watch` this
        // keeps the client correct.)
        extract_last_sse_frame(&decoded)
    } else {
        Ok(decoded)
    }
}

/// Decode HTTP/1.1 chunked transfer-encoding into the contiguous body.
fn dechunk(mut body: &[u8]) -> BrpResult<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| BrpClientError::Http("truncated chunk size line".to_string()))?;
        let size_str = String::from_utf8_lossy(&body[..line_end]);
        let size = usize::from_str_radix(size_str.trim(), 16)
            .map_err(|e| BrpClientError::Http(format!("bad chunk size {size_str:?}: {e}")))?;
        body = &body[line_end + 2..];
        if size == 0 {
            break;
        }
        if body.len() < size {
            return Err(BrpClientError::Http("truncated chunk body".to_string()));
        }
        out.extend_from_slice(&body[..size]);
        // Skip the trailing CRLF after the chunk data.
        body = &body[size..];
        if body.starts_with(b"\r\n") {
            body = &body[2..];
        }
    }
    Ok(out)
}

/// Extract the JSON payload of the *last* `data: ...` SSE frame in `body`.
fn extract_last_sse_frame(body: &[u8]) -> BrpResult<Vec<u8>> {
    let text = String::from_utf8_lossy(body);
    let last = text
        .split("\n\n")
        .filter(|f| !f.trim().is_empty())
        .last()
        .ok_or_else(|| BrpClientError::Protocol("empty SSE stream".to_string()))?;
    let payload = last
        .lines()
        .find_map(|l| l.strip_prefix("data:"))
        .ok_or_else(|| BrpClientError::Protocol(format!("SSE frame has no `data:`: {last:?}")))?;
    Ok(payload.trim().as_bytes().to_vec())
}
