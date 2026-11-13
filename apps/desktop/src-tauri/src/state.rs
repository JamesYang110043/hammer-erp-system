//! Tauri 管理的全域狀態。

use std::path::PathBuf;
use std::sync::{PoisonError, RwLock};

use hammer_shared::Id;
use hammer_shared::audit::{AuditLogEntry, AuditLogQuery};
use hammer_shared::auth::{ChangePasswordRequest, CurrentUser, LoginRequest, LoginResponse};
use hammer_shared::layout::{
    CreateViewLayoutRequest, DefaultViewLayout, DeleteViewLayoutQuery, UpdateViewLayoutRequest,
    ViewLayout,
};
use hammer_shared::pagination::Page;
use hammer_shared::record::{
    CreateRecordRequest, DeleteRecordQuery, Record, RecordListQuery, UpdateRecordRequest,
};
use hammer_shared::schema::{
    ChangeFieldTypeRequest, CreateEntityRequest, CreateFieldRequest, DeactivateFieldRequest,
    EntityDefinition, EntityDetail, FieldDefinition, ReorderFieldsRequest, UpdateFieldRequest,
};
use hammer_shared::setup::{CreateAdminRequest, SetupStatus};
use hammer_shared::user::{
    CreateUserRequest, DeleteUserQuery, ResetPasswordRequest, Role, UpdateUserRequest, User,
};

use crate::api_client::ApiClient;
use crate::error::ClientError;
use crate::settings::Settings;
use crate::token_store::TokenStore;

pub struct AppState {
    pub client: ApiClient,
    settings_path: PathBuf,
    settings: RwLock<Settings>,
    tokens: Box<dyn TokenStore>,
}

impl AppState {
    pub fn new(client: ApiClient, settings_path: PathBuf, tokens: Box<dyn TokenStore>) -> Self {
        let settings = Settings::load(&settings_path);
        Self {
            client,
            settings_path,
            settings: RwLock::new(settings),
            tokens,
        }
    }

    pub fn server_url(&self) -> String {
        self.settings
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .server_url
            .clone()
    }

    /// 更新伺服器網址並寫入設定檔（`server_url` 須已正規化）。
    ///
    /// token 以伺服器網址區分，切換網址後使用該網址自己的登入狀態。
    pub fn set_server_url(&self, server_url: String) -> Result<(), ClientError> {
        let mut settings = self
            .settings
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let mut next = settings.clone();
        next.server_url = server_url;
        next.save(&self.settings_path)
            .map_err(|err| ClientError::storage(format!("儲存設定失敗：{err}")))?;
        *settings = next;
        Ok(())
    }

    /// 伺服器是否還沒有任何帳號。
    pub async fn setup_status(&self) -> Result<SetupStatus, ClientError> {
        self.client.setup_status(&self.server_url()).await
    }

    /// 建立第一個管理者，成功後直接登入。
    pub async fn setup_admin(&self, req: &CreateAdminRequest) -> Result<CurrentUser, ClientError> {
        let url = self.server_url();
        let resp = self.client.setup_admin(&url, req).await?;
        self.save_session(&url, resp)
    }

    /// 登入，token 存進鑰匙圈。
    pub async fn login(&self, req: &LoginRequest) -> Result<CurrentUser, ClientError> {
        let url = self.server_url();
        let resp = self.client.login(&url, req).await?;
        self.save_session(&url, resp)
    }

    /// 登出：通知伺服器撤銷 session，並清除本機 token。
    ///
    /// 伺服器連不上或 session 已失效時，仍清除本機 token（伺服器端的 session 會閒置逾時）。
    pub async fn logout(&self) -> Result<(), ClientError> {
        let url = self.server_url();
        let Some(token) = self.tokens.get(&url)? else {
            return Ok(());
        };
        if let Err(err) = self.client.logout(&url, &token).await {
            eprintln!("登出時通知伺服器失敗，仍清除本機登入狀態：{err}");
        }
        self.tokens.delete(&url)
    }

    /// 目前登入的使用者。本機沒有 token 時回傳 `None`。
    pub async fn current_user(&self) -> Result<Option<CurrentUser>, ClientError> {
        let url = self.server_url();
        if self.tokens.get(&url)?.is_none() {
            return Ok(None);
        }
        self.with_token(async |client, url, token| client.me(url, token).await)
            .await
            .map(Some)
    }

    /// 修改自己的密碼。
    pub async fn change_password(&self, req: &ChangePasswordRequest) -> Result<(), ClientError> {
        self.with_token(async |client, url, token| client.change_password(url, token, req).await)
            .await
    }

