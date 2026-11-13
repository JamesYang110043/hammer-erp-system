//! 資料存取層的錯誤。
//!
//! 這裡不依賴 `hammer-shared`；轉成 API 錯誤碼（例如 `ErrorCode::Conflict`）
//! 是上層（`core`）的責任。

use sqlx::migrate::MigrateError;

/// 資料存取層的錯誤。
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// 樂觀鎖定衝突：資料已被其他人修改（§9.4），對應 `ErrorCode::Conflict`。
    #[error("此資料已被其他人修改，請重新載入")]
    Conflict,

    /// 執行 migration 失敗。
    #[error("資料庫 migration 失敗：{0}")]
    Migrate(#[from] MigrateError),

    /// 要拼接進 DDL 的識別字含有不允許的字元（[`crate::dynamic_index`]）。
    /// 上層應先檢查命名規則，發生時視為內部錯誤。
    #[error("不安全的識別字：{0:?}")]
    UnsafeIdentifier(String),

    /// 其他資料庫錯誤。
    #[error("資料庫錯誤：{0}")]
    Sqlx(#[from] sqlx::Error),
}

impl DbError {
    /// 是否為樂觀鎖定衝突。
    pub fn is_conflict(&self) -> bool {
        matches!(self, Self::Conflict)
    }

    /// 違反 UNIQUE 限制時，回傳限制名稱（例如 `users_username_key`）。
    pub fn unique_violation(&self) -> Option<&str> {
        match self {
            Self::Sqlx(sqlx::Error::Database(e)) if e.is_unique_violation() => e.constraint(),
            _ => None,
        }
    }
}
