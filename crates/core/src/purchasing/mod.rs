//! 進貨模組（採購單、進貨單、進貨退回，docs/architecture.md §8.8）。
//!
//! - [`amounts`]：金額與稅額的計算（純邏輯）
//! - [`save`]：採購單、進貨單存檔時的幣別、匯率、稅別預設值與金額計算（進貨退回也使用金額計算）
//! - [`orders`]：採購單的確認、手動結案、作廢（[`PurchaseOrderHandler`]）
//! - [`receipts`]：進貨單存檔時的檢查、過帳與作廢（[`GoodsReceiptHandler`]）
//! - [`returns`]：進貨退回存檔時從進貨單帶入、退回數量上限、過帳與作廢（[`PurchaseReturnHandler`]）

pub mod amounts;
pub mod orders;
pub mod receipts;
pub mod returns;
pub mod save;

pub use orders::PurchaseOrderHandler;
pub use receipts::GoodsReceiptHandler;
pub use returns::{PurchaseReturnHandler, derive_return};

/// 採購單的資料類別代碼。
pub const PURCHASE_ORDER: &str = "purchase_order";
/// 進貨單的資料類別代碼。
pub const GOODS_RECEIPT: &str = "goods_receipt";
/// 進貨退回的資料類別代碼。
pub const PURCHASE_RETURN: &str = "purchase_return";
/// 進貨退回明細的明細子表代碼。
pub const PURCHASE_RETURN_LINE: &str = "purchase_return_line";
