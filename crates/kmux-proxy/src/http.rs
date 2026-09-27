//! HTTP framing, authentication, and routing for the versioned proxy API.
use crate::{config::Config, unified};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const MAX_BODY: usize = 1 << 20;

fn token_ok(cfg: &Config, given: &str) -> bool {
    !given.is_empty() && given == cfg.token
}

pub(crate) fn handle(mut stream: TcpStream, cfg: &Config) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let result = read_request(&mut stream);
    let (code, value) = match result {
        Err(e) => ("400 Bad Request", serde_json::json!({"error":e})),
        Ok((method, path, _body)) if method == "GET" && path == "/health" => (
            "200 OK",
            serde_json::json!({"ok":true,"version":kmuxd::api::VERSION}),
        ),
        Ok((method, path, body)) if method == "POST" && path == "/v1/changes" => {
            match serde_json::from_slice::<kmuxd::api::ChangesRequest>(&body) {
                Err(e) => (
                    "400 Bad Request",
                    serde_json::json!({"error": e.to_string()}),
                ),
                Ok(r) if !token_ok(cfg, &r.token) => (
                    "401 Unauthorized",
                    serde_json::json!({"error":"unauthorized"}),
                ),
                Ok(r) => match unified::changes(cfg, r) {
                    Ok(value) => ("200 OK", serde_json::to_value(value).unwrap()),
                    Err(e) => ("409 Conflict", serde_json::json!({"error": e})),
                },
            }
        }
        Ok((method, path, body)) if method == "POST" && path == "/v1/action" => {
            match serde_json::from_slice::<kmuxd::api::Request>(&body) {
                Err(e) => (
                    "400 Bad Request",
                    serde_json::json!({"error":e.to_string()}),
                ),
                Ok(r) if !token_ok(cfg, &r.token) => (
                    "401 Unauthorized",
                    serde_json::json!({"error":"unauthorized"}),
                ),
                Ok(r) => {
                    if matches!(r.action, kmuxd::api::Action::Diagnose) {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(25)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(25)));
                    }
                    match unified::request(cfg, r) {
                        Ok(snapshot) => ("200 OK", serde_json::to_value(snapshot).unwrap()),
                        Err(e) => ("409 Conflict", serde_json::json!({"error": e})),
                    }
                }
            }
        }
        _ => (
            "404 Not Found",
            serde_json::json!({"error":"use POST /v1/action with API version 1"}),
        ),
    };
    let body = value.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}
fn read_request(stream: &mut impl Read) -> Result<(String, String, Vec<u8>), String> {
    let mut head = Vec::new();
    let mut byte = [0; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16384 {
            return Err("headers too large".into());
        }
        stream.read_exact(&mut byte).map_err(|e| e.to_string())?;
        head.push(byte[0]);
    }
    let text = std::str::from_utf8(&head).map_err(|e| e.to_string())?;
    let mut lines = text.split("\r\n");
    let parts: Vec<_> = lines.next().unwrap_or("").split_whitespace().collect();
    if parts.len() != 3 {
        return Err("invalid request".into());
    }
    let mut length = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("transfer-encoding") {
                return Err("chunked requests unsupported".into());
            }
            if name.eq_ignore_ascii_case("content-length") {
                if length.is_some() {
                    return Err("duplicate content-length".into());
                }
                length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| "bad content length")?,
                );
            }
        }
    }
    let length = length.unwrap_or(0);
    if length > MAX_BODY {
        return Err("request too large".into());
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).map_err(|e| e.to_string())?;
    Ok((parts[0].into(), parts[1].into(), body))
}
#[cfg(test)]
mod http_tests {
    use super::*;
    #[test]
    fn reads_full_unicode_body_even_when_fragmented() {
        struct Fragmented(std::io::Cursor<Vec<u8>>);
        impl Read for Fragmented {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = buf.len().min(2);
                self.0.read(&mut buf[..n])
            }
        }
        let body = r#"{"text":"λ\nquotes"}"#;
        let bytes = format!(
            "POST /v1/action HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes();
        let (method, path, actual) =
            read_request(&mut Fragmented(std::io::Cursor::new(bytes))).unwrap();
        assert_eq!(method, "POST");
        assert_eq!(path, "/v1/action");
        assert_eq!(actual, body.as_bytes());
    }
    #[test]
    fn rejects_ambiguous_or_oversized_bodies() {
        for header in [
            "Content-Length: 0\r\nContent-Length: 1",
            "Content-Length: 1048577",
            "Transfer-Encoding: chunked",
        ] {
            let bytes = format!("POST /v1/action HTTP/1.1\r\n{header}\r\n\r\n");
            assert!(read_request(&mut bytes.as_bytes()).is_err());
        }
    }
}
