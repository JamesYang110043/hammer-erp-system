//! Migration 執行。
//!
//! Migration 檔案放在 `crates/db/migrations/`，編譯時嵌入執行檔，
//! 不需要連線資料庫也能編譯。

use sqlx::PgPool;
use sqlx::migrate::Migrator;

use crate::DbError;

/// 嵌入 `crates/db/migrations/` 的所有 migration。
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// 套用所有尚未執行的 migration。已套用過的會略過，重複執行不會出錯。
pub async fn run_migrations(pool: &PgPool) -> Result<(), DbError> {
    MIGRATOR.run(pool).await?;
    Ok(())
}
