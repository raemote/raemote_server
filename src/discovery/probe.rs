//! A single bounded HTTP probe: confirms an origin serves HTTP and reads its
//! `<title>`.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{timeout_at, Instant};

use super::model::Origin;

/// Hard cap on how many bytes we read from a probe response.
const MAX_PROBE_BYTES: usize = 24 * 1024;
/// Stop reading the body once we have this much (or a title), whichever first.
const BODY_READ_CAP: usize = 16 * 1024;

/// What a successful HTTP probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    /// HTTP status code from the response line.
    pub status: u16,
    /// `<title>` text, normalized, when the body contained one.
    pub title: Option<String>,
    /// `Server` response header, when present.
    pub server: Option<String>,
}

/// Probe `origin` with a single bounded `GET /` and decide whether it is an
/// HTTP server. Returns `None` if it is not HTTP or the budget elapses.
///
/// The read loop parses whatever arrived even if the peer keeps the socket
/// open past the budget, so a slow-but-real server is still classified.
pub async fn probe_http(origin: &Origin, budget: Duration) -> Option<ProbeResult> {
    let mut stream = match timeout_at(
        Instant::now() + budget,
        TcpStream::connect((origin.host.as_str(), origin.port)),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        _ => return None,
    };

    let request = format!(
        "GET / HTTP/1.1\r\nHost: {}\r\nUser-Agent: raemote-discovery/0.1\r\nAccept: text/html,application/xhtml+xml,*/*;q=0.8\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
        origin.authority()
    );
    if stream.write_all(request.as_bytes()).await.is_err() {
        return None;
    }

    let deadline = Instant::now() + budget;
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    let mut headers_done = false;

    loop {
        let n = match timeout_at(deadline, stream.read(&mut tmp)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => n,
            // Timeout or error: parse what we already have.
            _ => break,
        };
        buf.extend_from_slice(&tmp[..n]);

        if !headers_done && find_slice(&buf, b"\r\n\r\n").is_some() {
            headers_done = true;
        }
        if headers_done && (find_ci(&buf, b"</title").is_some() || buf.len() >= BODY_READ_CAP) {
            break;
        }
        if buf.len() >= MAX_PROBE_BYTES {
            break;
        }
    }

    parse_response(&buf)
}

fn parse_response(buf: &[u8]) -> Option<ProbeResult> {
    let header_end = find_slice(buf, b"\r\n\r\n");
    let header_bytes = match header_end {
        Some(i) => &buf[..i],
        None => buf,
    };

    let text = String::from_utf8_lossy(header_bytes);
    let mut lines = text.split("\r\n");
    let status_line = lines.next()?;
    let mut parts = status_line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    let status: u16 = parts.next()?.parse().ok()?;

    let mut server = None;
    for line in lines {
        if let Some((key, value)) = line.split_once(':')
            && key.eq_ignore_ascii_case("server")
        {
            let value = value.trim();
            if !value.is_empty() {
                server = Some(value.to_string());
            }
        }
    }

    let body = match header_end {
        Some(i) => &buf[i + 4..],
        None => buf,
    };
    let title = extract_title(body);

    Some(ProbeResult {
        status,
        title,
        server,
    })
}

/// Extract and normalize the first `<title>` from an HTML byte slice.
fn extract_title(bytes: &[u8]) -> Option<String> {
    let open = find_ci(bytes, b"<title")?;
    let after_open = open + b"<title".len();
    let gt = bytes[after_open..].iter().position(|&b| b == b'>')? + after_open + 1;
    let close = find_ci(&bytes[gt..], b"</title")? + gt;

    let decoded = String::from_utf8_lossy(&bytes[gt..close]);
    let mut out = String::with_capacity(decoded.len());
    let mut prev_space = true;
    for ch in decoded.chars() {
        if ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    let out = out.trim();
    if out.is_empty() {
        None
    } else {
        Some(out.chars().take(120).collect())
    }
}

/// Exact byte-slice search.
fn find_slice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// ASCII case-insensitive byte-slice search (no allocation).
fn find_ci(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// Serve exactly one canned response, then close.
    async fn serve_once(response: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        port
    }

    const OK_RESPONSE: &str = "HTTP/1.1 200 OK\r\nServer: nginx/1.25\r\nContent-Type: text/html\r\nContent-Length: 44\r\n\r\n<html><head><title>  My   App </title></head></html>";

    #[tokio::test]
    async fn probes_http_and_extracts_title() {
        let port = serve_once(OK_RESPONSE).await;
        let origin = Origin::http("127.0.0.1", port);
        let result = probe_http(&origin, Duration::from_millis(1000))
            .await
            .expect("should probe");
        assert_eq!(result.status, 200);
        assert_eq!(result.title.as_deref(), Some("My App"));
        assert_eq!(result.server.as_deref(), Some("nginx/1.25"));
    }

    #[tokio::test]
    async fn rejects_non_http() {
        let port = serve_once("GARBAGE NOT HTTP\r\n\r\n").await;
        let origin = Origin::http("127.0.0.1", port);
        assert!(probe_http(&origin, Duration::from_millis(1000))
            .await
            .is_none());
    }

    #[tokio::test]
    async fn times_out_on_silent_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            // Accept and hold the connection open without replying.
            if let Ok((sock, _)) = listener.accept().await {
                tokio::time::sleep(Duration::from_secs(5)).await;
                drop(sock);
            }
        });
        let origin = Origin::http("127.0.0.1", port);
        let start = Instant::now();
        assert!(probe_http(&origin, Duration::from_millis(150))
            .await
            .is_none());
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn no_server_returns_none() {
        // Bind then drop to obtain a definitely-closed port.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let origin = Origin::http("127.0.0.1", port);
        assert!(probe_http(&origin, Duration::from_millis(200))
            .await
            .is_none());
    }

    #[test]
    fn extract_title_handles_case_and_empty() {
        assert_eq!(
            extract_title(b"<HTML><TITLE>Hello</TITLE>"),
            Some("Hello".to_string())
        );
        assert_eq!(extract_title(b"<title>   </title>"), None);
        assert_eq!(extract_title(b"no title here"), None);
    }
}
