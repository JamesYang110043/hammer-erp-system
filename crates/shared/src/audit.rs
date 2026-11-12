//! 操作紀錄的動作代碼（`docs/architecture.md` §8.4 `audit_logs`）。
//!
//! 資料庫 `audit_logs.action` 存的就是這些字串。

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

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
}
