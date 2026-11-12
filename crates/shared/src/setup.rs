//! 初次啟動建立第一個管理者的 request / response（docs/architecture.md §11）。
//!
//! | 端點 | request | response |
//! |---|---|---|
//! | `GET /api/v1/setup/status` | — | [`SetupStatus`] |
//! | `POST /api/v1/setup/admin` | [`CreateAdminRequest`] | [`LoginResponse`](crate::auth::LoginResponse) |
//!
//! 兩個端點都不需要登入。`users` 已有資料時，`POST /setup/admin` 回傳 `409 CONFLICT`。

use serde::{Deserialize, Serialize};
use specta::Type;

/// `GET /api/v1/setup/status` 的回應。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SetupStatus {
    /// 資料庫還沒有任何帳號，需要先建立管理者。
    pub needs_setup: bool,
}

/// `POST /api/v1/setup/admin` 的輸入。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CreateAdminRequest {
    /// 登入帳號（前後空白會去除）。
    pub username: String,
    /// 顯示名稱（前後空白會去除）。
    pub display_name: String,
    /// 至少 10 個字元（§6.2）。
    pub password: String,
}

impl std::fmt::Debug for CreateAdminRequest {
    /// 不印出密碼，避免寫進 log。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateAdminRequest")
            .field("username", &self.username)
            .field("display_name", &self.display_name)
            .field("password", &"***")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_json_shape() {
        assert_eq!(
            serde_json::to_value(SetupStatus { needs_setup: true }).unwrap(),
            json!({ "needs_setup": true })
        );
    }

    #[test]
    fn request_deserializes_and_hides_password() {
        let req: CreateAdminRequest = serde_json::from_value(json!({
            "username": "admin",
            "display_name": "管理者",
            "password": "secret-password",
        }))
        .unwrap();
        assert_eq!(req.username, "admin");
        assert_eq!(req.display_name, "管理者");
        let debug = format!("{req:?}");
        assert!(debug.contains("admin"));
        assert!(!debug.contains("secret-password"));
    }
}
