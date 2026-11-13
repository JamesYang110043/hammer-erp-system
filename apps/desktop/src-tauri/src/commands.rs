//! 前端透過 `invoke` 呼叫的 Tauri commands。
//!
//! 前端不直接呼叫 HTTP API，一律經過這裡（`docs/architecture.md` §3.3）。
//! 新增 command 後要加進 [`crate::specta_builder`]，並執行 `pnpm gen:bindings`。

use hammer_shared::Id;
use hammer_shared::audit::{AuditLogEntry, AuditLogQuery};
use hammer_shared::auth::{ChangePasswordRequest, CurrentUser, LoginRequest};
use hammer_shared::health::HealthResponse;
use hammer_shared::pagination::Page;
use hammer_shared::setup::{CreateAdminRequest, SetupStatus};
use hammer_shared::user::{
    CreateUserRequest, DeleteUserQuery, ResetPasswordRequest, Role, UpdateUserRequest, User,
};
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

/// 伺服器是否還沒有任何帳號（需要先建立管理者）。
#[tauri::command]
#[specta::specta]
pub async fn setup_status(state: State<'_, AppState>) -> Result<SetupStatus, ClientError> {
    state.setup_status().await
}

/// 建立第一個管理者，成功後直接登入（token 存進鑰匙圈）。
#[tauri::command]
#[specta::specta]
pub async fn setup_admin(
    state: State<'_, AppState>,
    req: CreateAdminRequest,
) -> Result<CurrentUser, ClientError> {
    state.setup_admin(&req).await
}

/// 登入，token 存進鑰匙圈；token 不會回傳給前端。
#[tauri::command]
#[specta::specta]
pub async fn login(
    state: State<'_, AppState>,
    req: LoginRequest,
) -> Result<CurrentUser, ClientError> {
    state.login(&req).await
}

/// 登出並清除本機 token。伺服器連不上時仍會清除本機 token。
#[tauri::command]
#[specta::specta]
pub async fn logout(state: State<'_, AppState>) -> Result<(), ClientError> {
    state.logout().await
}

/// 目前登入的使用者；本機沒有 token 時回傳 `null`。
///
/// token 已失效時回傳 `sessionExpired` 錯誤，並清除本機 token。
#[tauri::command]
#[specta::specta]
pub async fn current_user(state: State<'_, AppState>) -> Result<Option<CurrentUser>, ClientError> {
    state.current_user().await
}

/// 修改自己的密碼。
#[tauri::command]
#[specta::specta]
pub async fn change_password(
    state: State<'_, AppState>,
    req: ChangePasswordRequest,
) -> Result<(), ClientError> {
    state.change_password(&req).await
}

/// 角色列表（需要 `user.manage`）。
#[tauri::command]
#[specta::specta]
pub async fn list_roles(state: State<'_, AppState>) -> Result<Vec<Role>, ClientError> {
    state.list_roles().await
}

/// 帳號列表（需要 `user.manage`）。
#[tauri::command]
#[specta::specta]
pub async fn list_users(state: State<'_, AppState>) -> Result<Vec<User>, ClientError> {
    state.list_users().await
}

/// 新增帳號（需要 `user.manage`）。建立後第一次登入必須修改密碼。
#[tauri::command]
#[specta::specta]
pub async fn create_user(
    state: State<'_, AppState>,
    req: CreateUserRequest,
) -> Result<User, ClientError> {
    state.create_user(&req).await
}

/// 修改帳號的顯示名稱、角色、停用狀態（需要 `user.manage`）。
///
/// `req.version` 不符時回傳 `409 CONFLICT`。
#[tauri::command]
#[specta::specta]
pub async fn update_user(
    state: State<'_, AppState>,
    id: Id,
    req: UpdateUserRequest,
) -> Result<User, ClientError> {
    state.update_user(id, &req).await
}

/// 管理者重設密碼為臨時密碼（需要 `user.manage`）。
#[tauri::command]
#[specta::specta]
pub async fn reset_user_password(
    state: State<'_, AppState>,
    id: Id,
    req: ResetPasswordRequest,
) -> Result<User, ClientError> {
    state.reset_user_password(id, &req).await
}

/// 解除登入失敗鎖定（需要 `user.manage`）。
#[tauri::command]
#[specta::specta]
pub async fn unlock_user(state: State<'_, AppState>, id: Id) -> Result<User, ClientError> {
    state.unlock_user(id).await
}

/// 刪除帳號（需要 `user.manage`）。`version` 不符時回傳 `409 CONFLICT`。
#[tauri::command]
#[specta::specta]
pub async fn delete_user(
    state: State<'_, AppState>,
    id: Id,
    version: i32,
) -> Result<(), ClientError> {
    state.delete_user(id, &DeleteUserQuery { version }).await
}

/// 查詢操作紀錄（需要 `audit.read`）。
#[tauri::command]
#[specta::specta]
pub async fn list_audit_logs(
    state: State<'_, AppState>,
    query: AuditLogQuery,
) -> Result<Page<AuditLogEntry>, ClientError> {
    state.list_audit_logs(&query).await
}
