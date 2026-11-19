//! 發票模組（銷項發票、銷貨折讓、進項憑證、進貨折讓，docs/architecture.md §8.11）。
//!
//! 只做內部紀錄：發票在別處開立，系統登錄號碼與金額，不上傳財政部、不做字軌配號。
//!
//! - [`save`]：存檔時由伺服器寫入的期別（[`save::tax_period`]）與折讓從原發票帶入的表頭
//!   （[`save::derive_allowance`]）

pub mod save;

/// 銷項發票的資料類別代碼。
pub const SALES_INVOICE: &str = "sales_invoice";
/// 發票明細的明細子表代碼。
pub const SALES_INVOICE_LINE: &str = "sales_invoice_line";
/// 發票關聯出貨單的明細子表代碼。
pub const SALES_INVOICE_SHIPMENT: &str = "sales_invoice_shipment";
/// 銷貨折讓的資料類別代碼。
pub const SALES_ALLOWANCE: &str = "sales_allowance";
/// 折讓明細的明細子表代碼。
pub const SALES_ALLOWANCE_LINE: &str = "sales_allowance_line";
/// 進項憑證的資料類別代碼。
pub const PURCHASE_INVOICE: &str = "purchase_invoice";
/// 憑證關聯進貨單的明細子表代碼。
pub const PURCHASE_INVOICE_RECEIPT: &str = "purchase_invoice_receipt";
/// 進貨折讓的資料類別代碼。
pub const PURCHASE_ALLOWANCE: &str = "purchase_allowance";
