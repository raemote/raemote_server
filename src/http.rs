//! The HTTP ALPN (`raemote/0`): a hub API and reverse proxy for authorized
//! devices.
//!
//! Only authorized device identities are served. Routes: `/_hub/catalog`,
//! `/_hub/discover`, and `/app/{name}/...` (proxied to the local app).

use std::collections::HashMap;
use std::convert::Infallible;
use std::io;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo};
use iroh::endpoint::{Connection, VarInt};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::EndpointId;
use leaky_bucket::RateLimiter;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

use crate::auth::{default_device_name, sanitize_device_name, AuthState};
use crate::catalog::Catalog;
use crate::config::Config;
use crate::discovery::model::Origin;
use crate::iroh_stream::IrohStream;

/// ALPN for serving the HTTP API to authorized clients only.
pub const SERVE_ALPN: &[u8] = b"raemote/0";

/// Application error codes used when closing connections.
const ERR_UNAUTHORIZED: u32 = 401;
const ERR_TOO_MANY_STREAMS: u32 = 503;

/// Max time to wait for the local origin to produce a response head.
const PROXY_TIMEOUT: Duration = Duration::from_secs(30);

/// How long `/_hub/discover` waits for a scan to complete.
const DISCOVER_WAIT: Duration = Duration::from_secs(3);

type ResBody = BoxBody<Bytes, io::Error>;

/// Per-node limiter combining a concurrency cap (Semaphore) and a rate limit
/// (leaky bucket). Created on first use for each authorized node, lives for
/// process lifetime (until config reload invalidates the cache).
struct NodeLimiter {
    semaphore: Arc<Semaphore>,
    rate: Arc<RateLimiter>,
}

impl NodeLimiter {
    fn new(max_concurrent: usize, rate_refill: usize, rate_interval: Duration, rate_max: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            rate: Arc::new(
                RateLimiter::builder()
                    .initial(rate_refill)
                    .refill(rate_refill)
                    .interval(rate_interval)
                    .max(rate_max)
                    .build(),
            ),
        }
    }

    fn try_acquire_concurrency(&self) -> Option<OwnedSemaphorePermit> {
        self.semaphore.clone().try_acquire_owned().ok()
    }

    async fn acquire_rate_token(&self) {
        self.rate.clone().acquire_owned(1).await;
    }
}

/// Handle to the discovery engine, so the HTTP API can trigger a scan.
#[derive(Clone)]
pub struct DiscoveryHandle {
    /// Signalled to request an immediate scan.
    pub trigger: Arc<Notify>,
    /// Bumped after each scan; used to wait for one to finish.
    pub generation: Arc<AtomicU64>,
}

/// Shared state for every request served over an iroh stream.
pub struct AppState {
    client: Client<HttpConnector, ResBody>,
    config: Arc<RwLock<Config>>,
    catalog: Arc<RwLock<Catalog>>,
    discovery: Option<DiscoveryHandle>,
}

impl AppState {
    /// Build state without a discovery handle.
    pub fn new(config: Arc<RwLock<Config>>, catalog: Arc<RwLock<Catalog>>) -> Self {
        Self::with_discovery(config, catalog, None)
    }

    /// Build state, optionally wired to the discovery engine.
    pub fn with_discovery(
        config: Arc<RwLock<Config>>,
        catalog: Arc<RwLock<Catalog>>,
        discovery: Option<DiscoveryHandle>,
    ) -> Self {
        let client = Client::builder(TokioExecutor::new()).build_http();
        Self {
            client,
            config,
            catalog,
            discovery,
        }
    }
}

/// Handler for [`SERVE_ALPN`]: the HTTP API, gated on the caller's node ID
/// being in the authorized set. Unauthorized connections are closed with
/// `ERR_UNAUTHORIZED` before any stream is served. Each authorized node
/// gets a per-node concurrency cap and rate limit.
pub struct ServeHandler {
    state: Arc<AppState>,
    auth: Arc<AuthState>,
    limiters: Mutex<HashMap<EndpointId, Arc<NodeLimiter>>>,
    /// The config generation at which limiters were last created.
    /// Compared against `config_generation` to invalidate on reload.
    limiter_generation: AtomicU64,
    /// Shared config generation counter; incremented on reload.
    config_generation: Arc<AtomicU64>,
    /// Number of live authorized serve connections (for status/diagnostics).
    active_connections: Arc<AtomicUsize>,
}

