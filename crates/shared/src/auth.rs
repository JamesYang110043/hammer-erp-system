//! 登入、登出、目前使用者、修改密碼的 request / response（docs/architecture.md §6.2、§10）。
//!
//! | 端點 | request | response |
//! |---|---|---|
//! | `POST /api/v1/auth/login` | [`LoginRequest`] | [`LoginResponse`] |
//! | `POST /api/v1/auth/logout` | — | `204 No Content` |
//! | `GET /api/v1/auth/me` | — | [`CurrentUser`] |
//! | `POST /api/v1/auth/change-password` | [`ChangePasswordRequest`] | `204 No Content` |

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::Id;

/// `POST /api/v1/auth/login` 的輸入。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for LoginRequest {
    /// 不印出密碼，避免寫進 log。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginRequest")
            .field("username", &self.username)
            .field("password", &"***")
            .finish()
    }
}

/// `POST /api/v1/auth/login` 成功時的回應。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LoginResponse {
    /// Session token。之後的請求放在 `Authorization: Bearer <token>`。
    pub token: String,
    pub user: CurrentUser,
}

/// 目前登入的使用者（`GET /api/v1/auth/me`，以及登入回應中的 `user`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CurrentUser {
    pub id: Id,
    pub username: String,
    pub display_name: String,
    /// 角色代碼，對應 `role` 模組的常數（`admin` / `editor` / `viewer`）。
    pub role: String,
    /// 權限代碼，對應 `permission` 模組的常數，依代碼排序。
    pub permissions: Vec<String>,
    /// 為 `true` 時必須先修改密碼，其他 API 會回傳 `PASSWORD_CHANGE_REQUIRED`。
    pub must_change_password: bool,
}

/// `POST /api/v1/auth/change-password` 的輸入。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    /// 至少 10 個字元（§6.2）。
    pub new_password: String,
}

impl std::fmt::Debug for ChangePasswordRequest {
    /// 不印出密碼，避免寫進 log。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChangePasswordRequest { *** }")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn login_response_json_shape() {
        let id = crate::new_id();
        let body = LoginResponse {
            token: "t".into(),
            user: CurrentUser {
                id,
                username: "alice".into(),
                display_name: "Alice".into(),
                role: "editor".into(),
                permissions: vec!["record.read".into()],
                must_change_password: true,
            },
        };
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            json!({
                "token": "t",
                "user": {
                    "id": id.to_string(),
                    "username": "alice",
                    "display_name": "Alice",
                    "role": "editor",
                    "permissions": ["record.read"],
                    "must_change_password": true,
                }
            })
        );
    }

    #[test]
    fn requests_deserialize() {
        let req: LoginRequest =
            serde_json::from_value(json!({ "username": "a", "password": "p" })).unwrap();
        assert_eq!(req.username, "a");
        let req: ChangePasswordRequest =
            serde_json::from_value(json!({ "current_password": "old", "new_password": "new" }))
                .unwrap();
        assert_eq!(req.new_password, "new");
    }

    #[test]
    fn debug_hides_passwords() {
        let login = LoginRequest {
            username: "alice".into(),
            password: "secret-pass".into(),
        };
        assert!(!format!("{login:?}").contains("secret-pass"));
        let change = ChangePasswordRequest {
            current_password: "old-secret".into(),
            new_password: "new-secret".into(),
        };
        let debug = format!("{change:?}");
        assert!(!debug.contains("old-secret") && !debug.contains("new-secret"));
    }
}
