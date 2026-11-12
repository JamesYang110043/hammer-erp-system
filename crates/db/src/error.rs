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

    /// 其他資料庫錯誤。
    #[error("資料庫錯誤：{0}")]
    Sqlx(#[from] sqlx::Error),
}

impl DbError {
    /// 是否為樂觀鎖定衝突。
    pub fn is_conflict(&self) -> bool {
        matches!(self, Self::Conflict)
    }
}
