use axum::{body::Body, http::HeaderMap};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
use tauri::{AppHandle, Emitter};
use tokio::{io::AsyncWriteExt, sync::oneshot};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::config;

pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardTextPayload {
    pub text: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferFileMeta {
    pub index: u32,
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferOffer {
    pub transfer_id: Uuid,
    pub sender_id: String,
    pub sender_alias: String,
    pub files: Vec<TransferFileMeta>,
    pub clipboard_text: Option<ClipboardTextPayload>,
    pub total_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferDecision {
    pub transfer_id: Uuid,
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferOfferResponse {
    pub transfer_id: Option<Uuid>,
    pub accepted: bool,
    pub upload_token: Option<String>,
    pub expires_in: Option<u64>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiveTransferStatusKind {
    Receiving,
    Complete,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReceiveTransferStatusEvent {
    pub transfer_id: Uuid,
    pub status: ReceiveTransferStatusKind,
    pub file_name: String,
    pub saved_name: Option<String>,
    pub file_index: u32,
    pub file_count: u32,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadResult {
    pub ok: bool,
    pub saved_name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendFileRequest {
    pub path: String,
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectedFileInfo {
    pub path: String,
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone)]
struct AcceptedTransfer {
    token: String,
    files: Vec<TransferFileMeta>,
}

#[derive(Default)]
pub struct TransferState {
    pending: Mutex<HashMap<Uuid, oneshot::Sender<bool>>>,
    accepted: Mutex<HashMap<Uuid, AcceptedTransfer>>,
}

pub async fn handle_offer(
    app: AppHandle,
    state: std::sync::Arc<TransferState>,
    offer: TransferOffer,
) -> TransferOfferResponse {
    if offer.files.is_empty() && offer.clipboard_text.is_none() {
        return TransferOfferResponse::rejected("empty_transfer");
    }

    if let Some(clipboard_text) = &offer.clipboard_text {
        if let Err(reason) = validate_clipboard_text(clipboard_text) {
            return TransferOfferResponse::rejected(&reason);
        }
    }

    let transfer_id = offer.transfer_id;
    let (sender, receiver) = oneshot::channel();

    {
        let mut pending = match state.pending.lock() {
            Ok(pending) => pending,
            Err(_) => return TransferOfferResponse::rejected("state_unavailable"),
        };
        pending.insert(transfer_id, sender);
    }

    if app.emit("transfer-offer", &offer).is_err() {
        remove_pending(&state, transfer_id);
        return TransferOfferResponse::rejected("ui_unavailable");
    }

    let decision = tokio::time::timeout(Duration::from_secs(120), receiver).await;
    remove_pending(&state, transfer_id);

    match decision {
        Ok(Ok(true)) => {
            let token = Uuid::new_v4().to_string();
            if let Ok(mut accepted) = state.accepted.lock() {
                accepted.insert(
                    transfer_id,
                    AcceptedTransfer {
                        token: token.clone(),
                        files: offer.files,
                    },
                );
            }
            TransferOfferResponse {
                transfer_id: Some(transfer_id),
                accepted: true,
                upload_token: Some(token),
                expires_in: Some(120),
                reason: None,
            }
        }
        Ok(Ok(false)) => TransferOfferResponse::rejected("user_rejected"),
        Ok(Err(_)) => TransferOfferResponse::rejected("decision_cancelled"),
        Err(_) => TransferOfferResponse::rejected("timeout"),
    }
}

pub async fn upload_file(
    app: AppHandle,
    state: std::sync::Arc<TransferState>,
    transfer_id: Uuid,
    file_index: u32,
    headers: HeaderMap,
    body: Body,
) -> Result<UploadResult, String> {
    let token = parse_bearer_token(&headers)?;
    let session = {
        let accepted = state
            .accepted
            .lock()
            .map_err(|_| "传输状态不可用".to_string())?;
        accepted
            .get(&transfer_id)
            .cloned()
            .ok_or_else(|| "传输请求不存在或已过期".to_string())?
    };

    if session.token != token {
        return Err("上传 token 无效".to_string());
    }

    let file_count = session.files.len() as u32;
    let expected_file = session
        .files
        .iter()
        .find(|file| file.index == file_index)
        .ok_or_else(|| "文件 index 无效".to_string())?;
    let _ = app.emit(
        "receive-transfer-status",
        receive_status_event(
            transfer_id,
            ReceiveTransferStatusKind::Receiving,
            expected_file,
            file_count,
            None,
        ),
    );
    let saved_name = sanitized_file_name(&expected_file.name);
    let save_path = unique_save_path(&config::load_or_create_config()?.save_dir, &saved_name)?;
    let mut file = tokio::fs::File::create(&save_path)
        .await
        .map_err(|error| error.to_string())?;
    let mut body = body.into_data_stream();
    let mut written = 0u64;

    while let Some(chunk) = body.next().await {
        let data = chunk.map_err(|error| error.to_string())?;
        written += data.len() as u64;
        if written > expected_file.size {
            return Err("上传大小超过预期".to_string());
        }
        file.write_all(&data)
            .await
            .map_err(|error| error.to_string())?;
    }

    file.flush().await.map_err(|error| error.to_string())?;

    if written != expected_file.size {
        return Err("上传大小与预期不一致".to_string());
    }

    let saved_name = save_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&saved_name)
        .to_string();
    let _ = app.emit(
        "receive-transfer-status",
        receive_status_event(
            transfer_id,
            ReceiveTransferStatusKind::Complete,
            expected_file,
            file_count,
            Some(saved_name.clone()),
        ),
    );

    Ok(UploadResult {
        ok: true,
        saved_name,
        size: written,
    })
}

pub fn validate_clipboard_text(payload: &ClipboardTextPayload) -> Result<(), String> {
    let actual_size = payload.text.as_bytes().len();
    if payload.text.trim().is_empty() {
        return Err("clipboard_empty".to_string());
    }

    if actual_size > MAX_CLIPBOARD_TEXT_BYTES {
        return Err("clipboard_too_large".to_string());
    }

    if payload.size != actual_size as u64 {
        return Err("clipboard_size_mismatch".to_string());
    }

    Ok(())
}

pub fn inspect_files(paths: Vec<String>) -> Result<Vec<SelectedFileInfo>, String> {
    paths
        .into_iter()
        .map(|path| {
            let metadata =
                fs::metadata(&path).map_err(|error| format!("读取文件信息失败 {path}：{error}"))?;
            if !metadata.is_file() {
                return Err(format!("暂不支持文件夹：{path}"));
            }

            let name = Path::new(&path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file")
                .to_string();

            Ok(SelectedFileInfo {
                path,
                name,
                size: metadata.len(),
            })
        })
        .collect()
}

pub async fn send_files(
    host: String,
    port: u16,
    address_type: crate::peer::AddressType,
    transfer_id: Uuid,
    token: String,
    files: Vec<SendFileRequest>,
) -> Result<Vec<UploadResult>, String> {
    let client = reqwest::Client::new();
    let base_url = match address_type {
        crate::peer::AddressType::Ipv4 => format!("http://{}:{}", host, port),
        crate::peer::AddressType::Ipv6 => {
            format!("http://[{}]:{}", host.trim_matches(&['[', ']'][..]), port)
        }
    };
    let mut results = Vec::with_capacity(files.len());

    for (index, file) in files.into_iter().enumerate() {
        let source = tokio::fs::File::open(&file.path)
            .await
            .map_err(|error| format!("读取文件失败 {}：{error}", file.name))?;
        let body = reqwest::Body::wrap_stream(ReaderStream::new(source));
        let response = client
            .post(format!(
                "{base_url}/api/v1/transfer/{transfer_id}/upload/{index}"
            ))
            .bearer_auth(&token)
            .body(body)
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?
            .json::<UploadResult>()
            .await
            .map_err(|error| error.to_string())?;
        results.push(response);
    }

    Ok(results)
}

pub fn decide_transfer(state: &TransferState, decision: TransferDecision) -> Result<(), String> {
    let sender = {
        let mut pending = state
            .pending
            .lock()
            .map_err(|_| "传输状态不可用".to_string())?;
        pending.remove(&decision.transfer_id)
    };

    match sender {
        Some(sender) => sender
            .send(decision.accepted)
            .map_err(|_| "传输请求已过期".to_string()),
        None => Err("传输请求不存在或已过期".to_string()),
    }
}

fn receive_status_event(
    transfer_id: Uuid,
    status: ReceiveTransferStatusKind,
    file: &TransferFileMeta,
    file_count: u32,
    saved_name: Option<String>,
) -> ReceiveTransferStatusEvent {
    ReceiveTransferStatusEvent {
        transfer_id,
        status,
        file_name: file.name.clone(),
        saved_name,
        file_index: file.index + 1,
        file_count,
        size: file.size,
    }
}

fn parse_bearer_token(headers: &HeaderMap) -> Result<String, String> {
    let value = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| "缺少 Authorization header".to_string())?;

    value
        .strip_prefix("Bearer ")
        .map(|token| token.to_string())
        .ok_or_else(|| "Authorization header 格式无效".to_string())
}

fn sanitized_file_name(name: &str) -> String {
    let file_name = Path::new(name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("received-file");
    let sanitized: String = file_name
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            _ => character,
        })
        .collect();

    if sanitized.trim().is_empty() {
        "received-file".to_string()
    } else {
        sanitized
    }
}

fn unique_save_path(save_dir: &str, file_name: &str) -> Result<PathBuf, String> {
    let directory = PathBuf::from(save_dir);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;

    let path = directory.join(file_name);
    if !path.exists() {
        return Ok(path);
    }

    let original = Path::new(file_name);
    let stem = original
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("received-file");
    let extension = original
        .extension()
        .and_then(|extension| extension.to_str());

    for index in 1..1000 {
        let candidate_name = match extension {
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None => format!("{stem} ({index})"),
        };
        let candidate = directory.join(candidate_name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err("无法生成不重复的保存文件名".to_string())
}

fn remove_pending(state: &TransferState, transfer_id: Uuid) {
    if let Ok(mut pending) = state.pending.lock() {
        pending.remove(&transfer_id);
    }
}

impl TransferOfferResponse {
    fn rejected(reason: &str) -> Self {
        Self {
            transfer_id: None,
            accepted: false,
            upload_token: None,
            expires_in: None,
            reason: Some(reason.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receive_status_event_uses_one_based_file_index() {
        let file = TransferFileMeta {
            index: 0,
            name: "report.pdf".to_string(),
            size: 42,
        };

        let event = receive_status_event(
            Uuid::nil(),
            ReceiveTransferStatusKind::Receiving,
            &file,
            3,
            None,
        );

        assert_eq!(event.transfer_id, Uuid::nil());
        assert_eq!(event.status, ReceiveTransferStatusKind::Receiving);
        assert_eq!(event.file_name, "report.pdf");
        assert_eq!(event.saved_name, None);
        assert_eq!(event.file_index, 1);
        assert_eq!(event.file_count, 3);
        assert_eq!(event.size, 42);
    }
}
