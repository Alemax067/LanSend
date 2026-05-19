use serde::{Deserialize, Serialize};
use std::{fs, net::IpAddr, path::PathBuf, time::Duration};
use uuid::Uuid;

use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressType {
    Ipv4,
    Ipv6,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerStatus {
    Unknown,
    Online,
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub os: String,
    pub arch: String,
    pub app_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub id: Uuid,
    pub peer_id: Option<String>,
    pub alias: Option<String>,
    pub address_type: AddressType,
    pub host: String,
    pub port: u16,
    pub status: PeerStatus,
    pub last_seen: Option<String>,
    pub system_info: Option<SystemInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddPeerRequest {
    pub address_type: AddressType,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
struct HelloResponse {
    device_id: String,
    alias: String,
    app_version: String,
    os: String,
    arch: String,
}

pub async fn add_peer(request: AddPeerRequest) -> Result<Peer, String> {
    validate_peer_input(&request)?;

    let mut peers = load_peers()?;
    let mut peer = Peer {
        id: Uuid::new_v4(),
        peer_id: None,
        alias: None,
        address_type: request.address_type,
        host: normalize_host(&request.host),
        port: request.port,
        status: PeerStatus::Unknown,
        last_seen: None,
        system_info: None,
    };

    if let Ok(probed) = probe_peer_inner(peer.clone()).await {
        peer = probed;
    }

    peers.push(peer.clone());
    save_peers(&peers)?;
    Ok(peer)
}

pub fn list_peers() -> Result<Vec<Peer>, String> {
    load_peers()
}

pub fn remove_peer(id: Uuid) -> Result<Vec<Peer>, String> {
    let mut peers = load_peers()?;
    peers.retain(|peer| peer.id != id);
    save_peers(&peers)?;
    Ok(peers)
}

pub async fn update_peer(id: Uuid, request: AddPeerRequest) -> Result<Peer, String> {
    validate_peer_input(&request)?;

    let mut peers = load_peers()?;
    let index = peers
        .iter()
        .position(|peer| peer.id == id)
        .ok_or_else(|| "设备不存在".to_string())?;
    let original = peers[index].clone();
    let mut peer = Peer {
        id,
        peer_id: original.peer_id,
        alias: original.alias,
        address_type: request.address_type,
        host: normalize_host(&request.host),
        port: request.port,
        status: PeerStatus::Unknown,
        last_seen: original.last_seen,
        system_info: original.system_info,
    };

    if let Ok(probed) = probe_peer_inner(peer.clone()).await {
        peer = probed;
    }

    peers[index] = peer.clone();
    save_peers(&peers)?;
    Ok(peer)
}

pub async fn probe_peer(id: Uuid) -> Result<Peer, String> {
    let mut peers = load_peers()?;
    let index = peers
        .iter()
        .position(|peer| peer.id == id)
        .ok_or_else(|| "设备不存在".to_string())?;

    let original = peers[index].clone();
    let peer = match probe_peer_inner(original.clone()).await {
        Ok(peer) => peer,
        Err(_) => {
            let mut peer = original;
            peer.status = PeerStatus::Offline;
            peer
        }
    };

    peers[index] = peer.clone();
    save_peers(&peers)?;
    Ok(peer)
}

pub async fn probe_all_peers() -> Result<Vec<Peer>, String> {
    let peers = load_peers()?;
    let mut updated = Vec::with_capacity(peers.len());

    for peer in peers {
        let probed = match probe_peer_inner(peer.clone()).await {
            Ok(peer) => peer,
            Err(_) => {
                let mut peer = peer;
                peer.status = PeerStatus::Offline;
                peer
            }
        };
        updated.push(probed);
    }

    save_peers(&updated)?;
    Ok(updated)
}

pub async fn test_peer(request: AddPeerRequest) -> Result<Peer, String> {
    validate_peer_input(&request)?;

    let peer = Peer {
        id: Uuid::new_v4(),
        peer_id: None,
        alias: None,
        address_type: request.address_type,
        host: normalize_host(&request.host),
        port: request.port,
        status: PeerStatus::Unknown,
        last_seen: None,
        system_info: None,
    };

    probe_peer_inner(peer).await
}

async fn probe_peer_inner(mut peer: Peer) -> Result<Peer, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(format!("{}/api/v1/hello", base_url(&peer)))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json::<HelloResponse>()
        .await
        .map_err(|error| error.to_string())?;

    peer.peer_id = Some(response.device_id);
    peer.alias = Some(response.alias);
    peer.status = PeerStatus::Online;
    peer.last_seen = Some(now_text());
    peer.system_info = Some(SystemInfo {
        os: response.os,
        arch: response.arch,
        app_version: response.app_version,
    });

    Ok(peer)
}

fn load_peers() -> Result<Vec<Peer>, String> {
    let path = peers_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&content).map_err(|error| error.to_string())
}

fn save_peers(peers: &[Peer]) -> Result<(), String> {
    let path = peers_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let content = serde_json::to_string_pretty(peers).map_err(|error| error.to_string())?;
    fs::write(path, content).map_err(|error| error.to_string())
}

fn peers_path() -> Result<PathBuf, String> {
    Ok(config::app_config_dir()?.join("peers.json"))
}

fn validate_peer_input(request: &AddPeerRequest) -> Result<(), String> {
    let host = normalize_host(&request.host);
    if host.is_empty() {
        return Err("请输入设备地址".to_string());
    }

    if !(1024..=49151).contains(&request.port) {
        return Err("端口必须在 1024 到 49151 之间".to_string());
    }

    let host_without_zone = host.split('%').next().unwrap_or(&host);
    match (host_without_zone.parse::<IpAddr>(), &request.address_type) {
        (Ok(IpAddr::V4(_)), AddressType::Ipv4) => Ok(()),
        (Ok(IpAddr::V6(_)), AddressType::Ipv6) => Ok(()),
        (Ok(IpAddr::V4(_)), AddressType::Ipv6) => {
            Err("地址类型选择了 IPv6，但输入的是 IPv4".to_string())
        }
        (Ok(IpAddr::V6(_)), AddressType::Ipv4) => {
            Err("地址类型选择了 IPv4，但输入的是 IPv6".to_string())
        }
        (Err(_), _) => Err("请输入有效的 IP 地址".to_string()),
    }
}

fn normalize_host(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string()
}

fn base_url(peer: &Peer) -> String {
    match peer.address_type {
        AddressType::Ipv4 => format!("http://{}:{}", peer.host, peer.port),
        AddressType::Ipv6 => format!("http://[{}]:{}", peer.host, peer.port),
    }
}

fn now_text() -> String {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs().to_string(),
        Err(_) => "0".to_string(),
    }
}