    /// 角色列表（帳號管理）。
    pub async fn list_roles(&self) -> Result<Vec<Role>, ClientError> {
        self.with_token(async |client, url, token| client.list_roles(url, token).await)
            .await
    }

    /// 帳號列表。
    pub async fn list_users(&self) -> Result<Vec<User>, ClientError> {
        self.with_token(async |client, url, token| client.list_users(url, token).await)
            .await
    }

    /// 新增帳號。
    pub async fn create_user(&self, req: &CreateUserRequest) -> Result<User, ClientError> {
        self.with_token(async |client, url, token| client.create_user(url, token, req).await)
            .await
    }

    /// 修改帳號（顯示名稱、角色、停用）。
    pub async fn update_user(&self, id: Id, req: &UpdateUserRequest) -> Result<User, ClientError> {
        self.with_token(async |client, url, token| client.update_user(url, token, id, req).await)
            .await
    }

    /// 管理者重設密碼。
    pub async fn reset_user_password(
        &self,
        id: Id,
        req: &ResetPasswordRequest,
    ) -> Result<User, ClientError> {
        self.with_token(async |client, url, token| {
            client.reset_user_password(url, token, id, req).await
        })
        .await
    }

    /// 解除鎖定。
    pub async fn unlock_user(&self, id: Id) -> Result<User, ClientError> {
        self.with_token(async |client, url, token| client.unlock_user(url, token, id).await)
            .await
    }

    /// 刪除帳號。
    pub async fn delete_user(&self, id: Id, query: &DeleteUserQuery) -> Result<(), ClientError> {
        self.with_token(async |client, url, token| client.delete_user(url, token, id, query).await)
            .await
    }

    /// 操作紀錄查詢。
    pub async fn list_audit_logs(
        &self,
        query: &AuditLogQuery,
    ) -> Result<Page<AuditLogEntry>, ClientError> {
        self.with_token(async |client, url, token| client.list_audit_logs(url, token, query).await)
            .await
    }

    /// 資料類別列表。
    pub async fn list_entities(&self) -> Result<Vec<EntityDefinition>, ClientError> {
        self.with_token(async |client, url, token| client.list_entities(url, token).await)
            .await
    }

    /// 資料類別與其所有欄位定義。
    pub async fn get_entity(&self, key: &str) -> Result<EntityDetail, ClientError> {
        self.with_token(async |client, url, token| client.get_entity(url, token, key).await)
            .await
    }

    /// 新增資料類別。
    pub async fn create_entity(
        &self,
        req: &CreateEntityRequest,
    ) -> Result<EntityDetail, ClientError> {
        self.with_token(async |client, url, token| client.create_entity(url, token, req).await)
            .await
    }

    /// 新增欄位。
    pub async fn create_field(
        &self,
        key: &str,
        req: &CreateFieldRequest,
    ) -> Result<FieldDefinition, ClientError> {
        self.with_token(async |client, url, token| client.create_field(url, token, key, req).await)
            .await
    }

    /// 修改欄位。
    pub async fn update_field(
        &self,
        key: &str,
        field_key: &str,
        req: &UpdateFieldRequest,
    ) -> Result<FieldDefinition, ClientError> {
        self.with_token(async |client, url, token| {
            client.update_field(url, token, key, field_key, req).await
        })
        .await
    }

    /// 停用欄位（資料保留）。
    pub async fn deactivate_field(
        &self,
        key: &str,
        field_key: &str,
        req: &DeactivateFieldRequest,
    ) -> Result<FieldDefinition, ClientError> {
        self.with_token(async |client, url, token| {
            client
                .deactivate_field(url, token, key, field_key, req)
                .await
        })
        .await
    }

    /// 調整欄位排序。
    pub async fn reorder_fields(
        &self,
        key: &str,
        req: &ReorderFieldsRequest,
    ) -> Result<Vec<FieldDefinition>, ClientError> {
        self.with_token(async |client, url, token| {
            client.reorder_fields(url, token, key, req).await
        })
        .await
    }

    /// 變更欄位資料型態。
    pub async fn change_field_type(
        &self,
        key: &str,
        field_key: &str,
        req: &ChangeFieldTypeRequest,
    ) -> Result<FieldDefinition, ClientError> {
        self.with_token(async |client, url, token| {
            client
                .change_field_type(url, token, key, field_key, req)
                .await
        })
        .await
    }