impl ServeHandler {
    /// Build a serve handler over shared state and auth.
    pub fn new(
        state: Arc<AppState>,
        auth: Arc<AuthState>,
        config_generation: Arc<AtomicU64>,
        active_connections: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            state,
            auth,
            limiters: Mutex::new(HashMap::new()),
            limiter_generation: AtomicU64::new(0),
            config_generation,
            active_connections,
        }
    }

    fn get_or_create_limiter(&self, node: EndpointId) -> Arc<NodeLimiter> {
        // Check if config generation changed; if so, clear the cache.
        let current_gen = self.config_generation.load(Ordering::Relaxed);
        let last_gen = self.limiter_generation.load(Ordering::Relaxed);
        if current_gen != last_gen {
            let mut limiters = self.limiters.lock().expect("limiters poisoned");
            // Re-check under the lock to avoid races.
            if self.limiter_generation.load(Ordering::Relaxed) != current_gen {
                limiters.clear();
                self.limiter_generation.store(current_gen, Ordering::Relaxed);
                tracing::debug!(
                    old_gen = last_gen,
                    new_gen = current_gen,
                    "cleared limiter cache on config reload"
                );
            }
        }

        let cfg = self.state.config.read().expect("config poisoned");
        let serve = &cfg.serve;

        let mut limiters = self.limiters.lock().expect("limiters poisoned");
        limiters
            .entry(node)
            .or_insert_with(|| {
                Arc::new(NodeLimiter::new(
                    serve.max_concurrent_streams,
                    serve.rate_limit.refill,
                    Duration::from_millis(serve.rate_limit.interval_ms),
                    serve.rate_limit.max,
                ))
            })
            .clone()
    }
}

impl std::fmt::Debug for ServeHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServeHandler").finish_non_exhaustive()
    }
}

/// Decrements the live serve-connection count when the connection ends,
/// including on early return or panic.
struct ConnectionGuard(Arc<AtomicUsize>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl ProtocolHandler for ServeHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let node = connection.remote_id();
        if !self.auth.is_authorized(node) {
            tracing::warn!(
                node = %node.fmt_short(),
                "closing unauthorized serve connection"
            );
            connection.close(VarInt::from_u32(ERR_UNAUTHORIZED), b"unauthorized");
            return Ok(());
        }
        tracing::info!(node = %node.fmt_short(), "serving authorized node");
        self.active_connections.fetch_add(1, Ordering::Relaxed);
        let _connection_guard = ConnectionGuard(self.active_connections.clone());
        let limiter = self.get_or_create_limiter(node);
        loop {
            match connection.accept_bi().await {
                Ok((mut send, recv)) => {
                    // Re-check on every stream: a device revoked after the
                    // connection was established must lose access on its next
                    // request, not keep a live connection forever.
                    if !self.auth.is_authorized(node) {
                        tracing::warn!(
                            node = %node.fmt_short(),
                            "closing serve connection for revoked device"
                        );
                        connection.close(VarInt::from_u32(ERR_UNAUTHORIZED), b"revoked");
                        break;
                    }
                    tracing::info!(node = %node.fmt_short(), "accepted bi-stream");
                    let state = self.state.clone();
                    let auth = self.auth.clone();
                    let limiter = limiter.clone();
                    tokio::spawn(async move {
                        let _concurrency_permit = match limiter.try_acquire_concurrency() {
                            Some(permit) => permit,
                            None => {
                                tracing::warn!(
                                    node = %node.fmt_short(),
                                    "rejecting stream: too many concurrent streams"
                                );
                                let _ = send.reset(VarInt::from_u32(ERR_TOO_MANY_STREAMS));
                                return;
                            }
                        };
                        limiter.acquire_rate_token().await;
                        serve_stream(state, auth, IrohStream::new(send, recv), node).await;
                    });
                }
                Err(err) => {
                    tracing::debug!(?err, "serve connection closed");
                    break;
                }
            }
        }
        Ok(())
    }
}

