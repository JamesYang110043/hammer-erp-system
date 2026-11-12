//! 商業邏輯：單據流程、庫存 FIFO、匯率換算、權限判斷。
//!
//! - [`audit`]：操作紀錄的 diff 與寫入
//! - [`auth`]：密碼、session token、session 有效判斷、登入鎖定、目前使用者與權限檢查
//! - [`setup`]：初次啟動建立第一個管理者
//! - `test_util`（`test-util` feature）：整合測試用，直接建立使用者與 session
//!
//! 相依方向見 `docs/architecture.md` §4。

pub mod audit;
pub mod auth;
mod error;
pub mod setup;
#[cfg(feature = "test-util")]
pub mod test_util;

pub use error::CoreError;

/// 重新匯出資料存取層。`server` 只依賴 `core`（§4），
/// 連線池與 migration 透過這裡取得，不直接依賴 `hammer-db`。
pub use hammer_db as db;