    /// 資料列表。
    pub async fn list_records(
        &self,
        key: &str,
        query: &RecordListQuery,
    ) -> Result<Page<Record>, ClientError> {
        self.with_token(async |client, url, token| {
            client.list_records(url, token, key, query).await
        })
        .await
    }

    /// 取得一筆資料。
    pub async fn get_record(&self, key: &str, id: Id) -> Result<Record, ClientError> {
        self.with_token(async |client, url, token| client.get_record(url, token, key, id).await)
            .await
    }

    /// 新增資料。
    pub async fn create_record(
        &self,
        key: &str,
        req: &CreateRecordRequest,
    ) -> Result<Record, ClientError> {
        self.with_token(async |client, url, token| client.create_record(url, token, key, req).await)
            .await
    }

    /// 修改資料。
    pub async fn update_record(
        &self,
        key: &str,
        id: Id,
        req: &UpdateRecordRequest,
    ) -> Result<Record, ClientError> {
        self.with_token(async |client, url, token| {
            client.update_record(url, token, key, id, req).await
        })
        .await
    }

    /// 刪除資料。
    pub async fn delete_record(
        &self,
        key: &str,
        id: Id,
        query: &DeleteRecordQuery,
    ) -> Result<(), ClientError> {
        self.with_token(async |client, url, token| {
            client.delete_record(url, token, key, id, query).await
        })
        .await
    }

    /// 列表版面（全域版面與自己的版面）。
    pub async fn list_layouts(&self, key: &str) -> Result<Vec<ViewLayout>, ClientError> {
        self.with_token(async |client, url, token| client.list_layouts(url, token, key).await)
            .await
    }

    /// 要套用的預設版面。
    pub async fn get_default_layout(&self, key: &str) -> Result<DefaultViewLayout, ClientError> {
        self.with_token(async |client, url, token| client.get_default_layout(url, token, key).await)
            .await
    }

    /// 新增版面。
    pub async fn create_layout(
        &self,
        key: &str,
        req: &CreateViewLayoutRequest,
    ) -> Result<ViewLayout, ClientError> {
        self.with_token(async |client, url, token| client.create_layout(url, token, key, req).await)
            .await
    }

    /// 修改版面。
    pub async fn update_layout(
        &self,
        key: &str,
        id: Id,
        req: &UpdateViewLayoutRequest,
    ) -> Result<ViewLayout, ClientError> {
        self.with_token(async |client, url, token| {
            client.update_layout(url, token, key, id, req).await
        })
        .await
    }

    /// 刪除版面。
    pub async fn delete_layout(
        &self,
        key: &str,
        id: Id,
        query: &DeleteViewLayoutQuery,
    ) -> Result<(), ClientError> {
        self.with_token(async |client, url, token| {
            client.delete_layout(url, token, key, id, query).await
        })
        .await
    }

    /// 以目前的 token 呼叫需要登入的 API。
    ///
    /// 沒有 token 或伺服器回 `401` 時回傳 [`ClientError::SessionExpired`]，並清除本機 token。
    async fn with_token<T>(
        &self,
        call: impl AsyncFnOnce(&ApiClient, &str, &str) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let url = self.server_url();
        let Some(token) = self.tokens.get(&url)? else {
            return Err(ClientError::session_expired());
        };
        let result = call(&self.client, &url, &token).await;
        if matches!(result, Err(ClientError::SessionExpired { .. })) {
            self.tokens.delete(&url)?;
        }
        result
    }

