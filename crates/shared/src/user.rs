//! 帳號管理的 request / response（docs/architecture.md §6.1、§6.2、§8.4 `users`）。
//!
//! 以下端點都需要 `user.manage` 權限：
//!
//! | 端點 | request | response |
//! |---|---|---|
//! | `GET /api/v1/roles` | — | `Role[]` |
//! | `GET /api/v1/users` | — | `User[]`（未刪除，依帳號排序） |
//! | `GET /api/v1/users/{id}` | — | [`User`] |
//! | `POST /api/v1/users` | [`CreateUserRequest`] | `201` [`User`] |
//! | `PATCH /api/v1/users/{id}` | [`UpdateUserRequest`] | [`User`] |
//! | `POST /api/v1/users/{id}/reset-password` | [`ResetPasswordRequest`] | [`User`] |
//! | `POST /api/v1/users/{id}/unlock` | — | [`User`] |
//! | `DELETE /api/v1/users/{id}?version=N` | [`DeleteUserQuery`] | `204 No Content` |
//!
//! 錯誤：
//!
//! - 輸入不符合規則（含帳號重複）→ `422 VALIDATION`，`details.field` 為欄位名稱
//! - 操作後沒有任何有效的管理者 → `422 VALIDATION`，`details.reason` 為 `last_admin`
//! - `version` 不符 → `409 CONFLICT`（§9.4）
//! - 帳號不存在或已刪除 → `404 NOT_FOUND`

use serde::{Deserialize, Serialize};
use specta::Type;
use time::OffsetDateTime;

use crate::Id;

/// 角色與其權限（`GET /api/v1/roles`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Role {
    pub id: Id,
    /// 角色代碼，對應 `role` 模組的常數（`admin` / `editor` / `viewer`）。
    pub code: String,
    /// 顯示名稱，例如「管理者」。
    pub name: String,
    /// 權限代碼，對應 `permission` 模組的常數，依代碼排序。
    pub permissions: Vec<String>,
}

/// 帳號（帳號管理畫面使用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct User {
    pub id: Id,
    pub username: String,
    pub display_name: String,
    /// 角色代碼。
    pub role: String,
    /// 角色顯示名稱。
    pub role_name: String,
    /// `false` 代表已停用，無法登入。
    pub is_active: bool,
    /// 目前是否因連續登入失敗而鎖定。
    pub is_locked: bool,
    /// 鎖定到此時間（RFC 3339）；沒有鎖定時為 `null`。
    #[serde(with = "time::serde::rfc3339::option")]
    #[specta(type = Option<String>)]
    pub locked_until: Option<OffsetDateTime>,
    /// 下次登入必須修改密碼。
    pub must_change_password: bool,
    /// 最後登入時間（RFC 3339）；從未登入時為 `null`。
    #[serde(with = "time::serde::rfc3339::option")]
    #[specta(type = Option<String>)]
    pub last_login_at: Option<OffsetDateTime>,
    /// 建立時間（RFC 3339）。
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub created_at: OffsetDateTime,
    /// 最後修改時間（RFC 3339）。
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub updated_at: OffsetDateTime,
    /// 樂觀鎖定版本號，修改、刪除時帶回（§9.4）。
    pub version: i32,
}

/// `POST /api/v1/users` 的輸入。建立後 `must_change_password = true`。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CreateUserRequest {
    /// 登入帳號（前後空白會去除）。不可與既有帳號（含已刪除）重複。
    pub username: String,
    /// 顯示名稱（前後空白會去除）。
    pub display_name: String,
    /// 角色代碼。
    pub role: String,
    /// 初始密碼，至少 10 個字元（§6.2）。
    pub password: String,
}

impl std::fmt::Debug for CreateUserRequest {
    /// 不印出密碼，避免寫進 log。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateUserRequest")
            .field("username", &self.username)
            .field("display_name", &self.display_name)
            .field("role", &self.role)
            .field("password", &"***")
            .finish()
    }
}

/// `PATCH /api/v1/users/{id}` 的輸入。省略或 `null` 的欄位不修改。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UpdateUserRequest {
    /// 讀取時取得的版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
    #[serde(default)]
    pub display_name: Option<String>,
    /// 角色代碼。
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub is_active: Option<bool>,
}

/// `POST /api/v1/users/{id}/reset-password` 的輸入。
///
/// 設定新的臨時密碼、`must_change_password = true`、解除鎖定，並撤銷該使用者所有 session。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ResetPasswordRequest {
    /// 臨時密碼，至少 10 個字元（§6.2）。
    pub new_password: String,
}

impl std::fmt::Debug for ResetPasswordRequest {
    /// 不印出密碼，避免寫進 log。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResetPasswordRequest { *** }")
    }
}

/// `DELETE /api/v1/users/{id}` 的 query string。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct DeleteUserQuery {
    /// 讀取時取得的版本號；不符時回傳 `409 CONFLICT`。
    pub version: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::macros::datetime;

    #[test]
    fn user_json_shape() {
        let id = crate::new_id();
        let user = User {
            id,
            username: "alice".into(),
            display_name: "Alice".into(),
            role: "editor".into(),
            role_name: "可編輯者".into(),
            is_active: true,
            is_locked: false,
            locked_until: None,
            must_change_password: true,
            last_login_at: Some(datetime!(2026-11-13 01:05:30 UTC)),
            created_at: datetime!(2026-11-12 08:00 UTC),
            updated_at: datetime!(2026-11-12 09:00 +8),
            version: 3,
        };
        let value = serde_json::to_value(&user).unwrap();
        assert_eq!(
            value,
            json!({
                "id": id.to_string(),
                "username": "alice",
                "display_name": "Alice",
                "role": "editor",
                "role_name": "可編輯者",
                "is_active": true,
                "is_locked": false,
                "locked_until": null,
                "must_change_password": true,
                "last_login_at": "2026-11-13T01:05:30Z",
                "created_at": "2026-11-12T08:00:00Z",
                "updated_at": "2026-11-12T09:00:00+08:00",
                "version": 3,
            })
        );
        let back: User = serde_json::from_value(value).unwrap();
        assert_eq!(back, user);
    }

    #[test]
    fn update_request_fields_are_optional() {
        let req: UpdateUserRequest = serde_json::from_value(json!({ "version": 2 })).unwrap();
        assert_eq!(
            req,
            UpdateUserRequest {
                version: 2,
                display_name: None,
                role: None,
                is_active: None,
            }
        );
        let req: UpdateUserRequest =
            serde_json::from_value(json!({ "version": 1, "is_active": false, "role": null }))
                .unwrap();
        assert_eq!(req.is_active, Some(false));
        assert_eq!(req.role, None);
        assert!(serde_json::from_value::<UpdateUserRequest>(json!({})).is_err());
    }

    #[test]
    fn debug_hides_passwords() {
        let create = CreateUserRequest {
            username: "bob".into(),
            display_name: "Bob".into(),
            role: "viewer".into(),
            password: "secret-password".into(),
        };
        let debug = format!("{create:?}");
        assert!(debug.contains("bob") && !debug.contains("secret-password"));
        let reset = ResetPasswordRequest {
            new_password: "secret-password".into(),
        };
        assert!(!format!("{reset:?}").contains("secret-password"));
    }
}