/// Serve HTTP/1.1 requests arriving on one iroh bidirectional stream.
///
/// `node` is the authenticated caller, passed so routes like `/_hub/device`
/// can act on "this device".
pub async fn serve_stream(
    state: Arc<AppState>,
    auth: Arc<AuthState>,
    stream: IrohStream,
    node: EndpointId,
) {
    let service = service_fn(move |req| {
        let state = state.clone();
        let auth = auth.clone();
        async move { handle(req, state, auth, node).await }
    });
    if let Err(err) = hyper::server::conn::http1::Builder::new()
        .half_close(true)
        .serve_connection(TokioIo::new(stream), service)
        .await
    {
        tracing::debug!("stream closed: {err}");
    }
}

async fn handle(
    req: Request<Incoming>,
    state: Arc<AppState>,
    auth: Arc<AuthState>,
    node: EndpointId,
) -> Result<Response<ResBody>, io::Error> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let headers: Vec<_> = req.headers().iter().map(|(k, v)| format!("{k}: {v:?}")).collect();
    tracing::info!(%method, %path, ?headers, "HTTP request received");

    if path == "/_hub/catalog" {
        let resp = catalog(&state);
        tracing::info!(status = %resp.status(), "responding with catalog");
        return Ok(resp);
    }

    if path == "/_hub/discover" {
        let resp = discover(&state).await;
        tracing::info!(status = %resp.status(), "responding with discovery result");
        return Ok(resp);
    }

    if path == "/_hub/info" {
        return Ok(info(&state));
    }

    if path == "/_hub/devices" {
        return Ok(devices(&auth, node));
    }

    if path == "/_hub/device" {
        if method == Method::PUT || method == Method::POST {
            return Ok(rename_self(&auth, node, req).await);
        }
        return Ok(error_response(
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "use PUT to set this device's name",
            None,
        ));
    }

    if let Some(rest) = path.strip_prefix("/app/") {
        let mut segments = rest.splitn(2, '/');
        let name = segments.next().unwrap_or_default();
        let sub_path = segments.next().unwrap_or_default();
        // Look up the origin, clone it, then drop the guard before any await.
        let origin = {
            let catalog = state.catalog.read().expect("catalog poisoned");
            catalog.find(name).map(|app| app.origin.clone())
        };
        match origin {
            Some(origin) => {
                tracing::info!(app = %name, authority = %origin.authority(), sub_path, "proxying to origin");
                return proxy(&state, req, &origin, sub_path).await;
            }
            None => {
                tracing::warn!(app = %name, "unknown app");
                return Ok(error_response(
                    StatusCode::NOT_FOUND,
                    "unknown_app",
                    &format!("unknown app \"{name}\""),
                    Some("it may have stopped — refresh the app list, or run `raemote discover` on the server"),
                ));
            }
        }
    }

    tracing::warn!(path, "no matching route");
    Ok(error_response(
        StatusCode::NOT_FOUND,
        "not_found",
        &format!("no such route: {path}"),
        None,
    ))
}

/// Trigger an immediate discovery scan (if wired) and wait briefly for it to
/// finish, then return the refreshed catalog.
async fn discover(state: &AppState) -> Response<ResBody> {
    let Some(handle) = &state.discovery else {
        return catalog(state);
    };
    let before = handle.generation.load(Ordering::Relaxed);
    handle.trigger.notify_one();
    let deadline = tokio::time::Instant::now() + DISCOVER_WAIT;
    while handle.generation.load(Ordering::Relaxed) == before
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    catalog(state)
}

fn catalog(state: &AppState) -> Response<ResBody> {
    let catalog = state.catalog.read().expect("catalog poisoned");
    let apps: Vec<_> = catalog
        .apps()
        .iter()
        .map(|app| {
            serde_json::json!({
                "name": app.name,
                "path": format!("/app/{}", app.name),
                "port": app.origin.port,
                "scheme": app.origin.scheme.as_str(),
                "source": app.source.as_str(),
                "title": app.title,
            })
        })
        .collect();
    json_response(StatusCode::OK, serde_json::json!({ "apps": apps }))
}

/// `GET /_hub/info`: basic server identity for the app to display.
fn info(state: &AppState) -> Response<ResBody> {
    let name = {
        let cfg = state.config.read().expect("config poisoned");
        crate::config::server_name(&cfg)
    };
    json_response(
        StatusCode::OK,
        serde_json::json!({
            "name": name,
            "version": env!("CARGO_PKG_VERSION"),
        }),
    )
}

