//! Enumerating and filtering the OS listening-socket table.

use std::collections::HashSet;
use std::net::SocketAddr;

use anyhow::Result;

use super::model::Origin;

/// A candidate origin plus the process that owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The listening origin.
    pub origin: Origin,
    /// Owning process name, when known.
    pub process: Option<String>,
    /// Owning process id, when known.
    pub pid: Option<u32>,
}

/// Filter applied to the OS listening-socket table.
#[derive(Debug, Clone)]
pub struct ListenerFilter {
    /// Ignore ports below this value.
    pub min_port: u16,
    /// Only keep loopback/wildcard binds.
    pub loopback_only: bool,
    /// Ports to skip (built-in denylist plus user excludes).
    pub exclude_ports: HashSet<u16>,
    /// Process names to skip (lowercased).
    pub exclude_processes: HashSet<String>,
    /// Origins to skip entirely (built-in none, user-configured).
    pub exclude_origins: HashSet<Origin>,
}

impl ListenerFilter {
    /// Build a filter, folding in the built-in denylists and lowercasing
    /// process names.
    pub fn new(
        min_port: u16,
        loopback_only: bool,
        exclude_ports: &[u16],
        exclude_processes: &[String],
        exclude_origins: &[String],
    ) -> Self {
        Self {
            min_port,
            loopback_only,
            // Ports that can never be a user web app.
            exclude_ports: BUILTIN_EXCLUDE_PORTS
                .iter()
                .copied()
                .chain(exclude_ports.iter().copied())
                .collect(),
            exclude_processes: BUILTIN_EXCLUDE_PROCESSES
                .iter()
                .map(|p| p.to_ascii_lowercase())
                .chain(exclude_processes.iter().map(|p| p.to_ascii_lowercase()))
                .collect(),
            exclude_origins: exclude_origins
                .iter()
                .filter_map(|s| {
                    let origin = Origin::parse_authority(s);
                    if origin.is_none() {
                        tracing::warn!(
                            value = %s,
                            "ignoring invalid discovery.exclude_origins entry (expected host:port)"
                        );
                    }
                    origin
                })
                .collect(),
        }
    }
}

/// Ports that are never exposed: remote-access, databases, printers, etc.
pub const BUILTIN_EXCLUDE_PORTS: &[u16] = &[
    22,    // ssh
    25,    // smtp
    53,    // dns
    88,    // kerberos
    110,   // pop3
    111,   // rpcbind
    135,   // msrpc
    137, 138, 139, // netbios
    143,   // imap
    161, 162, // snmp
    389,   // ldap
    445,   // smb
    465,   // smtps
    514,   // syslog
    515,   // lpd
    548,   // afp
    587,   // submission
    631,   // ipp
    636,   // ldaps
    873,   // rsync
    993,   // imaps
    995,   // pop3s
    1080,  // socks
    1433,  // mssql
    1521,  // oracle
    1723,  // pptp
    2049,  // nfs
    2375, 2376, // docker daemon
    3306,  // mysql
    3389,  // rdp
    5000,  // airplay receiver
    5353,  // mdns
    5432,  // postgres
    5900,  // vnc
    6379,  // redis
    7000,  // airplay
    9200, 9300, // elasticsearch
    27017, // mongodb
];

/// Local forward-proxy / tunnel processes that open a listening socket but are
/// not user web apps.
///
/// Deliberately conservative: only names that are essentially always a proxy or
/// tunnel. `docker-proxy` is intentionally **not** listed, because Docker
/// publishes real web apps through it.
pub const BUILTIN_EXCLUDE_PROCESSES: &[&str] = &[
    "xray",
    "v2ray",
    "v2ray-core",
    "sing-box",
    "clash",
    "clash-meta",
    "mihomo",
    "shadowsocks",
    "ss-local",
    "ssserver",
    "ss-tunnel",
    "trojan",
    "trojan-go",
    "hysteria",
    "hysteria2",
    "naive",
    "tun2socks",
    "mitmproxy",
    "mitmdump",
    "privoxy",
    "squid",
    "tinyproxy",
    "3proxy",
    "tor",
];

/// Enumerate locally-listening TCP sockets and filter them down to plausible
/// web-app candidates. Blocking (OS calls); call via `spawn_blocking`.
pub fn enumerate(filter: &ListenerFilter) -> Result<Vec<Candidate>> {
    let all: Vec<listeners::Listener> = listeners::get_all()
        .map_err(|e| anyhow::anyhow!("failed to enumerate listeners: {e}"))?
        .into_iter()
        .collect();
    Ok(filter_listeners(&all, filter))
}

