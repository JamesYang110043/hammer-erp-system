//! sqlx 資料存取與 migrations。
//!
//! - [`connect`]：依 [`PoolConfig`] 建立連線池
//! - [`run_migrations`]：套用 `crates/db/migrations/` 的 migration
//! - [`ensure_version_matched`]：樂觀鎖定檢查，版本不符時回傳 [`DbError::Conflict`]
//! - [`audit`]：寫入 `audit_logs`
//! - [`session`]：`sessions` 的新增、查詢、更新與撤銷
//! - [`settings`]：讀取 `settings`
//! - [`user`]：認證用的使用者、角色與權限查詢
//!
//! 共通欄位與 `custom_data` 慣例寫在 `migrations/0001_foundation.sql` 的註解。
//! 相依方向見 `docs/architecture.md` §4。

pub mod audit;
mod error;
mod migrate;
mod optimistic;
mod pool;
pub mod session;
pub mod settings;
pub mod user;

pub use error::DbError;
pub use migrate::{MIGRATOR, run_migrations};
pub use optimistic::ensure_version_matched;
pub use pool::{PoolConfig, connect};

/// 重新匯出 sqlx，讓上層使用相同版本。
pub use sqlx;
