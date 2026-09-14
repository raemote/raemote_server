//! Minimal dev client with a persistent identity (`~/.raemote/client.key`).
//!
//! Usage:
//!   cargo run --example client -- bind <node-id> <token>
//!   cargo run --example client -- bind <raemote://bind?... URI>
//!   cargo run --example client -- get <node-id> [path]
//!   cargo run --example client -- request <node-id> <METHOD> <path> [json-body]
//!
//! `bind` authorizes this client's node ID on the server (multi-use token,
//! valid until it expires). `get`/`request` then issue plain HTTP requests over
//! the serve ALPN. All commands reuse the same persistent key so the node ID —
//! and thus the authorization — survives across runs.

use std::env;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result};
use iroh::EndpointId;
use iroh::endpoint::presets;
use iroh::SecretKey;
use iroh::Endpoint;

const SERVE_ALPN: &[u8] = b"raemote/0";
const BIND_ALPN: &[u8] = b"raemote/bind/0";

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let command = args.next().context(usage())?;

    let secret_key = load_or_create_client_key()?;
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .bind()
        .await?;
    println!("client node id: {}", endpoint.id());

    match command.as_str() {
        "bind" => {
            let arg = args.next().context(usage())?;
            let (node, token) = if arg.starts_with("raemote://") {
                parse_bind_uri(&arg)?
            } else {
                let token = args.next().context("bind needs a token")?;
                (parse_node(&arg)?, token)
            };
            bind(&endpoint, node, &token).await
        }
        "get" => {
            let node = args.next().context(usage())?;
            let path = args
                .next()
                .unwrap_or_else(|| "/_hub/catalog".to_string());
            request(&endpoint, parse_node(&node)?, "GET", &path, None).await
        }
        "request" => {
            let node = args.next().context(usage())?;
            let method = args.next().unwrap_or_else(|| "GET".to_string());
            let path = args.next().unwrap_or_else(|| "/".to_string());
            let body = args.next();
            request(&endpoint, parse_node(&node)?, &method, &path, body.as_deref()).await
        }
        _ => Err(anyhow::anyhow!(usage())),
    }
}

async fn bind(endpoint: &Endpoint, node: EndpointId, token: &str) -> Result<()> {
    let connection = endpoint
        .connect(node, BIND_ALPN)
        .await
        .context("failed to connect")?;
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(format!("{token}\n").as_bytes())
        .await
        .context("failed to send token")?;
    send.finish()?;

    let reply = recv
        .read_to_end(1024)
        .await
        .context("failed to read bind reply")?;
    print!("{}", String::from_utf8_lossy(&reply));
    Ok(())
}

async fn request(
    endpoint: &Endpoint,
    node: EndpointId,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<()> {
    let connection = endpoint
        .connect(node, SERVE_ALPN)
        .await
        .context("failed to connect")?;
    let (mut send, mut recv) = connection.open_bi().await?;

    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: raemote\r\nConnection: close\r\nAccept: */*\r\n"
    );
    if let Some(body) = body {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    if let Some(body) = body {
        request.push_str(body);
    }

    send.write_all(request.as_bytes())
        .await
        .context("failed to send request")?;
    send.finish()?;

    let body = recv
        .read_to_end(8 * 1024 * 1024)
        .await
        .context("failed to read response (is this client bound?)")?;
    println!("{}", String::from_utf8_lossy(&body));
    Ok(())
}

/// `raemote://bind?node=<id>&token=<hex>&exp=<unix>`
fn parse_bind_uri(uri: &str) -> Result<(EndpointId, String)> {
    let query = uri
        .strip_prefix("raemote://bind?")
        .context("not a raemote bind URI")?;
    let mut node = None;
    let mut token = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').context("malformed bind URI")?;
        match key {
            "node" => node = Some(parse_node(value)?),
            "token" => token = Some(value.to_string()),
            _ => {}
        }
    }
    Ok((
        node.context("bind URI is missing the node id")?,
        token.context("bind URI is missing the token")?,
    ))
}

fn parse_node(s: &str) -> Result<EndpointId> {
    EndpointId::from_str(s).context("invalid node id")
}

fn load_or_create_client_key() -> Result<SecretKey> {
    let path = client_key_path()?;
    if path.exists() {
        let bytes = std::fs::read(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let bytes: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("client key must be 32 bytes"))?;
        Ok(SecretKey::from_bytes(&bytes))
    } else {
        let key = SecretKey::generate();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&path, key.to_bytes())
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(key)
    }
}

fn client_key_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".raemote").join("client.key"))
}

fn usage() -> String {
    "usage: client bind <node-id> <token> | client bind <raemote://bind?...> | client get <node-id> [path] | client request <node-id> <METHOD> <path> [json-body]".to_string()
}