/// `GET /_hub/devices`: the paired devices and their display names.
fn devices(auth: &AuthState, node: EndpointId) -> Response<ResBody> {
    let devices: Vec<_> = auth
        .device_infos()
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "node_id": d.node_id.to_string(),
                "name": d.name,
                "self": d.node_id == node,
            })
        })
        .collect();
    json_response(StatusCode::OK, serde_json::json!({ "devices": devices }))
}

/// `PUT /_hub/device`: let a device set its own display name.
async fn rename_self(
    auth: &AuthState,
    node: EndpointId,
    req: Request<Incoming>,
) -> Response<ResBody> {
    #[derive(serde::Deserialize)]
    struct RenameBody {
        name: String,
    }

    let body = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "could not read the request body",
                None,
            );
        }
    };
    let parsed: RenameBody = match serde_json::from_slice(&body) {
        Ok(parsed) => parsed,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "expected a JSON body like {\"name\": \"My iPhone\"}",
                None,
            );
        }
    };

    match auth.set_device_name(node, &parsed.name) {
        Ok(()) => {
            let name =
                sanitize_device_name(&parsed.name).unwrap_or_else(|| default_device_name(node));
            json_response(StatusCode::OK, serde_json::json!({ "ok": true, "name": name }))
        }
        Err(err) => {
            error_response(StatusCode::BAD_REQUEST, "bad_request", &err.to_string(), None)
        }
    }
}

