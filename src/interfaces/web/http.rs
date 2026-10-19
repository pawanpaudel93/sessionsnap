//! A minimal HTTP/1.1 reader and writer for the UI server: one request per connection.

use std::io::{self, BufRead, Read, Write};

pub const MAX_HEAD: usize = 16 * 1024;
pub const MAX_BODY: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: String,
    /// Without the query string.
    pub path: String,
    /// Names lowercased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The first header with this (lowercase) name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        Response { status, content_type: "application/json", body: value.to_string().into_bytes() }
    }

    pub fn text(status: u16, message: &str) -> Self {
        Response { status, content_type: "text/plain; charset=utf-8", body: message.as_bytes().to_vec() }
    }
}

/// Reads one request. An error is the response to send back.
pub fn read_request(r: &mut impl BufRead) -> Result<Request, Response> {
    let mut lines = vec![];
    let mut head = r.by_ref().take(MAX_HEAD as u64);
    loop {
        let mut line = String::new();
        let n = head.read_line(&mut line).map_err(|_| Response::text(400, "unreadable request"))?;
        if n == 0 {
            return Err(if head.limit() == 0 {
                Response::text(431, "request head too large")
            } else {
                Response::text(400, "incomplete request")
            });
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line.is_empty() {
            break;
        }
        lines.push(line);
    }
    let mut first = lines.first().ok_or_else(|| Response::text(400, "empty request"))?.split(' ');
    let (method, target) = match (first.next(), first.next(), first.next()) {
        (Some(m), Some(t), Some(v)) if v.starts_with("HTTP/1.") => (m.to_string(), t),
        _ => return Err(Response::text(400, "bad request line")),
    };
    let path = target.split('?').next().unwrap_or_default().to_string();
    let mut headers = vec![];
    for line in &lines[1..] {
        let (k, v) = line.split_once(':').ok_or_else(|| Response::text(400, "bad header"))?;
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    let mut req = Request { method, path, headers, body: vec![] };
    if req.header("transfer-encoding").is_some() {
        return Err(Response::text(400, "chunked bodies are not supported"));
    }
    let len: usize = match req.header("content-length") {
        None => 0,
        Some(v) => v.parse().map_err(|_| Response::text(400, "bad content-length"))?,
    };
    if len > MAX_BODY {
        return Err(Response::text(413, "body too large"));
    }
    req.body = vec![0; len];
    r.read_exact(&mut req.body).map_err(|_| Response::text(400, "incomplete body"))?;
    Ok(req)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

pub fn write_response(w: &mut impl Write, r: &Response) -> io::Result<()> {
    write!(
        w,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'\r\n\r\n",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len()
    )?;
    w.write_all(&r.body)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Result<Request, Response> {
        read_request(&mut io::Cursor::new(raw.as_bytes().to_vec()))
    }

    #[test]
    fn reads_request_line_headers_and_body() {
        let req = parse("POST /api/run?x=1 HTTP/1.1\r\nHost: 127.0.0.1:47317\r\nContent-Length: 4\r\nX-Sessionsnap-Token: abc\r\n\r\n{}{}")
            .unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/api/run");
        assert_eq!(req.header("host"), Some("127.0.0.1:47317"));
        assert_eq!(req.header("x-sessionsnap-token"), Some("abc"));
        assert_eq!(req.body, b"{}{}");
    }

    #[test]
    fn rejects_bad_requests() {
        assert_eq!(parse("GARBAGE\r\n\r\n").unwrap_err().status, 400);
        assert_eq!(parse("GET / HTTP/1.1\r\nHost: x\r\n").unwrap_err().status, 400);
        assert_eq!(parse(&format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEAD))).unwrap_err().status, 431);
        assert_eq!(
            parse(&format!("POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1)).unwrap_err().status,
            413
        );
        assert_eq!(parse("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap_err().status, 400);
        assert_eq!(parse("POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc").unwrap_err().status, 400);
    }

    #[test]
    fn writes_status_headers_and_body() {
        let mut out = vec![];
        write_response(&mut out, &Response::text(404, "nope")).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("HTTP/1.1 404 Not Found\r\n"), "{s}");
        assert!(s.contains("Content-Length: 4\r\n"));
        assert!(s.contains("Connection: close\r\n"));
        assert!(s.contains("Content-Security-Policy: default-src 'self'"));
        assert!(s.ends_with("\r\n\r\nnope"));
    }
}
