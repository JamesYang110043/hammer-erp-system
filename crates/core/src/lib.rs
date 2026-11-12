//! 商業邏輯：單據流程、庫存 FIFO、匯率換算、權限判斷。
//!
//! - [`auth`]：密碼、session token、session 有效判斷、登入鎖定
//!
//! 相依方向見 `docs/architecture.md` §4。

pub mod auth;
mod error;

pub use error::CoreError;

/// 重新匯出資料存取層。`server` 只依賴 `core`（§4），
/// 連線池與 migration 透過這裡取得，不直接依賴 `hammer-db`。
pub use hammer_db as db;
