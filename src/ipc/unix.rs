//! Unix-domain-socket transport for the IPC protocol.

use std::path::Path;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use super::{Request, Response};

// ---------------------------------------------------------------------------
// Framing: newline-delimited JSON
// ---------------------------------------------------------------------------

async fn send_response(stream: &mut UnixStream, resp: &Response) -> Result<()> {
    let line = serde_json::to_string(resp)?;
    stream.write_all(line.as_bytes()).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;
    Ok(())
}

async fn read_request(stream: &mut UnixStream) -> Result<Option<Request>> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader
        .read_line(&mut line)
        .await
        .context("failed to read IPC line")?;
    if n == 0 {
        return Ok(None);
    }
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let req: Request = serde_json::from_str(line)
        .with_context(|| format!("failed to parse IPC request: {line}"))?;
    Ok(Some(req))
}

// ---------------------------------------------------------------------------
// Server (daemon side)
// ---------------------------------------------------------------------------

/// IPC handler trait. The daemon provides an implementation that knows about
/// config, auth, and the endpoint.
pub trait IpcHandler: Send + Sync + 'static {
    /// Handle one authenticated request and produce a response.
    fn handle(&self, req: Request, ipc_token: &str) -> impl Future<Output = Response> + Send;
}

/// Run the IPC server on a Unix domain socket. Blocks until the socket is
/// removed or an error occurs.
pub async fn serve<H: IpcHandler + Clone>(
    socket: &Path,
    handler: H,
    ipc_token: String,
) -> Result<()> {
    // Remove stale socket from a previous run.
    let _ = std::fs::remove_file(socket);

    let listener = UnixListener::bind(socket)
        .with_context(|| format!("failed to bind IPC socket at {}", socket.display()))?;
    crate::identity::restrict(socket, 0o600);

    tracing::info!("IPC server listening on {}", socket.display());

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let handler = handler.clone();
                let token = ipc_token.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, handler, &token).await {
                        tracing::debug!("IPC client error: {e:#}");
                    }
                });
            }
            Err(e) => {
                tracing::error!("IPC accept error: {e:#}");
            }
        }
    }
}

async fn handle_client<H: IpcHandler>(
    mut stream: UnixStream,
    handler: H,
    ipc_token: &str,
) -> Result<()> {
    // First message must be Hello.
    match read_request(&mut stream).await? {
        Some(Request::Hello { token }) => {
            if !constant_time_eq(&token, ipc_token) {
                send_response(&mut stream, &Response::Denied).await?;
                return Ok(());
            }
            send_response(&mut stream, &Response::Ok).await?;
        }
        _ => {
            send_response(&mut stream, &Response::Denied).await?;
            return Ok(());
        }
    }

    // Process requests until the client disconnects.
    while let Some(req) = read_request(&mut stream).await? {
        let resp = handler.handle(req, ipc_token).await;
        send_response(&mut stream, &resp).await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Client (CLI side)
// ---------------------------------------------------------------------------

/// CLI-side IPC client: connects, authenticates, and exchanges requests.
pub struct IpcClient {
    stream: UnixStream,
}

impl IpcClient {
    /// Connect to the daemon's IPC socket and authenticate.
    pub async fn connect(socket: &Path, ipc_token: &str) -> Result<Self> {
        let stream = UnixStream::connect(socket)
            .await
            .with_context(|| format!("failed to connect to IPC socket at {}", socket.display()))?;

        let mut client = Self { stream };
        client
            .send_raw(&Request::Hello {
                token: ipc_token.to_string(),
            })
            .await?;
        match client.read_raw().await? {
            Some(Response::Ok) => Ok(client),
            Some(Response::Denied) => Err(anyhow::anyhow!("IPC authentication denied")),
            Some(other) => Err(anyhow::anyhow!("unexpected IPC response: {other:?}")),
            None => Err(anyhow::anyhow!("IPC connection closed during handshake")),
        }
    }

    /// Send a request and read the response.
    pub async fn request(&mut self, req: Request) -> Result<Response> {
        self.send_raw(&req).await?;
        self.read_raw()
            .await?
            .ok_or_else(|| anyhow::anyhow!("IPC connection closed"))
    }

    async fn send_raw(&mut self, req: &Request) -> Result<()> {
        let line = serde_json::to_string(req)?;
        self.stream.write_all(line.as_bytes()).await?;
        self.stream.write_all(b"\n").await?;
        self.stream.flush().await?;
        Ok(())
    }

    async fn read_raw(&mut self) -> Result<Option<Response>> {
        let mut reader = BufReader::new(&mut self.stream);
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(None);
        }
        let line = line.trim();
        if line.is_empty() {
            return Ok(None);
        }
        let resp: Response = serde_json::from_str(line)?;
        Ok(Some(resp))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
    }
}
