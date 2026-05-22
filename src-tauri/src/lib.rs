mod config;
mod network;
mod peer;
mod server;
mod transfer;

use config::{AppConfig, UpdateAppConfig};
use network::LocalAddresses;
use peer::{AddPeerRequest, Peer};
use std::sync::Arc;
use transfer::{
    ClipboardTextPayload, SelectedFileInfo, SendFileRequest, TransferDecision, TransferFileMeta,
    TransferOffer, TransferOfferResponse, TransferState, UploadResult,
};
use uuid::Uuid;

#[tauri::command]
fn get_app_config() -> Result<AppConfig, String> {
    config::load_or_create_config()
}

#[tauri::command]
fn update_app_config(update: UpdateAppConfig) -> Result<AppConfig, String> {
    config::update_config(update)
}

#[tauri::command]
fn get_local_addresses() -> Result<LocalAddresses, String> {
    network::get_local_addresses()
}

#[tauri::command]
fn list_peers() -> Result<Vec<Peer>, String> {
    peer::list_peers()
}

#[tauri::command]
async fn add_peer(request: AddPeerRequest) -> Result<Peer, String> {
    peer::add_peer(request).await
}

#[tauri::command]
fn remove_peer(id: Uuid) -> Result<Vec<Peer>, String> {
    peer::remove_peer(id)
}

#[tauri::command]
async fn update_peer(id: Uuid, request: AddPeerRequest) -> Result<Peer, String> {
    peer::update_peer(id, request).await
}

#[tauri::command]
async fn probe_peer(id: Uuid) -> Result<Peer, String> {
    peer::probe_peer(id).await
}

#[tauri::command]
async fn probe_all_peers() -> Result<Vec<Peer>, String> {
    peer::probe_all_peers().await
}

#[tauri::command]
async fn test_peer(request: AddPeerRequest) -> Result<Peer, String> {
    peer::test_peer(request).await
}

#[tauri::command]
async fn send_transfer_offer(
    host: String,
    port: u16,
    address_type: peer::AddressType,
    files: Vec<TransferFileMeta>,
    clipboard_text: Option<ClipboardTextPayload>,
) -> Result<TransferOfferResponse, String> {
    if let Some(clipboard_text) = &clipboard_text {
        transfer::validate_clipboard_text(clipboard_text)?;
    }

    let config = config::load_or_create_config()?;
    let total_size = files.iter().map(|file| file.size).sum::<u64>()
        + clipboard_text.as_ref().map(|payload| payload.size).unwrap_or(0);
    let offer = TransferOffer {
        transfer_id: Uuid::new_v4(),
        sender_id: config.device_id.to_string(),
        sender_alias: config.alias,
        files,
        clipboard_text,
        total_size,
    };
    let base_url = match address_type {
        peer::AddressType::Ipv4 => format!("http://{}:{}", host, port),
        peer::AddressType::Ipv6 => {
            format!("http://[{}]:{}", host.trim_matches(&['[', ']'][..]), port)
        }
    };

    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(130))
        .build()
        .map_err(|error| error.to_string())?
        .post(format!("{base_url}/api/v1/transfer/offer"))
        .json(&offer)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json::<TransferOfferResponse>()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn inspect_files(paths: Vec<String>) -> Result<Vec<SelectedFileInfo>, String> {
    transfer::inspect_files(paths)
}

#[tauri::command]
async fn upload_transfer_files(
    host: String,
    port: u16,
    address_type: peer::AddressType,
    transfer_id: Uuid,
    token: String,
    files: Vec<SendFileRequest>,
) -> Result<Vec<UploadResult>, String> {
    transfer::send_files(host, port, address_type, transfer_id, token, files).await
}

#[tauri::command]
fn decide_transfer(
    decision: TransferDecision,
    state: tauri::State<'_, Arc<TransferState>>,
) -> Result<(), String> {
    transfer::decide_transfer(state.inner(), decision)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let transfer_state = Arc::new(TransferState::default());

    tauri::Builder::default()
        .manage(transfer_state.clone())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            server::start_server(app.handle().clone(), transfer_state.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_config,
            update_app_config,
            get_local_addresses,
            list_peers,
            add_peer,
            remove_peer,
            update_peer,
            probe_peer,
            probe_all_peers,
            test_peer,
            inspect_files,
            send_transfer_offer,
            upload_transfer_files,
            decide_transfer
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