/// Pure filtering step, split out for testing without OS access.
pub fn filter_listeners(
    all: &[listeners::Listener],
    filter: &ListenerFilter,
) -> Vec<Candidate> {
    let mut seen: HashSet<Origin> = HashSet::new();
    let mut out = Vec::new();

    for l in all {
        if l.protocol != listeners::Protocol::TCP {
            continue;
        }
        if l.state != listeners::SocketState::Listen {
            continue;
        }
        let port = l.socket.port();
        if port == 0 || port < filter.min_port {
            continue;
        }
        if filter.exclude_ports.contains(&port) {
            continue;
        }
        let process_name = l.process.name.trim();
        if !process_name.is_empty()
            && filter.exclude_processes.contains(&process_name.to_ascii_lowercase())
        {
            continue;
        }
        if filter.loopback_only && !is_local_bind(&l.socket) {
            continue;
        }

        let origin = Origin::http(Origin::local_host(l.socket), port);
        if filter.exclude_origins.contains(&origin) {
            continue;
        }
        if !seen.insert(origin.clone()) {
            continue;
        }
        out.push(Candidate {
            origin,
            process: if process_name.is_empty() {
                None
            } else {
                Some(process_name.to_string())
            },
            pid: Some(l.process.pid),
        });
    }

    // Deterministic order (by port) so names/catalog stay stable across scans.
    out.sort_by(|a, b| a.origin.port.cmp(&b.origin.port));
    out
}

fn is_local_bind(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback() || addr.ip().is_unspecified()
}

#[cfg(test)]
mod tests {
    use super::*;
    use listeners::{Listener, Process, Protocol, SocketState};

    fn listener(addr: &str, proto: Protocol, state: SocketState, name: &str) -> Listener {
        Listener {
            process: Process {
                pid: 42,
                name: name.to_string(),
                path: String::new(),
            },
            socket: addr.parse().unwrap(),
            protocol: proto,
            state,
        }
    }

    fn filter() -> ListenerFilter {
        ListenerFilter::new(1024, true, &[], &[], &[])
    }

    #[test]
    fn keeps_normal_loopback_http_port() {
        let all = vec![listener(
            "127.0.0.1:5173",
            Protocol::TCP,
            SocketState::Listen,
            "node",
        )];
        let got = filter_listeners(&all, &filter());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].origin.host, "127.0.0.1");
        assert_eq!(got[0].origin.port, 5173);
        assert_eq!(got[0].process.as_deref(), Some("node"));
    }

    #[test]
    fn skips_non_tcp_non_listen_and_zero_port() {
        let all = vec![
            listener("127.0.0.1:5173", Protocol::UDP, SocketState::Unknown, "x"),
            listener("127.0.0.1:5174", Protocol::TCP, SocketState::Established, "x"),
            listener("127.0.0.1:0", Protocol::TCP, SocketState::Listen, "x"),
        ];
        assert!(filter_listeners(&all, &filter()).is_empty());
    }

    #[test]
    fn skips_low_and_denylisted_ports() {
        let all = vec![
            listener("127.0.0.1:80", Protocol::TCP, SocketState::Listen, "x"),
            listener("127.0.0.1:3306", Protocol::TCP, SocketState::Listen, "mysql"),
        ];
        assert!(filter_listeners(&all, &filter()).is_empty());
    }

    #[test]
    fn skips_excluded_process() {
        let f = ListenerFilter::new(1024, true, &[], &["node".to_string()], &[]);
        let all = vec![listener(
            "127.0.0.1:5173",
            Protocol::TCP,
            SocketState::Listen,
            "Node",
        )];
        assert!(filter_listeners(&all, &f).is_empty());
    }

    #[test]
    fn loopback_only_skips_lan_bind() {
        let all = vec![listener(
            "192.168.1.5:8000",
            Protocol::TCP,
            SocketState::Listen,
            "x",
        )];
        assert!(filter_listeners(&all, &filter()).is_empty());

        // ...but is kept when loopback_only is off.
        let f = ListenerFilter::new(1024, false, &[], &[], &[]);
        let got = filter_listeners(&all, &f);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].origin.host, "192.168.1.5");
    }

    #[test]
    fn skips_builtin_proxy_process() {
        let all = vec![listener(
            "127.0.0.1:10808",
            Protocol::TCP,
            SocketState::Listen,
            "xray",
        )];
        assert!(filter_listeners(&all, &filter()).is_empty());
    }

    #[test]
    fn skips_excluded_origin() {
        let f = ListenerFilter::new(1024, true, &[], &[], &["127.0.0.1:10808".to_string()]);
        let all = vec![
            listener("127.0.0.1:10808", Protocol::TCP, SocketState::Listen, "x"),
            listener("127.0.0.1:3000", Protocol::TCP, SocketState::Listen, "x"),
        ];
        let got = filter_listeners(&all, &f);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].origin.port, 3000);
    }

    #[test]
    fn excluded_origin_host_is_normalized() {
        // A wildcard bind is reported as 127.0.0.1, so excluding localhost works.
        let f = ListenerFilter::new(1024, true, &[], &[], &["localhost:8000".to_string()]);
        let all = vec![listener(
            "0.0.0.0:8000",
            Protocol::TCP,
            SocketState::Listen,
            "x",
        )];
        assert!(filter_listeners(&all, &f).is_empty());
    }

    #[test]
    fn dedupes_wildcard_v4_and_v6() {
        let all = vec![
            listener("0.0.0.0:8000", Protocol::TCP, SocketState::Listen, "x"),
            listener("[::]:8000", Protocol::TCP, SocketState::Listen, "x"),
        ];
        let got = filter_listeners(&all, &filter());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].origin.host, "127.0.0.1");
    }
}
