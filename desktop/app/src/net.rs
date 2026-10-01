//! The addresses this computer can be reached at, in the order phones should try them.

use std::net::IpAddr;

pub fn local_addresses() -> Vec<String> {
    let interfaces: Vec<(String, std::net::Ipv4Addr)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|iface| match iface.ip() {
            IpAddr::V4(ip) => Some((iface.name, ip)),
            IpAddr::V6(_) => None,
        })
        .collect();
    tethertone_core::pairing::order_addresses(&interfaces)
}

pub fn machine_name() -> String {
    gethostname::gethostname().to_string_lossy().trim_end_matches(".local").to_string()
}
