use if_addrs::{get_if_addrs, IfAddr};
use serde::Serialize;
use std::{net::{IpAddr, Ipv4Addr, Ipv6Addr}, process::Command};

#[derive(Debug, Clone, Serialize)]
pub struct LocalAddresses {
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    pub ipv6_status: String,
}

#[derive(Debug, Clone)]
struct AddressCandidate {
    address: IpAddr,
    interface_name: String,
}

pub fn get_local_addresses() -> Result<LocalAddresses, String> {
    let interfaces = get_if_addrs().map_err(|error| error.to_string())?;
    let candidates: Vec<AddressCandidate> = interfaces
        .into_iter()
        .map(|interface| AddressCandidate {
            address: match interface.addr {
                IfAddr::V4(address) => IpAddr::V4(address.ip),
                IfAddr::V6(address) => IpAddr::V6(address.ip),
            },
            interface_name: interface.name,
        })
        .collect();

    let ipv4 = candidates
        .iter()
        .filter_map(|candidate| match candidate.address {
            IpAddr::V4(address) if !address.is_loopback() && !address.is_link_local() => Some((
                ipv4_score(address, &candidate.interface_name),
                address.to_string(),
            )),
            _ => None,
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, address)| address);

    let stable_ipv6_addresses = linux_stable_ipv6_addresses();
    let ipv6_result = candidates
        .iter()
        .filter_map(|candidate| match candidate.address {
            IpAddr::V6(address) if !address.is_loopback() && !address.is_unspecified() => Some((
                ipv6_score(address, &candidate.interface_name, &stable_ipv6_addresses),
                address.to_string(),
            )),
            _ => None,
        })
        .min_by_key(|(score, _)| *score);

    let (ipv6, ipv6_status) = match ipv6_result {
        Some((score, address)) if score < 300 => (Some(address), "stable".to_string()),
        Some((_, address)) => (Some(address), "link_local_only".to_string()),
        None => (None, "unavailable".to_string()),
    };

    Ok(LocalAddresses {
        ipv4,
        ipv6,
        ipv6_status,
    })
}

fn ipv4_score(address: Ipv4Addr, interface_name: &str) -> u16 {
    let base = if is_preferred_private_ipv4(address) {
        0
    } else if is_private_ipv4(address) {
        100
    } else {
        200
    };

    base + interface_penalty(interface_name)
}

fn ipv6_score(
    address: Ipv6Addr,
    interface_name: &str,
    stable_ipv6_addresses: &[Ipv6Addr],
) -> u16 {
    let segments = address.segments();
    let first = segments[0];
    let base = if (first & 0xfe00) == 0xfc00 {
        0
    } else if stable_ipv6_addresses.contains(&address) {
        100
    } else if (first & 0xe000) == 0x2000 {
        250
    } else if (first & 0xffc0) == 0xfe80 {
        300
    } else {
        400
    };

    base + interface_penalty(interface_name)
}

fn linux_stable_ipv6_addresses() -> Vec<Ipv6Addr> {
    let output = match Command::new("ip")
        .args(["-6", "addr", "show", "scope", "global"])
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);

    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if !trimmed.starts_with("inet6 ")
                || trimmed.contains(" temporary ")
                || trimmed.contains(" deprecated ")
            {
                return None;
            }

            trimmed
                .split_whitespace()
                .nth(1)
                .and_then(|address| address.split('/').next())
                .and_then(|address| address.parse::<Ipv6Addr>().ok())
        })
        .collect()
}

fn is_private_ipv4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 10
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168)
}

fn is_preferred_private_ipv4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    (octets[0] == 192 && octets[1] == 168)
        || octets[0] == 10
        || (octets[0] == 172
            && (16..=31).contains(&octets[1])
            && octets[1] != 17
            && octets[1] != 18)
}

fn interface_penalty(interface_name: &str) -> u16 {
    let lower = interface_name.to_ascii_lowercase();
    let virtual_markers = [
        "docker",
        "veth",
        "br-",
        "virbr",
        "vmnet",
        "vbox",
        "wsl",
        "tailscale",
        "tun",
        "tap",
        "zt",
        "ham",
        "bridge",
    ];

    if virtual_markers.iter().any(|marker| lower.contains(marker)) {
        500
    } else {
        0
    }
}
