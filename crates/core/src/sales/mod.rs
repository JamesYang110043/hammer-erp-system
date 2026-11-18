//! 銷貨模組（訂單、出貨單、銷貨退回，docs/architecture.md §8.9）。
//!
//! - [`amounts`]：金額與稅額的計算（純邏輯；含稅、未稅、折扣）
//! - [`save`]：存檔時的稅別預設值、明細檢查與金額計算
//! - [`orders`]：訂單的確認、手動結案、作廢（[`SalesOrderHandler`]）
//! - [`returns`]：銷貨退回存檔時從出貨單帶入（[`derive_return`]）
//!
//! 出貨單的過帳與作廢、銷貨退回的過帳與作廢在之後的 issue 加入（#148、#150）。

pub mod amounts;
pub mod orders;
pub mod returns;
pub mod save;

pub use orders::SalesOrderHandler;
pub use returns::derive_return;

/// 訂單的資料類別代碼。
pub const SALES_ORDER: &str = "sales_order";
/// 訂單明細的明細子表代碼。
pub const SALES_ORDER_LINE: &str = "sales_order_line";
/// 出貨單的資料類別代碼。
pub const SHIPMENT: &str = "shipment";
/// 銷貨退回的資料類別代碼。
pub const SALES_RETURN: &str = "sales_return";
/// 銷貨退回明細的明細子表代碼。
pub const SALES_RETURN_LINE: &str = "sales_return_line";
