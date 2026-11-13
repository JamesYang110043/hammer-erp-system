//! sqlx 資料存取與 migrations。
//!
//! - [`connect`]：依 [`PoolConfig`] 建立連線池
//! - [`run_migrations`]：套用 `crates/db/migrations/` 的 migration
//! - [`ensure_version_matched`]：樂觀鎖定檢查，版本不符時回傳 [`DbError::Conflict`]
//! - [`audit`]：寫入與查詢 `audit_logs`
//! - [`dynamic_index`]：自訂欄位的動態索引（可搜尋、不可重複）
//! - [`lock`]：advisory lock
//! - [`record`]：自訂資料類別的資料（`custom_records`）
//! - [`role`]：角色與權限查詢
//! - [`schema`]：資料類別、欄位定義與定義變更紀錄
//! - [`session`]：`sessions` 的新增、查詢、更新與撤銷
//! - [`settings`]：讀取 `settings`
//! - [`user`]：認證用的使用者查詢，以及帳號的建立、修改、停用、刪除
//!
//! 共通欄位與 `custom_data` 慣例寫在 `migrations/0001_foundation.sql` 的註解。
//! 相依方向見 `docs/architecture.md` §4。

pub mod audit;
pub mod dynamic_index;
mod error;
pub mod lock;
mod migrate;
mod optimistic;
mod pool;
pub mod record;
pub mod role;
pub mod schema;
pub mod session;
pub mod settings;
pub mod user;

pub use error::DbError;
pub use migrate::{MIGRATOR, run_migrations};
pub use optimistic::ensure_version_matched;
pub use pool::{PoolConfig, connect};

/// 重新匯出 sqlx，讓上層使用相同版本。
pub use sqlx;
