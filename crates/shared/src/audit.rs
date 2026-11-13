//! 操作紀錄（`docs/architecture.md` §8.4 `audit_logs`）。
//!
//! - 動作代碼常數：資料庫 `audit_logs.action` 存的就是這些字串
//! - 查詢 API 的 request / response：
//!
//! | 端點 | request | response |
//! |---|---|---|
//! | `GET /api/v1/audit-logs` | [`AuditLogQuery`]（query string） | [`Page`](crate::pagination::Page)`<`[`AuditLogEntry`]`>` |
//!
//! 需要 `audit.read` 權限。依 `occurred_at` 由新到舊排序，分頁慣例見 [`crate::pagination`]。
//!
//! 錯誤：
//!
//! - 參數不合法（`action` 不是已定義的代碼、`from` 晚於 `to`、`page` 小於 1 等）
//!   → `422 VALIDATION`，`details.field` 為參數名稱
//! - query string 格式錯誤（例如時間不是 RFC 3339、uuid 格式錯誤）→ `400 BAD_REQUEST`

use serde::{Deserialize, Serialize};
use serde_json::Value;
use specta::Type;
use time::OffsetDateTime;

use crate::Id;

/// 新增資料。
pub const CREATE: &str = "create";
/// 修改資料。
pub const UPDATE: &str = "update";
/// 刪除資料（含軟刪除）。
pub const DELETE: &str = "delete";
/// 單據過帳。
pub const POST: &str = "post";
/// 單據作廢。
pub const VOID: &str = "void";
/// 登入成功。
pub const LOGIN: &str = "login";
/// 登入失敗。
pub const LOGIN_FAILED: &str = "login_failed";
/// 登出。
pub const LOGOUT: &str = "logout";
/// 變更密碼（含管理者重設）。
pub const PASSWORD_CHANGE: &str = "password_change";
/// 匯出 Excel、列印。
pub const EXPORT: &str = "export";

/// 所有動作代碼。
pub const ALL: &[&str] = &[
    CREATE,
    UPDATE,
    DELETE,
    POST,
    VOID,
    LOGIN,
    LOGIN_FAILED,
    LOGOUT,
    PASSWORD_CHANGE,
    EXPORT,
];

/// 是否為已定義的動作代碼。
pub fn is_valid(action: &str) -> bool {
    ALL.contains(&action)
}

/// 操作紀錄的一筆資料（`GET /api/v1/audit-logs`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct AuditLogEntry {
    pub id: Id,
    /// 發生時間（RFC 3339）。
    #[serde(with = "time::serde::rfc3339")]
    #[specta(type = String)]
    pub occurred_at: OffsetDateTime,
    /// 操作者；帳號不存在的登入失敗為 `null`。
    pub user_id: Option<Id>,
    /// 操作者的登入帳號；沒有操作者時為 `null`。
    pub username: Option<String>,
    /// 操作者的顯示名稱；沒有操作者時為 `null`。
    pub user_display_name: Option<String>,
    /// 動作代碼（本模組的常數）。
    pub action: String,
    /// 資料類別代碼。
    pub entity_key: Option<String>,
    pub record_id: Option<Id>,
    /// 變更前（只包含有變動的欄位）。任意 JSON，TypeScript 為 `unknown`。
    #[specta(type = Option<specta_typescript::Unknown>)]
    pub before_data: Option<Value>,
    /// 變更後（只包含有變動的欄位）。任意 JSON，TypeScript 為 `unknown`。
    #[specta(type = Option<specta_typescript::Unknown>)]
    pub after_data: Option<Value>,
    pub client_info: Option<String>,
}

/// `GET /api/v1/audit-logs` 的 query string。所有條件都可省略，有多個條件時須全部符合。
///
/// 時間使用 RFC 3339。query string 中的 `+` 會被當成空白，時區位移請寫成 `%2B08:00`，
/// 或改用 UTC（`Z`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AuditLogQuery {
    /// 發生時間 >= `from`。
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[specta(type = Option<String>)]
    pub from: Option<OffsetDateTime>,
    /// 發生時間 < `to`（不含）。
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[specta(type = Option<String>)]
    pub to: Option<OffsetDateTime>,
    /// 操作者。
    #[serde(default)]
    pub user_id: Option<Id>,
    /// 動作代碼，必須是本模組定義的代碼。
    #[serde(default)]
    pub action: Option<String>,
    /// 資料類別代碼。
    #[serde(default)]
    pub entity_key: Option<String>,
    #[serde(default)]
    pub record_id: Option<Id>,
    /// 頁碼，從 1 開始（預設 1）。
    #[serde(default)]
    pub page: Option<u32>,
    /// 每頁筆數（預設 50，上限 200）。
    #[serde(default)]
    pub page_size: Option<u32>,
}

/// [`AuditLogQuery`] 驗證錯誤 `details.field` 的參數名稱。
pub mod query_field {
    pub const FROM: &str = "from";
    pub const ACTION: &str = "action";
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;
    use time::macros::datetime;

    #[test]
    fn codes_are_unique() {
        let set: HashSet<_> = ALL.iter().collect();
        assert_eq!(set.len(), ALL.len());
        assert_eq!(ALL.len(), 10);
    }

    #[test]
    fn is_valid_checks_known_codes() {
        assert!(is_valid(LOGIN_FAILED));
        assert!(!is_valid("Login"));
        assert!(!is_valid(""));
    }

    #[test]
    fn entry_json_shape() {
        let id = crate::new_id();
        let entry = AuditLogEntry {
            id,
            occurred_at: datetime!(2026-11-13 01:05:30 UTC),
            user_id: None,
            username: None,
            user_display_name: None,
            action: LOGIN_FAILED.into(),
            entity_key: Some("users".into()),
            record_id: None,
            before_data: None,
            after_data: Some(json!({ "username": "ghost" })),
            client_info: Some("HammerERP/0.1.0".into()),
        };
        let value = serde_json::to_value(&entry).unwrap();
        assert_eq!(
            value,
            json!({
                "id": id.to_string(),
                "occurred_at": "2026-11-13T01:05:30Z",
                "user_id": null,
                "username": null,
                "user_display_name": null,
                "action": "login_failed",
                "entity_key": "users",
                "record_id": null,
                "before_data": null,
                "after_data": { "username": "ghost" },
                "client_info": "HammerERP/0.1.0",
            })
        );
        let back: AuditLogEntry = serde_json::from_value(value).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn query_fields_are_optional() {
        let q: AuditLogQuery = serde_json::from_value(json!({})).unwrap();
        assert_eq!(q, AuditLogQuery::default());
        let q: AuditLogQuery = serde_json::from_value(json!({
            "from": "2026-11-13T00:00:00+08:00",
            "action": "login",
            "page_size": 20,
        }))
        .unwrap();
        assert_eq!(q.from, Some(datetime!(2026-11-12 16:00 UTC)));
        assert_eq!(q.action.as_deref(), Some("login"));
        assert_eq!(q.page_size, Some(20));
        assert!(serde_json::from_value::<AuditLogQuery>(json!({ "from": "2026-11-13" })).is_err());
    }
}