async fn proxy(
    state: &AppState,
    req: Request<Incoming>,
    origin: &Origin,
    sub_path: &str,
) -> Result<Response<ResBody>, io::Error> {
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let upstream_uri = format!(
        "{}://{}/{}{}",
        origin.scheme.as_str(),
        origin.authority(),
        sub_path,
        query
    );
    tracing::info!(upstream = %upstream_uri, "proxying request to origin");

    let (parts, body) = req.into_parts();
    let mut builder = Request::builder().method(parts.method).uri(upstream_uri);
    {
        let headers = builder
            .headers_mut()
            .expect("uri is always valid in a fresh request builder");
        for (name, value) in parts.headers.iter() {
            if name.as_str() == "host" || is_hop_by_hop(name.as_str()) {
                continue;
            }
            // Point loopback Origin/Referer at the upstream so the app's own
            // CSRF checks pass.
            if let ("origin" | "referer", Ok(text)) = (name.as_str(), value.to_str()) {
                let rewritten = if name.as_str() == "origin" {
                    rewrite_origin(text, origin)
                } else {
                    rewrite_referer(text, origin)
                };
                if let Ok(header) = hyper::header::HeaderValue::from_str(&rewritten) {
                    headers.append(name, header);
                    continue;
                }
            }
            headers.append(name, value.clone());
        }
    }
    let body = body.map_err(io::Error::other).boxed();
    let upstream_req = builder.body(body).map_err(io::Error::other)?;

    match tokio::time::timeout(PROXY_TIMEOUT, state.client.request(upstream_req)).await {
        Ok(Ok(response)) => {
            let (mut parts, body) = response.into_parts();
            let mut headers = hyper::HeaderMap::new();
            for (name, value) in parts.headers.iter() {
                if name.as_str() == "host" || is_hop_by_hop(name.as_str()) {
                    continue;
                }
                // Keep absolute redirects on the loopback origin.
                if let ("location", Ok(text)) = (name.as_str(), value.to_str()) {
                    let rewritten = rewrite_location(text, origin);
                    if let Ok(header) = hyper::header::HeaderValue::from_str(&rewritten) {
                        headers.append(name, header);
                        continue;
                    }
                }
                headers.append(name, value.clone());
            }
            parts.headers = headers;
            Ok(Response::from_parts(parts, body.map_err(io::Error::other).boxed()))
        }
        Ok(Err(err)) => {
            tracing::warn!(authority = %origin.authority(), ?err, "upstream request failed");
            Ok(error_response(
                StatusCode::BAD_GATEWAY,
                "upstream_error",
                &format!("could not reach the app at {}", origin.authority()),
                Some("make sure the app is still running on the server"),
            ))
        }
        Err(_) => Ok(error_response(
            StatusCode::GATEWAY_TIMEOUT,
            "upstream_timeout",
            &format!("the app at {} took too long to respond", origin.authority()),
            Some("it may be busy or stuck; try again"),
        )),
    }
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

/// The upstream base URL for an origin (`http://host:port`).
fn upstream_base(origin: &Origin) -> String {
    format!("{}://{}", origin.scheme.as_str(), origin.authority())
}

/// Whether a host string refers to the local machine (client loopback).
fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// Rewrite a client `Origin` header to the upstream origin.
///
/// The web view sends its loopback origin; apps that validate `Origin` against
/// their own host would otherwise reject state-changing requests.
fn rewrite_origin(value: &str, origin: &Origin) -> String {
    match url::Url::parse(value) {
        Ok(url) if url.host_str().is_some_and(is_loopback_host) => upstream_base(origin),
        _ => value.to_string(),
    }
}

/// Rewrite a client `Referer` to the upstream origin, keeping the path/query.
fn rewrite_referer(value: &str, origin: &Origin) -> String {
    match url::Url::parse(value) {
        Ok(url) if url.host_str().is_some_and(is_loopback_host) => {
            let mut out = upstream_base(origin);
            out.push_str(url.path());
            if let Some(query) = url.query() {
                out.push('?');
                out.push_str(query);
            }
            out
        }
        _ => value.to_string(),
    }
}

/// Rewrite an absolute `Location` that points back at the origin into a
/// relative one, so the client stays on the loopback origin instead of dialing
/// the machine's real address.
fn rewrite_location(value: &str, origin: &Origin) -> String {
    let Ok(url) = url::Url::parse(value) else {
        return value.to_string();
    };
    let Some(host) = url.host_str() else {
        return value.to_string();
    };
    let same_host = host.eq_ignore_ascii_case(&origin.host)
        || (is_loopback_host(host) && is_loopback_host(&origin.host));
    let same_port = url.port_or_known_default() == Some(origin.port);
    if !(same_host && same_port) {
        return value.to_string();
    }

    let mut out = url.path().to_string();
    if out.is_empty() {
        out.push('/');
    }
    if let Some(query) = url.query() {
        out.push('?');
        out.push_str(query);
    }
    if let Some(fragment) = url.fragment() {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response<ResBody> {
    let body = serde_json::to_vec(&value).unwrap_or_default();
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(full(body))
        .expect("static response is valid")
}

fn error_body(code: &str, message: &str, hint: Option<&str>) -> serde_json::Value {
    let mut value = serde_json::json!({ "error": message, "code": code });
    if let Some(hint) = hint {
        value["hint"] = serde_json::Value::String(hint.to_string());
    }
    value
}

fn error_response(
    status: StatusCode,
    code: &str,
    message: &str,
    hint: Option<&str>,
) -> Response<ResBody> {
    json_response(status, error_body(code, message, hint))
}

fn full(body: impl Into<Bytes>) -> ResBody {
    Full::new(body.into())
        .map_err(|never: Infallible| match never {})
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn error_body_has_code_and_optional_hint() {
        let with_hint = error_body("unknown_app", "unknown app \"x\"", Some("refresh the list"));
        assert_eq!(with_hint["code"], "unknown_app");
        assert_eq!(with_hint["error"], "unknown app \"x\"");
        assert_eq!(with_hint["hint"], "refresh the list");

        let without_hint = error_body("not_found", "no such route", None);
        assert_eq!(without_hint["code"], "not_found");
        assert!(without_hint.get("hint").is_none());
    }

    #[test]
    fn rewrites_absolute_location_to_relative() {
        let origin = Origin::http("127.0.0.1", 8096);
        assert_eq!(
            rewrite_location("http://127.0.0.1:8096/web/index.html", &origin),
            "/web/index.html"
        );
        assert_eq!(rewrite_location("http://127.0.0.1:8096/", &origin), "/");
        assert_eq!(
            rewrite_location("http://127.0.0.1:8096/a?b=c#d", &origin),
            "/a?b=c#d"
        );
        // Loopback alias of the same host/port is also rewritten.
        assert_eq!(rewrite_location("http://localhost:8096/x", &origin), "/x");
        // Other host or port stays absolute.
        assert_eq!(
            rewrite_location("http://127.0.0.1:9999/x", &origin),
            "http://127.0.0.1:9999/x"
        );
        assert_eq!(
            rewrite_location("https://example.com/x", &origin),
            "https://example.com/x"
        );
        // Already-relative stays as-is.
        assert_eq!(rewrite_location("/already", &origin), "/already");
    }

    #[test]
    fn rewrites_location_for_lan_origin() {
        let origin = Origin::http("192.168.1.5", 8123);
        assert_eq!(
            rewrite_location("http://192.168.1.5:8123/panel", &origin),
            "/panel"
        );
        assert_eq!(
            rewrite_location("http://192.168.1.9:8123/panel", &origin),
            "http://192.168.1.9:8123/panel"
        );
    }

    #[test]
    fn rewrites_loopback_origin_and_referer() {
        let origin = Origin::http("127.0.0.1", 8096);
        assert_eq!(
            rewrite_origin("http://127.0.0.1:5523", &origin),
            "http://127.0.0.1:8096"
        );
        assert_eq!(
            rewrite_origin("http://localhost:1234", &origin),
            "http://127.0.0.1:8096"
        );
        // Non-loopback origins are left alone.
        assert_eq!(rewrite_origin("https://example.com", &origin), "https://example.com");
        assert_eq!(
            rewrite_referer("http://127.0.0.1:5523/web/index.html", &origin),
            "http://127.0.0.1:8096/web/index.html"
        );
    }

    #[test]
    fn node_limiter_concurrency_cap() {
        let limiter = NodeLimiter::new(2, 100, Duration::from_secs(1), 1000);
        let p1 = limiter.try_acquire_concurrency();
        assert!(p1.is_some());
        let p2 = limiter.try_acquire_concurrency();
        assert!(p2.is_some());
        let p3 = limiter.try_acquire_concurrency();
        assert!(p3.is_none());
        drop(p1);
        let p4 = limiter.try_acquire_concurrency();
        assert!(p4.is_some());
    }

    #[test]
    fn node_limiter_per_node_isolation() {
        let a = NodeLimiter::new(1, 100, Duration::from_secs(1), 1000);
        let b = NodeLimiter::new(1, 100, Duration::from_secs(1), 1000);
        assert!(a.try_acquire_concurrency().is_some());
        assert!(b.try_acquire_concurrency().is_some());
    }

    #[tokio::test]
    async fn node_limiter_rate_limit() {
        let limiter = NodeLimiter::new(100, 2, Duration::from_millis(50), 10);
        limiter.acquire_rate_token().await;
        limiter.acquire_rate_token().await;
        let start = std::time::Instant::now();
        limiter.acquire_rate_token().await;
        assert!(start.elapsed() >= Duration::from_millis(40));
    }

    #[test]
    fn serve_handler_get_or_create_limiter() {
        let config = Arc::new(RwLock::new(Config::default()));
        let catalog = Arc::new(RwLock::new(Catalog::empty()));
        let state = Arc::new(AppState::new(config, catalog));
        let auth = Arc::new(AuthState::load(10).unwrap());
        let config_generation = Arc::new(AtomicU64::new(0));
        let handler = ServeHandler::new(
            state,
            auth,
            config_generation,
            Arc::new(AtomicUsize::new(0)),
        );
        let node_a = iroh::SecretKey::from_bytes(&[1; 32]).public();
        let node_b = iroh::SecretKey::from_bytes(&[2; 32]).public();
        let limiter_a1 = handler.get_or_create_limiter(node_a);
        let limiter_a2 = handler.get_or_create_limiter(node_a);
        let limiter_b = handler.get_or_create_limiter(node_b);
        assert!(Arc::ptr_eq(&limiter_a1, &limiter_a2));
        assert!(!Arc::ptr_eq(&limiter_a1, &limiter_b));
    }

    #[test]
    fn limiter_cache_invalidated_on_reload() {
        let config = Arc::new(RwLock::new(Config::default()));
        let catalog = Arc::new(RwLock::new(Catalog::empty()));
        let state = Arc::new(AppState::new(config.clone(), catalog));
        let auth = Arc::new(AuthState::load(10).unwrap());
        let config_generation = Arc::new(AtomicU64::new(0));
        let handler = ServeHandler::new(
            state,
            auth,
            config_generation.clone(),
            Arc::new(AtomicUsize::new(0)),
        );

        let node = iroh::SecretKey::from_bytes(&[1; 32]).public();
        let limiter1 = handler.get_or_create_limiter(node);

        // Simulate config reload
        config_generation.fetch_add(1, Ordering::Relaxed);
        let limiter2 = handler.get_or_create_limiter(node);

        // After reload, a new limiter should be created (old one dropped from cache)
        assert!(!Arc::ptr_eq(&limiter1, &limiter2));
    }
}
