//! 銷貨模組（訂單、出貨單、銷貨退回，docs/architecture.md §8.9）。
//!
//! - [`amounts`]：金額與稅額的計算（純邏輯；含稅、未稅、折扣）
//! - [`save`]：存檔時的稅別預設值、明細檢查與金額計算
//! - [`orders`]：訂單的確認、手動結案、作廢（[`SalesOrderHandler`]）
//! - [`shipments`]：出貨單存檔時的檢查、過帳與作廢（[`ShipmentHandler`]）
//! - [`returns`]：銷貨退回存檔時從出貨單帶入與檢查、過帳與作廢（[`SalesReturnHandler`]）

pub mod amounts;
pub mod orders;
pub mod returns;
pub mod save;
pub mod shipments;

pub use orders::SalesOrderHandler;
pub use returns::{SalesReturnHandler, derive_return};
pub use shipments::ShipmentHandler;

/// 訂單的資料類別代碼。
pub const SALES_ORDER: &str = "sales_order";
/// 訂單明細的明細子表代碼。
pub const SALES_ORDER_LINE: &str = "sales_order_line";
/// 出貨單的資料類別代碼。
pub const SHIPMENT: &str = "shipment";
/// 出貨明細的明細子表代碼。
pub const SHIPMENT_LINE: &str = "shipment_line";
/// 銷貨退回的資料類別代碼。
pub const SALES_RETURN: &str = "sales_return";
/// 銷貨退回明細的明細子表代碼。
pub const SALES_RETURN_LINE: &str = "sales_return_line";
