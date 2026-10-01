//! Pairing: the token, the `tethertone://` link the QR code carries, and the
//! order the computer's addresses are offered in. Matches the macOS app, so a
//! phone sees no difference between the two.

use std::fs;
use std::io;
use std::net::Ipv4Addr;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingInfo {
    pub hosts: Vec<String>,
    pub port: u16,
    pub token: String,
    pub rate: u32,
    pub channels: u16,
    pub name: String,
}

impl PairingInfo {
    /// `tethertone://p?h=…&h=…&p=…&t=…&r=…&c=…&n=…` (PROTOCOL.md §3).
    pub fn uri(&self) -> String {
        let mut parts: Vec<String> = self.hosts.iter().map(|h| format!("h={}", percent_encode(h))).collect();
        parts.push(format!("p={}", self.port));
        parts.push(format!("t={}", percent_encode(&self.token)));
        parts.push(format!("r={}", self.rate));
        parts.push(format!("c={}", self.channels));
        if !self.name.is_empty() {
            parts.push(format!("n={}", percent_encode(&self.name)));
        }
        format!("tethertone://p?{}", parts.join("&"))
    }
}

/// Everything outside RFC 3986's unreserved set is escaped, as on macOS.
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `~/.tethertone`, shared with the macOS app and tools/probe.py.
pub fn config_dir() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".tethertone")
}

pub fn token_path() -> PathBuf {
    config_dir().join("token")
}

/// Loads the token, creating one on first run. `rotate` replaces it, which
/// invalidates every phone already paired.
pub fn load_token(rotate: bool) -> io::Result<String> {
    let path = token_path();
    if !rotate {
        if let Ok(existing) = fs::read_to_string(&path) {
            let trimmed = existing.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }
    let token = new_token()?;
    fs::create_dir_all(config_dir())?;
    fs::write(&path, &token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(config_dir(), fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(token)
}

/// 16 random bytes from the OS, base64url without padding.
pub fn new_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(base64url(&bytes))
}

pub fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 2);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        let emit = chunk.len() + 1;
        for i in 0..emit {
            out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// Constant-time comparison: the token is all that stands between the stream
/// and everyone else on the network.
pub fn token_matches(candidate: &str, expected: &str) -> bool {
    let (a, b) = (candidate.as_bytes(), expected.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// Tailscale and most mesh VPNs use the carrier-grade NAT range 100.64/10.
pub fn is_mesh_vpn(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}

/// Interface names that are VPN tunnels on Linux, Windows or macOS.
fn is_tunnel_interface(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["tun", "tap", "wg", "utun", "ipsec", "tailscale", "zerotier", "zt"].iter().any(|p| n.starts_with(p))
}

/// Interfaces a phone can never route through.
fn is_skipped_interface(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["lo", "docker", "br-", "veth", "virbr", "vmnet", "vboxnet", "awdl", "llw"].iter().any(|p| n.starts_with(p))
        || n.contains("loopback")
}

/// The addresses to advertise, physical first and VPN after: on the same
/// network the LAN address is faster, and the phone races them all anyway.
pub fn order_addresses(interfaces: &[(String, Ipv4Addr)]) -> Vec<String> {
    let mut lan = Vec::new();
    let mut vpn = Vec::new();
    for (name, ip) in interfaces {
        if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || is_skipped_interface(name) {
            continue;
        }
        let text = ip.to_string();
        let bucket = if is_tunnel_interface(name) || is_mesh_vpn(ip) { &mut vpn } else { &mut lan };
        if !bucket.contains(&text) {
            bucket.push(text);
        }
    }
    lan.into_iter().chain(vpn).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_matches_the_macos_app() {
        let info = PairingInfo {
            hosts: vec!["127.0.0.1".into(), "10.8.22.255".into()],
            port: 45678,
            token: "tok en+/=".into(),
            rate: 48000,
            channels: 2,
            name: "Studio Mac's".into(),
        };
        assert_eq!(
            info.uri(),
            "tethertone://p?h=127.0.0.1&h=10.8.22.255&p=45678&t=tok%20en%2B%2F%3D&r=48000&c=2&n=Studio%20Mac%27s"
        );
    }

    #[test]
    fn tokens_are_sixteen_random_bytes() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_eq!(a.len(), 22);
        assert_ne!(a, b);
        assert!(a.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
    }

    #[test]
    fn base64url_matches_rfc4648() {
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn token_comparison() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "abcd"));
    }

    #[test]
    fn physical_addresses_come_before_vpn_ones() {
        let ifs = vec![
            ("tailscale0".to_string(), Ipv4Addr::new(100, 90, 1, 2)),
            ("lo".to_string(), Ipv4Addr::new(127, 0, 0, 1)),
            ("wlan0".to_string(), Ipv4Addr::new(192, 168, 1, 4)),
            ("docker0".to_string(), Ipv4Addr::new(172, 17, 0, 1)),
            ("eth0".to_string(), Ipv4Addr::new(169, 254, 3, 3)),
            ("Ethernet".to_string(), Ipv4Addr::new(10, 0, 0, 5)),
        ];
        assert_eq!(order_addresses(&ifs), vec!["192.168.1.4", "10.0.0.5", "100.90.1.2"]);
    }
}
