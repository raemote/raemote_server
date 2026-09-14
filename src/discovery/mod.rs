//! Local web-app discovery.
//!
//! [`DiscoveryEngine`] periodically enumerates the OS listening-socket table,
//! filters candidate web servers, probes them over HTTP, and publishes the
//! result to a [`crate::catalog::Catalog`].

pub mod listener;
pub mod model;
pub mod probe;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinSet;

use crate::catalog::Catalog;
use crate::config::Config;

use listener::ListenerFilter;
use model::{DiscoveredApp, Origin};

/// A verified app plus when we last confirmed it.
struct CachedApp {
    app: DiscoveredApp,
    last_verified: Instant,
}

/// Background discovery loop.
///
/// Each cycle enumerates the OS listening-socket table (cheap) and only probes
/// origins that are new or whose `recheck_secs` TTL has lapsed. The catalog is
/// rebuilt only when the set/identity of apps changes.
pub struct DiscoveryEngine {
    config: Arc<RwLock<Config>>,
    catalog: Arc<RwLock<Catalog>>,
    trigger: Arc<Notify>,
    shutdown: Arc<Notify>,
    /// Bumped after every catalog rebuild; lets callers wait for a scan.
    catalog_generation: Arc<AtomicU64>,
    cache: HashMap<Origin, CachedApp>,
}

impl DiscoveryEngine {
    /// Build an engine over shared config and catalog.
    pub fn new(
        config: Arc<RwLock<Config>>,
        catalog: Arc<RwLock<Catalog>>,
        trigger: Arc<Notify>,
        shutdown: Arc<Notify>,
        catalog_generation: Arc<AtomicU64>,
    ) -> Self {
        Self {
            config,
            catalog,
            trigger,
            shutdown,
            catalog_generation,
            cache: HashMap::new(),
        }
    }

    /// Run until shutdown. Respects `discovery.enabled` at every cycle, so it
    /// can be toggled via config reload without restarting the task.
    pub async fn run(mut self) {
        // Scan immediately so apps appear at startup rather than only after a
        // full `interval_secs`.
        self.scan_once().await;
        loop {
            let interval_secs = {
                let cfg = self.config.read().expect("config poisoned");
                cfg.discovery.interval_secs.max(1)
            };
            tokio::select! {
                _ = self.shutdown.notified() => break,
                _ = self.trigger.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(interval_secs)) => {}
            }
            self.scan_once().await;
        }
    }

    /// Run a single discovery cycle.
    pub async fn scan_once(&mut self) {
        let (enabled, filter, probe_timeout, max_concurrent, recheck) = {
            let cfg = self.config.read().expect("config poisoned");
            let d = &cfg.discovery;
            (
                d.enabled,
                ListenerFilter::new(
                    d.min_port,
                    d.loopback_only,
                    &d.exclude_ports,
                    &d.exclude_processes,
                    &d.exclude_origins,
                ),
                Duration::from_millis(d.probe_timeout_ms.max(1)),
                d.max_concurrent_probes.max(1),
                Duration::from_secs(d.recheck_secs.max(1)),
            )
        };

        if !enabled {
            self.cache.clear();
            self.rebuild_catalog();
            return;
        }

        // Enumeration is blocking (OS calls); keep it off the async runtime.
        let candidates = match tokio::task::spawn_blocking(move || listener::enumerate(&filter)).await
        {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                tracing::warn!("discovery enumeration failed: {e:#}");
                return;
            }
            Err(e) => {
                tracing::warn!("discovery enumeration task failed: {e}");
                return;
            }
        };

        // Drop cache entries whose socket is gone.
        let live: HashSet<Origin> = candidates.iter().map(|c| c.origin.clone()).collect();
        self.cache.retain(|origin, _| live.contains(origin));

        // Probe only new or TTL-expired origins.
        let now = Instant::now();
        let mut to_probe = Vec::new();
        for candidate in &candidates {
            let needs_probe = match self.cache.get(&candidate.origin) {
                Some(cached) => now.duration_since(cached.last_verified) >= recheck,
                None => true,
            };
            if needs_probe {
                to_probe.push(candidate.clone());
            }
        }

        if !to_probe.is_empty() {
            let semaphore = Arc::new(Semaphore::new(max_concurrent));
            let mut set = JoinSet::new();
            for candidate in to_probe {
                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(permit) => permit,
                    Err(_) => break,
                };
                set.spawn(async move {
                    let _permit = permit;
                    let result = probe::probe_http(&candidate.origin, probe_timeout).await;
                    (candidate, result)
                });
            }

            while let Some(joined) = set.join_next().await {
                let Ok((candidate, result)) = joined else {
                    continue;
                };
                match result {
                    Some(result) if is_credible(&result) => {
                        self.cache.insert(
                            candidate.origin.clone(),
                            CachedApp {
                                app: DiscoveredApp {
                                    origin: candidate.origin,
                                    title: result.title,
                                    process: candidate.process,
                                    pid: candidate.pid,
                                },
                                last_verified: Instant::now(),
                            },
                        );
                    }
                    Some(_) => {
                        tracing::debug!(
                            origin = %candidate.origin.authority(),
                            "skipping low-confidence candidate (error status, no title)"
                        );
                        self.cache.remove(&candidate.origin);
                    }
                    None => {
                        // Not HTTP / unreachable: ensure it is not exposed.
                        self.cache.remove(&candidate.origin);
                    }
                }
            }
        }

        self.rebuild_catalog();
    }

    fn rebuild_catalog(&self) {
        let manual = self.config.read().expect("config poisoned").apps.clone();
        let discovered: Vec<DiscoveredApp> =
            self.cache.values().map(|c| c.app.clone()).collect();
        let catalog = Catalog::rebuild(&manual, &discovered);
        *self.catalog.write().expect("catalog poisoned") = catalog;
        self.catalog_generation.fetch_add(1, Ordering::Relaxed);
    }
}

/// Whether a probe looks like a real web app rather than, say, a local HTTP
/// proxy answering `GET /` with an error and no page.
///
/// Any non-error status counts (so JSON APIs and redirects are kept), as does
/// any response with a `<title>` (so a login page answering 401/403 is kept).
fn is_credible(result: &probe::ProbeResult) -> bool {
    result.title.is_some() || result.status < 400
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(status: u16, title: Option<&str>) -> probe::ProbeResult {
        probe::ProbeResult {
            status,
            title: title.map(str::to_string),
            server: None,
        }
    }

    #[test]
    fn success_without_title_is_credible() {
        assert!(is_credible(&result(200, None)));
        assert!(is_credible(&result(302, None)));
    }

    #[test]
    fn error_with_title_is_credible() {
        // A login page often answers 401 but still has a title.
        assert!(is_credible(&result(401, Some("Sign in"))));
    }

    #[test]
    fn error_without_title_is_not_credible() {
        // e.g. a proxy answering a bare GET / with 400 and no page.
        assert!(!is_credible(&result(400, None)));
        assert!(!is_credible(&result(404, None)));
        assert!(!is_credible(&result(502, None)));
    }
}
