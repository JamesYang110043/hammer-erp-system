//! sqlx 資料存取與 migrations。
//!
//! - [`connect`]：依 [`PoolConfig`] 建立連線池
//! - [`run_migrations`]：套用 `crates/db/migrations/` 的 migration
//! - [`ensure_version_matched`]：樂觀鎖定檢查，版本不符時回傳 [`DbError::Conflict`]
//!
//! 共通欄位與 `custom_data` 慣例寫在 `migrations/0001_foundation.sql` 的註解。
//! 相依方向見 `docs/architecture.md` §4。

mod error;
mod migrate;
mod optimistic;
mod pool;

pub use error::DbError;
pub use migrate::{MIGRATOR, run_migrations};
pub use optimistic::ensure_version_matched;
pub use pool::{PoolConfig, connect};

/// 重新匯出 sqlx，讓上層使用相同版本。
pub use sqlx;
