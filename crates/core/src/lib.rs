//! 商業邏輯：單據流程、庫存 FIFO、匯率換算、權限判斷。
//!
//! - [`audit`]：操作紀錄的 diff 與寫入
//! - [`audit_logs`]：操作紀錄查詢（分頁、篩選）
//! - [`auth`]：密碼、session token、session 有效判斷、登入鎖定、目前使用者與權限檢查
//! - [`layouts`]：列表版面（`view_layouts`）
//! - [`numbering`]：單號規則的解析與產生
//! - [`record_data`]：依欄位定義驗證並正規化資料的 `data`
//! - [`records`]：自訂資料類別的資料（通用資料 API）
//! - [`schema`]：資料類別與欄位定義的管理
//! - [`setup`]：初次啟動建立第一個管理者
//! - [`title`]：資料標題（`title_template`）的解析與產生
//! - [`users`]：帳號管理與最後一個管理者保護
//! - `test_util`（`test-util` feature）：整合測試用，直接建立使用者與 session
//!
//! 相依方向見 `docs/architecture.md` §4。

pub mod audit;
pub mod audit_logs;
pub mod auth;
mod error;
pub mod layouts;
pub mod numbering;
pub mod record_data;
pub mod records;
pub mod schema;
pub mod setup;
#[cfg(feature = "test-util")]
pub mod test_util;
pub mod title;
pub mod users;

pub use error::{CoreError, LAST_ADMIN_REASON};

/// 重新匯出資料存取層。`server` 只依賴 `core`（§4），
/// 連線池與 migration 透過這裡取得，不直接依賴 `hammer-db`。
pub use hammer_db as db;
