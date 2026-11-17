//! 商業邏輯：單據流程、庫存 FIFO、匯率換算、權限判斷。
//!
//! - [`attachments`]：附件的上傳、下載、綁定到 `file` 欄位與清理
//! - [`audit`]：操作紀錄的 diff 與寫入
//! - [`audit_logs`]：操作紀錄查詢（分頁、篩選）
//! - [`auth`]：密碼、session token、session 有效判斷、登入鎖定、目前使用者與權限檢查
//! - [`currencies`]：幣別的列表、新增、修改（本位幣不可停用）
//! - [`exchange_rates`]：匯率的查詢、新增、修改、刪除，以及單據日期適用的匯率
//! - [`formulas`]：存檔時計算公式欄位
//! - [`layouts`]：列表版面（`view_layouts`）
//! - [`master_data`]：基本資料（單位、商品分類、商品、客戶、廠商）的業務規則
//! - [`numbering`]：單號規則的解析與產生
//! - [`record_data`]：依欄位定義驗證並正規化資料的 `data`
//! - [`record_lines`]：明細子表的明細與主表一起存檔時的檢查（比對 id、不可重複、操作紀錄）
//! - `record_titles`：產生資料標題時批次讀取關聯引用的目標資料
//! - [`records`]：自訂資料類別的資料（通用資料 API）
//! - [`recalculate`]：重新計算資料類別所有資料的公式欄位
//! - [`relations`]：關聯欄位的檢查與回應中的關聯資訊
//! - [`schema`]：資料類別與欄位定義的管理
//! - [`setup`]：初次啟動建立第一個管理者
//! - [`tax_rates`]：稅別列表
//! - [`title`]：資料標題（`title_template`）的解析與產生
//! - [`users`]：帳號管理與最後一個管理者保護
//! - `test_util`（`test-util` feature）：整合測試用，直接建立使用者與 session
//!
//! 相依方向見 `docs/architecture.md` §4。

pub mod attachments;
pub mod audit;
pub mod audit_logs;
pub mod auth;
pub mod currencies;
mod error;
pub mod exchange_rates;
pub mod formulas;
pub mod layouts;
pub mod master_data;
pub mod numbering;
pub mod recalculate;
pub mod record_data;
pub mod record_lines;
mod record_titles;
pub mod records;
pub mod relations;
pub mod schema;
pub mod setup;
pub mod tax_rates;
#[cfg(feature = "test-util")]
pub mod test_util;
pub mod title;
pub mod users;

pub use error::{CoreError, LAST_ADMIN_REASON};

/// 重新匯出資料存取層。`server` 只依賴 `core`（§4），
/// 連線池與 migration 透過這裡取得，不直接依賴 `hammer-db`。
pub use hammer_db as db;
