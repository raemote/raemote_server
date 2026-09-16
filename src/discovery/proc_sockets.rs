//! Reading the kernel's listening-socket table directly (Linux).
//!
//! The `listeners` crate attributes each socket to a process by walking
//! `/proc/<pid>/fd`, which only works for processes the caller may read. A
//! service owned by another user — a root-owned system `nginx` in front of an
//! app, say — is therefore dropped entirely, even though its socket is perfectly
//! usable. `/proc/net/tcp{,6}` is world-readable, so read the sockets here and
//! let the process name stay optional.
//!
//! Parsing is split out from reading so it can be unit-tested anywhere.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Listening TCP sockets, from `/proc/net/tcp` and `/proc/net/tcp6`.
///
/// Empty on platforms that don't expose these tables.
pub fn listening_tcp() -> Vec<SocketAddr> {
    #[cfg(target_os = "linux")]
    {
        let mut out = Vec::new();
        for (path, ipv6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
            match std::fs::read_to_string(path) {
                Ok(table) => out.extend(parse_table(&table, ipv6)),
                Err(err) => tracing::debug!(path, %err, "could not read the socket table"),
            }
        }
        out
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// Parse a `/proc/net/tcp`-style table, keeping only `LISTEN` sockets.
///
/// Each row's local address is a hex address and port; IPv4 addresses are a
/// little-endian `u32`, and IPv6 addresses are four little-endian `u32` words.
pub fn parse_table(table: &str, ipv6: bool) -> Vec<SocketAddr> {
    const STATE_LISTEN: &str = "0A";

    let mut out = Vec::new();
    for line in table.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // sl, local_address, rem_address, st, ...
        if fields.get(3) != Some(&STATE_LISTEN) {
            continue;
        }
        let Some((addr_hex, port_hex)) = fields.get(1).and_then(|local| local.split_once(':')) else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        if port == 0 {
            continue;
        }
        let Some(ip) = parse_ip(addr_hex, ipv6) else {
            continue;
        };
        out.push(SocketAddr::new(ip, port));
    }
    out
}

fn parse_ip(hex: &str, ipv6: bool) -> Option<IpAddr> {
    if ipv6 {
        if hex.len() != 32 {
            return None;
        }
        let mut bytes = [0u8; 16];
        for (index, chunk) in hex.as_bytes().chunks(8).enumerate() {
            let word = u32::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
            bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        Some(IpAddr::V6(Ipv6Addr::from(bytes)))
    } else {
        if hex.len() != 8 {
            return None;
        }
        let word = u32::from_str_radix(hex, 16).ok()?;
        Some(IpAddr::V4(Ipv4Addr::from(word.to_le_bytes())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TCP: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1 1 0000
   1: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 2 1 0000
   2: 0100007F:1F90 0100007F:C350 01 00000000:00000000 00:00000000 00000000     0        0 3 1 0000
   3: 00000000:0000 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 4 1 0000
";

    #[test]
    fn keeps_listening_sockets_only() {
        let got = parse_table(TCP, false);
        // 127.0.0.1:8080 and 0.0.0.0:80; the ESTABLISHED row and port 0 are out.
        assert_eq!(
            got,
            vec![
                "127.0.0.1:8080".parse().unwrap(),
                "0.0.0.0:80".parse().unwrap(),
            ]
        );
    }

    #[test]
    fn parses_ipv6_words_little_endian() {
        // ::1:8080 and [::]:443
        let table = "\
  sl  local_address                         rem_address   st
   0: 00000000000000000000000001000000:1F90 00000000000000000000000000000000:0000 0A
   1: 00000000000000000000000000000000:01BB 00000000000000000000000000000000:0000 0A
";
        assert_eq!(
            parse_table(table, true),
            vec![
                "[::1]:8080".parse().unwrap(),
                "[::]:443".parse().unwrap(),
            ]
        );
    }

    #[test]
    fn ignores_malformed_rows() {
        let table = "\
  sl  local_address rem_address   st
   0: 0100007F 00000000:0000 0A
   1: zzzzzzzz:1F90 00000000:0000 0A
   2: 0100007F:ZZZZ 00000000:0000 0A
";
        assert!(parse_table(table, false).is_empty());
    }
}
