use axum::{
    body::Body,
    extract::{Path, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
};
use tauri::AppHandle;

use crate::{
    config, network,
    transfer::{self, TransferOffer, TransferOfferResponse, TransferState},
};

#[derive(Debug, Clone, Serialize)]
struct HelloResponse {
    device_id: String,
    alias: String,
    app_version: String,
    protocol_version: u16,
    os: String,
    arch: String,
    ipv4: Option<String>,
    ipv6: Option<String>,
}

#[derive(Clone)]
struct ServerState {
    app: AppHandle,
    transfer_state: Arc<TransferState>,
}

pub fn start_server(app: AppHandle, transfer_state: Arc<TransferState>) {
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("failed to start tokio runtime: {error}");
                return;
            }
        };

        runtime.block_on(async move {
            if let Err(error) = run_server(app, transfer_state).await {
                eprintln!("failed to start local server: {error}");
            }
        });
    });
}

async fn run_server(app: AppHandle, transfer_state: Arc<TransferState>) -> Result<(), String> {
    let config = config::load_or_create_config()?;
    let state = ServerState {
        app,
        transfer_state,
    };
    let ipv4_listener = bind_listener(SocketAddr::from((Ipv4Addr::UNSPECIFIED, config.listen_port))).await?;
    let ipv6_listener = bind_listener(SocketAddr::from((Ipv6Addr::UNSPECIFIED, config.listen_port))).await?;

    let ipv4_server = axum::serve(ipv4_listener, router(state.clone()));
    let ipv6_server = axum::serve(ipv6_listener, router(state));

    tokio::try_join!(ipv4_server, ipv6_server)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

async fn bind_listener(address: SocketAddr) -> Result<tokio::net::TcpListener, String> {
    tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| format!("端口 {} 监听失败：{error}", address.port()))
}

fn router(state: ServerState) -> Router {
    Router::new()
        .route("/api/v1/hello", get(hello))
        .route("/api/v1/transfer/offer", post(transfer_offer))
        .route(
            "/api/v1/transfer/:transfer_id/upload/:file_index",
            post(upload_file),
        )
        .with_state(state)
}

async fn hello() -> Result<Json<HelloResponse>, String> {
    let config = config::load_or_create_config()?;
    let addresses = network::get_local_addresses()?;

    Ok(Json(HelloResponse {
        device_id: config.device_id.to_string(),
        alias: config.alias,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_version: config.protocol_version,
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        ipv4: addresses.ipv4,
        ipv6: addresses.ipv6,
    }))
}

async fn transfer_offer(
    State(state): State<ServerState>,
    Json(offer): Json<TransferOffer>,
) -> Json<TransferOfferResponse> {
    Json(transfer::handle_offer(state.app, state.transfer_state, offer).await)
}

async fn upload_file(
    State(state): State<ServerState>,
    Path((transfer_id, file_index)): Path<(uuid::Uuid, u32)>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<transfer::UploadResult>, String> {
    transfer::upload_file(state.transfer_state, transfer_id, file_index, headers, body)
        .await
        .map(Json)
}
