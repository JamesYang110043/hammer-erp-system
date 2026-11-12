//! 權限代碼（`docs/architecture.md` §6.1）。
//!
//! 資料庫 `role_permissions.permission` 存的就是這些字串。

/// 查看業務資料。
pub const RECORD_READ: &str = "record.read";
/// 新增、修改業務資料。
pub const RECORD_WRITE: &str = "record.write";
/// 過帳、作廢單據。
pub const RECORD_POST: &str = "record.post";
/// 匯出 Excel、列印。
pub const DATA_EXPORT: &str = "data.export";
/// 欄位與資料類別客製化。
pub const SCHEMA_MANAGE: &str = "schema.manage";
/// 帳號管理。
pub const USER_MANAGE: &str = "user.manage";
/// 系統設定（單號規則、匯率等）。
pub const SETTINGS_MANAGE: &str = "settings.manage";
/// 查看操作紀錄。
pub const AUDIT_READ: &str = "audit.read";

/// 所有權限代碼。
pub const ALL: &[&str] = &[
    RECORD_READ,
    RECORD_WRITE,
    RECORD_POST,
    DATA_EXPORT,
    SCHEMA_MANAGE,
    USER_MANAGE,
    SETTINGS_MANAGE,
    AUDIT_READ,
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn codes_are_unique() {
        let set: HashSet<_> = ALL.iter().collect();
        assert_eq!(set.len(), ALL.len());
        assert_eq!(ALL.len(), 8);
    }
}
