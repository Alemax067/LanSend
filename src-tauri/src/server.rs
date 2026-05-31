use axum::{
    body::Body,
    extract::{Path, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
};
use tauri::AppHandle;
use tokio::net::TcpListener;

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
    let ipv4_listener = bind_ipv4_listener(config.listen_port).await;
    let ipv6_listener = bind_ipv6_listener(config.listen_port).await;

    match (ipv4_listener, ipv6_listener) {
        (Ok(ipv4_listener), Ok(ipv6_listener)) => {
            tokio::try_join!(
                serve_listener(ipv4_listener, state.clone()),
                serve_listener(ipv6_listener, state),
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
        }
        (Ok(listener), Err(error)) | (Err(error), Ok(listener)) => {
            eprintln!("failed to start one network listener: {error}");
            serve_listener(listener, state)
                .await
                .map_err(|error| error.to_string())
        }
        (Err(ipv4_error), Err(ipv6_error)) => Err(format!(
            "端口 {} 监听失败：IPv4: {ipv4_error}; IPv6: {ipv6_error}",
            config.listen_port
        )),
    }
}

async fn bind_ipv4_listener(port: u16) -> Result<TcpListener, String> {
    bind_listener(
        Domain::IPV4,
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
        false,
    )
    .map_err(|error| format!("IPv4 端口 {port} 监听失败：{error}"))
}

async fn bind_ipv6_listener(port: u16) -> Result<TcpListener, String> {
    bind_listener(
        Domain::IPV6,
        SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
        true,
    )
    .map_err(|error| format!("IPv6 端口 {port} 监听失败：{error}"))
}

fn bind_listener(domain: Domain, address: SocketAddr, only_v6: bool) -> Result<TcpListener, String> {
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))
        .map_err(|error| error.to_string())?;
    socket
        .set_reuse_address(true)
        .map_err(|error| error.to_string())?;
    if domain == Domain::IPV6 {
        socket
            .set_only_v6(only_v6)
            .map_err(|error| error.to_string())?;
    }
    socket
        .bind(&address.into())
        .map_err(|error| error.to_string())?;
    socket.listen(1024).map_err(|error| error.to_string())?;
    socket
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    TcpListener::from_std(socket.into()).map_err(|error| error.to_string())
}

async fn serve_listener(listener: TcpListener, state: ServerState) -> std::io::Result<()> {
    axum::serve(listener, router(state)).await
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
    transfer::upload_file(state.app, state.transfer_state, transfer_id, file_index, headers, body)
        .await
        .map(Json)
}
