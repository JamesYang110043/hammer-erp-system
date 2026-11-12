//! 前端透過 `invoke` 呼叫的 Tauri commands。
//!
//! 前端不直接呼叫 HTTP API，一律經過這裡（`docs/architecture.md` §3.3）。
//! 新增 command 後要加進 [`crate::specta_builder`]，並執行 `pnpm gen:bindings`。

use hammer_shared::health::HealthResponse;
use tauri::State;

use crate::error::ClientError;
use crate::settings::normalize_server_url;
use crate::state::AppState;

/// 呼叫伺服器的 `GET /api/v1/health`。
///
/// `server_url` 有值時改測該網址（設定頁「測試連線」用，不會儲存）；
/// 沒有值時使用已儲存的網址。
#[tauri::command]
#[specta::specta]
pub async fn health_check(
    state: State<'_, AppState>,
    server_url: Option<String>,
) -> Result<HealthResponse, ClientError> {
    let url = match server_url {
        Some(url) => normalize_server_url(&url)?,
        None => state.server_url(),
    };
    state.client.health(&url).await
}

/// 目前儲存的伺服器網址。
#[tauri::command]
#[specta::specta]
pub fn get_server_url(state: State<'_, AppState>) -> String {
    state.server_url()
}

/// 檢查並儲存伺服器網址，回傳正規化後的網址。
#[tauri::command]
#[specta::specta]
pub fn set_server_url(
    state: State<'_, AppState>,
    server_url: String,
) -> Result<String, ClientError> {
    let url = normalize_server_url(&server_url)?;
    state.set_server_url(url.clone())?;
    Ok(url)
}