    fn save_session(&self, url: &str, resp: LoginResponse) -> Result<CurrentUser, ClientError> {
        self.tokens.set(url, &resp.token)?;
        Ok(resp.user)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::Router;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use hammer_shared::constants::API_PREFIX;
    use serde_json::json;

    use super::*;
    use crate::token_store::MemoryTokenStore;

    /// 共用一個 token store，讓測試在 `AppState` 之外也能檢查內容。
    struct SharedStore(Arc<MemoryTokenStore>);

    impl TokenStore for SharedStore {
        fn get(&self, url: &str) -> Result<Option<String>, ClientError> {
            self.0.get(url)
        }
        fn set(&self, url: &str, token: &str) -> Result<(), ClientError> {
            self.0.set(url, token)
        }
        fn delete(&self, url: &str) -> Result<(), ClientError> {
            self.0.delete(url)
        }
    }

    fn user_json() -> serde_json::Value {
        json!({
            "id": "0196f5a0-0000-7000-8000-000000000001",
            "username": "admin",
            "display_name": "管理者",
            "role": "admin",
            "permissions": ["user.manage"],
            "must_change_password": false,
        })
    }

    /// 測試用伺服器：登入發 `tok`；`/auth/me` 只接受 `tok`；`expired` 為 true 時一律回 401。
    async fn serve(expired: Arc<AtomicBool>) -> String {
        let api = Router::new()
            .route(
                "/auth/login",
                post(|| async { axum::Json(json!({ "token": "tok", "user": user_json() })) }),
            )
            .route(
                "/auth/me",
                get(move |headers: HeaderMap| {
                    let expired = expired.load(Ordering::SeqCst);
                    async move {
                        let ok = headers.get("authorization").and_then(|v| v.to_str().ok())
                            == Some("Bearer tok");
                        if ok && !expired {
                            Ok(axum::Json(user_json()))
                        } else {
                            Err((
                                StatusCode::UNAUTHORIZED,
                                axum::Json(
                                    json!({ "code": "UNAUTHORIZED", "message": "請先登入" }),
                                ),
                            ))
                        }
                    }
                }),
            )
            .route("/auth/logout", post(|| async { StatusCode::NO_CONTENT }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().nest(API_PREFIX, api);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn state(url: &str, store: &Arc<MemoryTokenStore>, name: &str) -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "hammer-desktop-state-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let state = AppState::new(
            ApiClient::new().unwrap(),
            dir.join("settings.json"),
            Box::new(SharedStore(Arc::clone(store))),
        );
        state.set_server_url(url.to_owned()).unwrap();
        state
    }

    fn login_req() -> LoginRequest {
        LoginRequest {
            username: "admin".into(),
            password: "pw".into(),
        }
    }

    #[tokio::test]
    async fn login_stores_token_and_survives_restart() {
        let url = serve(Arc::default()).await;
        let store = Arc::new(MemoryTokenStore::default());

        let app = state(&url, &store, "restart");
        assert_eq!(app.current_user().await.unwrap(), None);
        let user = app.login(&login_req()).await.unwrap();
        assert_eq!(user.username, "admin");
        assert_eq!(store.get(&url).unwrap().as_deref(), Some("tok"));

        // 重新啟動：新的 AppState、同一個鑰匙圈
        let app = state(&url, &store, "restart");
        let user = app.current_user().await.unwrap().unwrap();
        assert_eq!(user.display_name, "管理者");
    }

    #[tokio::test]
    async fn expired_session_clears_token() {
        let expired = Arc::new(AtomicBool::new(false));
        let url = serve(Arc::clone(&expired)).await;
        let store = Arc::new(MemoryTokenStore::default());
        let app = state(&url, &store, "expired");
        app.login(&login_req()).await.unwrap();

        expired.store(true, Ordering::SeqCst);
        assert_eq!(
            app.current_user().await.unwrap_err(),
            ClientError::session_expired()
        );
        assert_eq!(store.get(&url).unwrap(), None);
        // token 已清除：之後視為未登入
        assert_eq!(app.current_user().await.unwrap(), None);
        let req = ChangePasswordRequest {
            current_password: "a".into(),
            new_password: "b".into(),
        };
        assert_eq!(
            app.change_password(&req).await.unwrap_err(),
            ClientError::session_expired()
        );
    }

    #[tokio::test]
    async fn logout_clears_token_even_if_server_unreachable() {
        let url = serve(Arc::default()).await;
        let store = Arc::new(MemoryTokenStore::default());
        let app = state(&url, &store, "logout");
        app.login(&login_req()).await.unwrap();
        app.logout().await.unwrap();
        assert_eq!(store.get(&url).unwrap(), None);

        // 伺服器連不上
        let dead = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            format!("http://{}", listener.local_addr().unwrap())
        };
        store.set(&dead, "tok").unwrap();
        app.set_server_url(dead.clone()).unwrap();
        app.logout().await.unwrap();
        assert_eq!(store.get(&dead).unwrap(), None);
    }

    #[tokio::test]
    async fn tokens_are_per_server_url() {
        let url = serve(Arc::default()).await;
        let store = Arc::new(MemoryTokenStore::default());
        let app = state(&url, &store, "per-server");
        app.login(&login_req()).await.unwrap();

        app.set_server_url("http://other.example:8080".to_owned())
            .unwrap();
        assert_eq!(app.current_user().await.unwrap(), None);
        app.set_server_url(url.clone()).unwrap();
        assert!(app.current_user().await.unwrap().is_some());
    }
}
