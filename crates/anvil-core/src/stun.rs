//! Minimal RFC 5389 STUN binding-request client.
//!
//! One job: ask a STUN server "what's my public IP?" so we can put it in
//! the SDP `c=` line of outbound INVITEs. Symmetric NAT and per-call
//! discovery (each RTP socket gets its own external mapping) are out of
//! scope for this milestone — they'd need ICE proper.

use std::net::SocketAddr;
use std::time::Duration;

use rand::{thread_rng, RngCore};
use tokio::net::UdpSocket;

const STUN_MAGIC_COOKIE: u32 = 0x2112_A442;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;

/// Run a single binding request against `server`. The request goes out
/// via an ephemeral local socket — that means the returned IP is our
/// public IP but the port is the ephemeral port's mapping, not the SIP
/// or RTP port's. For most NATs (full-cone, restricted-cone) the IP is
/// stable across sockets; the RTP port we actually use will get its
/// own NAT mapping when the first packet leaves on it.
pub(crate) async fn discover_public_addr(
    server: &str,
    timeout: Duration,
) -> Result<SocketAddr, anyhow::Error> {
    // Resolve `host:port` (lookup_host accepts both ip:port and dns:port).
    let server_addr = tokio::net::lookup_host(server)
        .await
        .map_err(|e| anyhow::anyhow!("STUN lookup {server}: {e}"))?
        .next()
        .ok_or_else(|| anyhow::anyhow!("STUN lookup {server}: no addresses"))?;

    let bind_local: std::net::SocketAddr = match server_addr {
        SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap(),
        SocketAddr::V6(_) => "[::]:0".parse().unwrap(),
    };
    let socket = UdpSocket::bind(bind_local)
        .await
        .map_err(|e| anyhow::anyhow!("STUN bind: {e}"))?;

    // Build a 20-byte binding request: type=0x0001, length=0,
    // magic cookie, 96-bit transaction id (rng).
    let mut tx_id = [0u8; 12];
    thread_rng().fill_bytes(&mut tx_id);
    let mut request = [0u8; 20];
    request[0..2].copy_from_slice(&0x0001u16.to_be_bytes()); // Binding Request
                                                             // bytes 2..4 are length=0 already
    request[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    request[8..20].copy_from_slice(&tx_id);

    socket
        .send_to(&request, server_addr)
        .await
        .map_err(|e| anyhow::anyhow!("STUN send_to: {e}"))?;

    // Wait for the response. Re-transmit isn't needed for a single shot
    // over a typical RTT; keep it simple.
    let mut buf = vec![0u8; 1500];
    let (len, _peer) = tokio::time::timeout(timeout, socket.recv_from(&mut buf))
        .await
        .map_err(|_| anyhow::anyhow!("STUN response timed out after {timeout:?}"))?
        .map_err(|e| anyhow::anyhow!("STUN recv: {e}"))?;

    parse_binding_response(&buf[..len], &tx_id)
}

fn parse_binding_response(data: &[u8], expected_tx_id: &[u8; 12]) -> anyhow::Result<SocketAddr> {
    if data.len() < 20 {
        anyhow::bail!("STUN response too short: {} bytes", data.len());
    }
    let msg_type = u16::from_be_bytes([data[0], data[1]]);
    if msg_type != 0x0101 {
        anyhow::bail!("STUN response type {msg_type:#06x} (expected 0x0101 Binding Success)");
    }
    let attrs_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    if data[4..8] != STUN_MAGIC_COOKIE.to_be_bytes() {
        anyhow::bail!("STUN response: bad magic cookie");
    }
    if &data[8..20] != expected_tx_id {
        anyhow::bail!("STUN response: transaction-id mismatch");
    }

    let mut cursor = 20usize;
    let end = (20 + attrs_len).min(data.len());
    while cursor + 4 <= end {
        let attr_type = u16::from_be_bytes([data[cursor], data[cursor + 1]]);
        let attr_len = u16::from_be_bytes([data[cursor + 2], data[cursor + 3]]) as usize;
        let attr_start = cursor + 4;
        let attr_end = attr_start + attr_len;
        if attr_end > end {
            anyhow::bail!("STUN attribute {attr_type:#06x} length overruns message");
        }

        if attr_type == ATTR_XOR_MAPPED_ADDRESS {
            return parse_xor_mapped(&data[attr_start..attr_end]);
        }
        if attr_type == ATTR_MAPPED_ADDRESS {
            // RFC 5389 requires XOR-MAPPED, but some servers still emit the
            // legacy form. Accept both.
            return parse_mapped(&data[attr_start..attr_end]);
        }

        // Pad to 4-byte boundary.
        cursor = attr_end + ((4 - (attr_len % 4)) % 4);
    }

    anyhow::bail!("STUN response had no MAPPED-ADDRESS attribute")
}

fn parse_xor_mapped(value: &[u8]) -> anyhow::Result<SocketAddr> {
    if value.len() < 8 {
        anyhow::bail!("XOR-MAPPED-ADDRESS too short");
    }
    let family = value[1];
    let xport = u16::from_be_bytes([value[2], value[3]]);
    let port = xport ^ ((STUN_MAGIC_COOKIE >> 16) as u16);

    match family {
        0x01 => {
            // IPv4: 4 bytes XOR'd with magic cookie
            let xaddr = u32::from_be_bytes([value[4], value[5], value[6], value[7]]);
            let addr = std::net::Ipv4Addr::from(xaddr ^ STUN_MAGIC_COOKIE);
            Ok(SocketAddr::new(addr.into(), port))
        }
        0x02 => {
            // IPv6: 16 bytes XOR'd with (magic_cookie || transaction_id).
            // We don't have the tx_id here; bail. IPv6 NAT is rare anyway.
            anyhow::bail!("IPv6 XOR-MAPPED-ADDRESS not implemented")
        }
        other => anyhow::bail!("unknown address family in XOR-MAPPED-ADDRESS: {other}"),
    }
}

fn parse_mapped(value: &[u8]) -> anyhow::Result<SocketAddr> {
    if value.len() < 8 {
        anyhow::bail!("MAPPED-ADDRESS too short");
    }
    let family = value[1];
    let port = u16::from_be_bytes([value[2], value[3]]);
    if family == 0x01 {
        let addr = std::net::Ipv4Addr::new(value[4], value[5], value[6], value[7]);
        Ok(SocketAddr::new(addr.into(), port))
    } else {
        anyhow::bail!("MAPPED-ADDRESS family {family} not supported")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xor_mapped_ipv4() {
        // Build a synthetic Binding Success with XOR-MAPPED-ADDRESS
        // for 198.51.100.42:5060.
        let real_addr = std::net::Ipv4Addr::new(198, 51, 100, 42);
        let real_port = 5060u16;

        let xport = real_port ^ ((STUN_MAGIC_COOKIE >> 16) as u16);
        let xaddr = u32::from(real_addr) ^ STUN_MAGIC_COOKIE;

        let tx_id = [9u8; 12];
        let mut msg = Vec::new();
        msg.extend_from_slice(&0x0101u16.to_be_bytes()); // Binding Success
        msg.extend_from_slice(&12u16.to_be_bytes()); // attrs_len
        msg.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
        msg.extend_from_slice(&tx_id);
        msg.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        msg.extend_from_slice(&8u16.to_be_bytes()); // attr length
        msg.push(0x00); // reserved
        msg.push(0x01); // family IPv4
        msg.extend_from_slice(&xport.to_be_bytes());
        msg.extend_from_slice(&xaddr.to_be_bytes());

        let parsed = parse_binding_response(&msg, &tx_id).expect("parse ok");
        assert_eq!(parsed, SocketAddr::new(real_addr.into(), real_port));
    }

    #[test]
    fn rejects_wrong_transaction_id() {
        let mut msg = vec![0u8; 20];
        msg[0..2].copy_from_slice(&0x0101u16.to_be_bytes());
        msg[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
        msg[8..20].copy_from_slice(&[1u8; 12]);

        let expected_tx = [2u8; 12];
        assert!(parse_binding_response(&msg, &expected_tx).is_err());
    }
}
